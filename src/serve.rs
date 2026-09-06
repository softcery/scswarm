use std::thread;
use std::time::Duration;

use crate::board::Board;
use crate::config::Config;
use crate::error::Result;
use crate::git::{self, Git};
use crate::slot::Slot;
use crate::state::{self, State};
use crate::{paths, stage};

const POLL: Duration = Duration::from_secs(5);
const SLOT: usize = 0;

/// Fetch, derive, run the first open row to its last stage, sleep. Until killed.
pub fn serve(git: &Git) -> Result<()> {
    let project = paths::project(git.root())?;
    let slot = Slot::open(git, paths::slot(&project, SLOT)?, git::MAIN)?;
    loop {
        cycle(git, &slot, &project)?;
        thread::sleep(POLL);
    }
}

fn cycle(git: &Git, slot: &Slot, project: &str) -> Result<()> {
    git.fetch()?;
    let config = Config::from_main(git)?;
    let board = Board::from_main(git, &config)?;
    let total = config.stages.len();
    let states = state::derive(&board, total, git)?;
    let Some(index) = states.iter().position(pending) else {
        return Ok(());
    };

    let row = &board.rows()[index];
    slot.checkout(&state::branch(row.id()), git::MAIN)?;
    let done = match states[index] {
        State::Running { stage } => stage - 1,
        _ => 0,
    };
    for number in (done + 1)..=total {
        let log = paths::transcript(project, row.id(), number)?;
        stage::run(slot, &config, row, number, &log)?;
    }
    Ok(())
}

/// Ready rows start, running rows resume at the stage after their last commit.
fn pending(state: &State) -> bool {
    matches!(*state, State::Ready | State::Running { .. })
}
