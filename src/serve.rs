use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::Result;
use crate::git::Git;
use crate::pool::{Job, Pool};
use crate::state::{self, State};

const POLL: Duration = Duration::from_secs(5);
const FIRST_STAGE: usize = 1;

/// Fetch, derive, fill idle slots with pending rows in board order, sleep. Until killed.
pub fn serve(repo: &Git, project: &str) -> Result<()> {
    let config = Config::from_main(repo)?;
    let mut pool = Pool::open(repo, project, &config)?;
    loop {
        cycle(repo, &mut pool)?;
        thread::sleep(POLL);
    }
}

fn cycle(repo: &Git, pool: &mut Pool) -> Result<()> {
    pool.collect()?;
    repo.fetch()?;
    let config = Arc::new(Config::from_main(repo)?);
    let board = Board::from_main(repo, &config)?;
    let states = state::derive(&board, config.stages.len(), repo)?;
    for (index, row) in board.rows().iter().enumerate() {
        if !pending(&states[index]) || pool.holds(row.id()) {
            continue;
        }
        let Some(slot) = pool.idle() else {
            return Ok(());
        };
        pool.assign(slot, job(&board, &states, index, &config))?;
    }
    Ok(())
}

/// Ready rows start, running rows resume at the stage after their last commit.
fn pending(state: &State) -> bool {
    matches!(*state, State::Ready | State::Running { .. })
}

fn job(board: &Board, states: &[State], index: usize, config: &Arc<Config>) -> Job {
    let row = &board.rows()[index];
    Job {
        row: row.clone(),
        config: Arc::clone(config),
        stack: stack(board, states, row),
        from: match states[index] {
            State::Running { stage } => stage,
            _ => FIRST_STAGE,
        },
    }
}

/// Base is `main`. Every parent landed and unmerged rides in as a merge, so the
/// branch starts with the work it waits on.
fn stack(board: &Board, states: &[State], row: &Row) -> Vec<String> {
    row.after
        .iter()
        .filter(|&&parent| states[parent] == State::Landed)
        .map(|&parent| state::branch(board.rows()[parent].id()))
        .collect()
}
