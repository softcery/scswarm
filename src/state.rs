use std::fmt;

use crate::board::Board;
use crate::error::Result;
use crate::git::{self, Git};

const LAND_REFS: &str = "refs/swarm/land/";
const FAIL_REFS: &str = "refs/swarm/fail/";

/// Row standing. Read from git, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Blocked,
    Ready,
    Running,
    Failed,
    Landed,
    Done,
}

pub fn branch(id: &str) -> String {
    format!("swarm/{id}")
}

/// Ref holding the commit a row landed. Sole record of finished work.
pub fn land_ref(id: &str) -> String {
    format!("{LAND_REFS}{id}")
}

/// Trunk tip a row last failed against. Outside `refs/heads/`: lists as no branch.
pub fn fail_ref(id: &str) -> String {
    format!("{FAIL_REFS}{id}")
}

pub fn derive(board: &Board, git: &Git) -> Result<Vec<State>> {
    let tip = git.tip(git::TRUNK)?;
    let own = board
        .rows()
        .iter()
        .map(|row| row_own(git, &row.id, &tip))
        .collect::<Result<Vec<Option<State>>>>()?;
    Ok(board
        .rows()
        .iter()
        .zip(&own)
        .map(|(row, state)| match state {
            Some(state) => *state,
            None if row.after.iter().all(|&parent| settled(own[parent])) => State::Ready,
            None => State::Blocked,
        })
        .collect())
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Blocked => "blocked",
            State::Ready => "ready",
            State::Running => "running",
            State::Failed => "failed",
            State::Landed => "landed",
            State::Done => "done",
        })
    }
}

/// Standing from the row's own refs. `None`: not started, so parents decide.
fn row_own(git: &Git, id: &str, tip: &str) -> Result<Option<State>> {
    if let Some(commit) = git.ref_commit(&land_ref(id))? {
        return Ok(Some(match git.is_ancestor(&commit, git::TRUNK)? {
            true => State::Done,
            false => State::Landed,
        }));
    }
    if git.ref_commit(&fail_ref(id))?.as_deref() == Some(tip) {
        return Ok(Some(State::Failed));
    }
    Ok(git.branch_exists(&branch(id))?.then_some(State::Running))
}

/// Parent a dependant can branch from.
fn settled(state: Option<State>) -> bool {
    matches!(state, Some(State::Landed | State::Done))
}
