use std::collections::BTreeMap;

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::Result;
use crate::git::Git;
use crate::paths;
use crate::state::{self, State};

const HEADER: [&str; 5] = ["id", "state", "branch", "stage", "worker"];
const GAP: &str = "  ";
const NONE: &str = "-";

/// One line per board row. Every cell derives from git.
pub fn status(repo: &Git, project: &str) -> Result<()> {
    let config = Config::from_main(repo)?;
    let board = Board::from_main(repo, &config)?;
    let total = config.stages.len();
    let states = state::derive(&board, total, repo)?;
    let slots = slots(repo, project)?;
    print!("{}", render(&board, &states, total, &slots));
    Ok(())
}

/// Slot number holding each branch. A worktree checked out on a branch is that
/// branch's worker, so the running slot needs no stored state.
fn slots(repo: &Git, project: &str) -> Result<BTreeMap<String, usize>> {
    let mut slots = BTreeMap::new();
    for (branch, path) in repo.checkouts()? {
        if let Some(index) = paths::slot_index(project, &path)? {
            slots.insert(branch, index);
        }
    }
    Ok(slots)
}

fn render(
    board: &Board,
    states: &[State],
    total: usize,
    slots: &BTreeMap<String, usize>,
) -> String {
    let mut table = vec![HEADER.map(str::to_string)];
    table.extend(board.rows().iter().zip(states).map(|(row, state)| {
        [
            row.id().to_string(),
            state.to_string(),
            branch_cell(row, *state),
            stage_cell(*state, total),
            worker_cell(row, slots),
        ]
    }));
    let widths = column_widths(&table);
    table.iter().map(|row| line(row, &widths)).collect()
}

fn branch_cell(row: &Row, state: State) -> String {
    match state {
        State::Ready | State::Blocked => NONE.to_string(),
        _ => state::branch(row.id()),
    }
}

fn stage_cell(state: State, total: usize) -> String {
    match state {
        State::Running { stage } => format!("{stage}/{total}"),
        State::Landed | State::Done => format!("{total}/{total}"),
        State::Ready | State::Blocked => NONE.to_string(),
    }
}

fn worker_cell(row: &Row, slots: &BTreeMap<String, usize>) -> String {
    slots
        .get(&state::branch(row.id()))
        .map_or_else(|| NONE.to_string(), usize::to_string)
}

fn column_widths(table: &[[String; 5]]) -> [usize; 5] {
    let mut widths = [0; 5];
    for row in table {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    widths
}

fn line(row: &[String; 5], widths: &[usize; 5]) -> String {
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
