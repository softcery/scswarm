use std::fmt;

use crate::board::Board;
use crate::error::{Error, Result};
use crate::git::{self, Git};

pub const TASK_TRAILER: &str = "Swarm-Task";
pub const STAGE_TRAILER: &str = "Swarm-Stage";

/// Row standing. Read from git, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Blocked,
    Ready,
    Running { stage: usize },
    Landed,
    Done,
}

pub fn branch(id: &str) -> String {
    format!("swarm/{id}")
}

pub fn derive(board: &Board, total: usize, git: &Git) -> Result<Vec<State>> {
    let progress = board
        .rows()
        .iter()
        .map(|row| row_progress(git, row.id(), total))
        .collect::<Result<Vec<Progress>>>()?;
    Ok(board
        .rows()
        .iter()
        .zip(&progress)
        .map(|(row, own)| match own {
            Progress::Absent if row.after.iter().all(|&dep| progress[dep].settled()) => {
                State::Ready
            }
            Progress::Absent => State::Blocked,
            Progress::Stage(done) => State::Running { stage: done + 1 },
            Progress::Landed => State::Landed,
            Progress::Done => State::Done,
        })
        .collect())
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Blocked => "blocked",
            State::Ready => "ready",
            State::Running { .. } => "running",
            State::Landed => "landed",
            State::Done => "done",
        })
    }
}

/// Stages committed on row branch.
#[derive(Debug, Clone, Copy)]
enum Progress {
    Absent,
    Stage(usize),
    Landed,
    Done,
}

impl Progress {
    fn settled(self) -> bool {
        matches!(self, Progress::Landed | Progress::Done)
    }
}

fn row_progress(git: &Git, id: &str, total: usize) -> Result<Progress> {
    let branch = branch(id);
    if !git.branch_exists(&branch)? {
        return Ok(Progress::Absent);
    }
    let stamp = git.trailer_commit(&branch, TASK_TRAILER, id, STAGE_TRAILER)?;
    let Some(trailer) = stamp else {
        return Ok(Progress::Stage(0));
    };
    let done = committed_stages(&trailer.value, &branch)?;
    if done < total {
        return Ok(Progress::Stage(done));
    }
    if git.is_ancestor(&trailer.commit, git::MAIN)? {
        Ok(Progress::Done)
    } else {
        Ok(Progress::Landed)
    }
}

fn committed_stages(trailer: &str, branch: &str) -> Result<usize> {
    trailer
        .split_once('/')
        .and_then(|(done, _)| done.trim().parse().ok())
        .ok_or_else(|| {
            Error::Failed(format!(
                "branch {branch}: trailer `{STAGE_TRAILER}` must read `<n>/<total>`"
            ))
        })
}
