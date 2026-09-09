use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::Result;
use crate::event;
use crate::git::{self, Git};
use crate::host::Host;
use crate::pool::{Job, Pool};
use crate::state::{self, State};

const POLL: Duration = Duration::from_secs(5);
const FIRST_STAGE: usize = 1;

/// Fetch, derive, fill idle slots with pending rows in board order, sleep. Until killed.
pub fn serve(repo: &Git, project: &str) -> Result<()> {
    let mut host = Host::load(project)?;
    let mut pool = Pool::open(repo, project, &host)?;
    let mut head = String::new();
    loop {
        cycle(repo, &mut pool, &host, &mut head)?;
        thread::sleep(POLL);
        host = reload(project, host);
    }
}

/// Report a fetch only when the trunk moved. Head starts empty, so cycle 1 reports.
fn cycle(repo: &Git, pool: &mut Pool, host: &Host, head: &mut String) -> Result<()> {
    pool.collect()?;
    let Some(sha) = trunk(repo, host) else {
        return Ok(());
    };
    if sha != *head {
        event::fetch(&sha);
        head.clone_from(&sha);
    }
    let config = Arc::new(Config::from_trunk(repo)?);
    let board = Board::from_trunk(repo, &config)?;
    let states = state::derive(&board, config.stages.len(), repo)?;
    for (index, row) in board.rows().iter().enumerate() {
        if !pending(&states[index]) || pool.holds(row.id()) {
            continue;
        }
        let Some(slot) = pool.idle() else {
            return Ok(());
        };
        pool.assign(slot, job(&board, &states, index, &config, &sha))?;
    }
    Ok(())
}

/// Host file rules the wiring, so every cycle reads it again. A bad file keeps the
/// last good copy: an editor mid-save must not stop the loop.
fn reload(project: &str, last: Host) -> Host {
    match Host::load(project) {
        Ok(host) => host,
        Err(why) => {
            event::stale(&why);
            last
        }
    }
}

/// Trunk tip after a fetch. `None` when git refuses: an absent branch or a lost
/// checkout costs one cycle, not the loop.
fn trunk(repo: &Git, host: &Host) -> Option<String> {
    match sync(repo, host) {
        Ok(sha) => Some(sha),
        Err(why) => {
            event::skip(&why);
            None
        }
    }
}

fn sync(repo: &Git, host: &Host) -> Result<String> {
    repo.set_origin(&host.checkout()?, &host.trunk)?;
    repo.fetch()?;
    repo.tip(git::TRUNK)
}

/// Ready rows start, running rows resume at the stage after their last commit.
fn pending(state: &State) -> bool {
    matches!(*state, State::Ready | State::Running { .. })
}

fn job(board: &Board, states: &[State], index: usize, config: &Arc<Config>, base: &str) -> Job {
    let row = &board.rows()[index];
    Job {
        row: row.clone(),
        config: Arc::clone(config),
        stack: stack(board, states, row),
        from: match states[index] {
            State::Running { stage } => stage,
            _ => FIRST_STAGE,
        },
        base: base.to_string(),
    }
}

/// Base is the trunk. Every parent landed and unmerged rides in as a merge, so the
/// branch starts with the work it waits on.
fn stack(board: &Board, states: &[State], row: &Row) -> Vec<String> {
    row.after
        .iter()
        .filter(|&&parent| states[parent] == State::Landed)
        .map(|&parent| state::branch(board.rows()[parent].id()))
        .collect()
}
