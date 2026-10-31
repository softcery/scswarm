use std::collections::BTreeMap;

use crate::board;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::lock;
use crate::paths::{self, Project, MERGE_LOG, RUN_LOG};
use crate::state::{self, Address, Key};
use crate::transcript::{Context, Entry, Kind, Transcript};

const HEADER: [&str; 7] = [
    "project", "row", "state", "branch", "worker", "started", "context",
];
const COLUMNS: usize = HEADER.len();
const GAP: &str = "  ";
const NONE: &str = "-";
const SECONDS_PER_DAY: u64 = 86_400;
const TOKENS_PER_UNIT: u64 = 1_000;

type Line = [String; COLUMNS];

/// One repo to list: a bare repo of the swarm, or a clone with no project.
pub struct Source {
    pub project: Option<Project>,
    pub repo: Git,
    /// Track branch prefix.
    pub tracks: String,
}

/// One line per row of every source, from git, row locks and host transcripts; a
/// clone reads running as queued and has no transcripts. Exit 1 on a failed track check or any of `faults`.
pub fn status(sources: &[Source], mut faults: Vec<Error>) -> Result<()> {
    let mut table = vec![HEADER.map(str::to_string)];
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
    let address = |key: &Key| {
        project.map(|project| Address {
            project: project.clone(),
            key: key.clone(),
        })
    };
    let held = |key: &Key| match address(key) {
        Some(row) => lock::held(&paths::transcripts(&row)?),
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
            let usage = match address(&key) {
                Some(row) => Usage::read(&row)?,
                None => Usage::default(),
            };
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
                usage.started.map_or_else(|| NONE.to_string(), utc),
                usage.context.map_or_else(|| NONE.to_string(), tokens),
            ]);
        }
    }
    Ok(faults)
}

/// What the host transcripts of one row tell beyond git.
#[derive(Default)]
struct Usage {
    /// Unix seconds at the last attempt start.
    started: Option<u64>,
    /// Latest context across the row session and its merge session.
    context: Option<Context>,
}

impl Usage {
    fn read(row: &Address) -> Result<Self> {
        let run = Transcript::at(paths::transcript(row, RUN_LOG)?).entries()?;
        let merge = Transcript::at(paths::transcript(row, MERGE_LOG)?).entries()?;
        let started = run
            .iter()
            .rev()
            .find_map(|entry| matches!(entry.kind, Kind::Start { .. }).then_some(entry.at));
        let context = run
            .iter()
            .chain(&merge)
            .filter_map(|entry: &Entry| match &entry.kind {
                Kind::Try(record) => record.context.map(|context| (entry.at, context)),
                _ => None,
            })
            .max_by_key(|(at, _)| *at)
            .map(|(_, context)| context);
        Ok(Self { started, context })
    }
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

/// Unix seconds as UTC minutes, `YYYY-MM-DDTHH:MMZ`. Days to civil date by
/// Howard Hinnant's `civil_from_days`, eras of 400 years from 0000-03-01.
fn utc(seconds: u64) -> String {
    let (days, rest) = (seconds / SECONDS_PER_DAY, seconds % SECONDS_PER_DAY);
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60
    )
}

/// Thousands of tokens, rounded down: `used/window`.
fn tokens(context: Context) -> String {
    format!(
        "{}k/{}k",
        context.used / TOKENS_PER_UNIT,
        context.window / TOKENS_PER_UNIT
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_across_leap_day_and_century() {
        assert_eq!(utc(0), "1970-01-01T00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00Z");
        assert_eq!(utc(1_793_115_780), "2026-10-27T15:43Z");
    }
}
