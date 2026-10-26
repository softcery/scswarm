use std::collections::BTreeMap;

use crate::board;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::lock;
use crate::paths::{self, Project};
use crate::state::{self, Key};

const HEADER: [&str; 5] = ["project", "row", "state", "branch", "worker"];
const COLUMNS: usize = HEADER.len();
const GAP: &str = "  ";
const NONE: &str = "-";

type Line = [String; COLUMNS];

/// One repo to list: a bare repo of the swarm, or a clone with no project.
pub struct Source {
    pub project: Option<Project>,
    pub repo: Git,
    /// Track branch prefix.
    pub tracks: String,
}

/// One line per row of every track of every source, from git and row locks. A
/// clone has no locks, so running reads as queued. A failed track check exits 1.
pub fn status(sources: &[Source]) -> Result<()> {
    let mut table = vec![HEADER.map(str::to_string)];
    let mut faults = Vec::new();
    for source in sources {
        faults.extend(list(source, &mut table)?);
    }
    print!("{}", render(&table));
    match faults.is_empty() {
        true => Ok(()),
        false => Err(Error::Invalid(
            faults
                .iter()
                .map(Error::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// Lines of one source, plus the fault of each track skipped.
fn list(source: &Source, table: &mut Vec<Line>) -> Result<Vec<Error>> {
    let (repo, project) = (&source.repo, source.project.as_ref());
    let scan = board::scan(repo, &source.tracks)?;
    let mut faults = scan.faults;
    let held = |key: &Key| match project {
        Some(project) => lock::held(&paths::transcripts(project, key)?),
        None => Ok(false),
    };
    let slots = match project {
        Some(_) => slots(repo)?,
        None => BTreeMap::new(),
    };
    let named = project.map_or_else(|| NONE.to_string(), Project::to_string);
    let branches = repo.refs(git::HEADS)?;
    for track in scan.tracks {
        let snapshot = match state::snapshot(repo, track, &held) {
            Ok(snapshot) => snapshot,
            Err(why @ Error::Invalid(_)) => {
                faults.push(why);
                continue;
            }
            Err(why) => return Err(why),
        };
        for (row, state) in snapshot.board.rows().iter().zip(&snapshot.states) {
            let key = Key::new(&snapshot.track.name, &row.id);
            let branch = key.branch();
            table.push([
                named.clone(),
                key.to_string(),
                state.to_string(),
                match branches.contains_key(&git::branch_ref(&branch)) {
                    true => branch.clone(),
                    false => NONE.to_string(),
                },
                slots
                    .get(&branch)
                    .map_or_else(|| NONE.to_string(), usize::to_string),
            ]);
        }
    }
    Ok(faults)
}

/// Slot number holding each branch. A worktree checked out on a branch is that
/// branch's worker, so the running slot needs no stored state.
fn slots(repo: &Git) -> Result<BTreeMap<String, usize>> {
    let mut slots = BTreeMap::new();
    for (branch, path) in repo.checkouts()? {
        if let Some(index) = paths::slot_index(&path)? {
            slots.insert(branch, index);
        }
    }
    Ok(slots)
}

fn render(table: &[Line]) -> String {
    let widths = column_widths(table);
    table.iter().map(|row| line(row, &widths)).collect()
}

fn column_widths(table: &[Line]) -> [usize; COLUMNS] {
    let mut widths = [0; COLUMNS];
    for row in table {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    widths
}

fn line(row: &Line, widths: &[usize; COLUMNS]) -> String {
    let last = row.len() - 1;
    let cells: Vec<String> = row
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(index, (cell, &width))| {
            if index == last {
                cell.clone()
            } else {
                format!("{cell:<width$}")
            }
        })
        .collect();
    format!("{}\n", cells.join(GAP))
}
