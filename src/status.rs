use std::collections::BTreeMap;

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::Result;
use crate::git::Git;
use crate::paths::{self, Project};
use crate::state;

const HEADER: [&str; 4] = ["id", "state", "branch", "worker"];
const COLUMNS: usize = HEADER.len();
const GAP: &str = "  ";
const NONE: &str = "-";

type Line = [String; COLUMNS];

/// One line per board row. Every cell derives from git.
pub fn status(repo: &Git, project: &Project) -> Result<()> {
    let config = Config::from_trunk(repo)?;
    let board = Board::from_trunk(repo, &config)?;
    let states = state::derive(&board, repo)?;
    let slots = slots(repo, project)?;
    let mut table = vec![HEADER.map(str::to_string)];
    for (row, state) in board.rows().iter().zip(&states) {
        table.push([
            row.id.clone(),
            state.to_string(),
            branch_cell(repo, row)?,
            worker_cell(row, &slots),
        ]);
    }
    print!("{}", render(&table));
    Ok(())
}

/// Slot number holding each branch. A worktree checked out on a branch is that
/// branch's worker, so the running slot needs no stored state.
fn slots(repo: &Git, project: &Project) -> Result<BTreeMap<String, usize>> {
    let mut slots = BTreeMap::new();
    for (branch, path) in repo.checkouts()? {
        if let Some(index) = paths::slot_index(project, &path)? {
            slots.insert(branch, index);
        }
    }
    Ok(slots)
}

/// A row that never started, and one whose merge failed, hold no branch.
fn branch_cell(repo: &Git, row: &Row) -> Result<String> {
    let branch = state::branch(&row.id);
    Ok(match repo.branch_exists(&branch)? {
        true => branch,
        false => NONE.to_string(),
    })
}

fn worker_cell(row: &Row, slots: &BTreeMap<String, usize>) -> String {
    slots
        .get(&state::branch(&row.id))
        .map_or_else(|| NONE.to_string(), usize::to_string)
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
