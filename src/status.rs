use std::collections::BTreeMap;
use std::env;
use std::io::{self, IsTerminal};

use crate::board;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::lock;
use crate::paths::{self, Project};
use crate::state::{self, Address, Key, State};

const HEADER: [&str; 5] = ["project", "row", "state", "branch", "worker"];
const COLUMNS: usize = HEADER.len();
const STATE_COLUMN: usize = 2;
const GAP: &str = "  ";
const NONE: &str = "-";
const RESET: &str = "\x1b[0m";

type Line = [String; COLUMNS];

struct Row {
    cells: Line,
    state: State,
}

/// One repo to list: a bare repo of the swarm, or a clone with no project.
pub struct Source {
    pub project: Option<Project>,
    pub repo: Git,
    /// Track branch prefix.
    pub tracks: String,
}

/// One line per row of every source, from git and row locks; a clone reads running
/// as queued. Exit 1 on a failed track check or any of `faults`.
pub fn status(sources: &[Source], mut faults: Vec<Error>) -> Result<()> {
    let mut rows = Vec::new();
    for source in sources {
        faults.extend(list(source, &mut rows)?);
    }
    print!("{}", render(&rows, color_enabled()));
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
fn list(source: &Source, rows: &mut Vec<Row>) -> Result<Vec<Error>> {
    let (repo, project) = (&source.repo, source.project.as_ref());
    let scan = board::scan(repo, &source.tracks)?;
    let mut faults = scan.faults;
    let held = |key: &Key| match project {
        Some(project) => lock::held(&paths::transcripts(&Address {
            project: project.clone(),
            key: key.clone(),
        })?),
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
            rows.push(Row {
                cells: [
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
                ],
                state: *state,
            });
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

/// Color only on a terminal, unless `NO_COLOR` holds a value.
fn color_enabled() -> bool {
    io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
}

fn render(rows: &[Row], color: bool) -> String {
    let header = HEADER.map(str::to_string);
    let widths = column_widths(&header, rows);
    let mut table = line(&header, &widths, None);
    for row in rows {
        table.push_str(&line(&row.cells, &widths, color.then_some(row.state)));
    }
    table
}

fn column_widths(header: &Line, rows: &[Row]) -> [usize; COLUMNS] {
    let mut widths = [0; COLUMNS];
    for cells in std::iter::once(header).chain(rows.iter().map(|row| &row.cells)) {
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = (*width).max(cell.chars().count());
        }
    }
    widths
}

/// Color wraps the padded state cell, so escape codes never count toward width.
fn line(row: &Line, widths: &[usize; COLUMNS], color: Option<State>) -> String {
    let last = row.len() - 1;
    let cells: Vec<String> = row
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(index, (cell, &width))| {
            let padded = if index == last {
                cell.clone()
            } else {
                format!("{cell:<width$}")
            };
            match color {
                Some(state) if index == STATE_COLUMN => {
                    format!("{}{padded}{RESET}", escape(state))
                }
                _ => padded,
            }
        })
        .collect();
    format!("{}\n", cells.join(GAP))
}

fn escape(state: State) -> &'static str {
    match state {
        State::Waiting => "\x1b[2m",
        State::Queued => "\x1b[33m",
        State::Running => "\x1b[94m",
        State::Failed => "\x1b[31m",
        State::Pushed => "\x1b[32m",
        State::Merged => "\x1b[35m",
    }
}
