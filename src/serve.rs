use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::Result;
use crate::event;
use crate::git::{self, Git};
use crate::host::{Host, Remote, Slots};
use crate::paths::Project;
use crate::pool::{Job, Pool};
use crate::slot;
use crate::state::{self, State};

const POLL: Duration = Duration::from_secs(5);

/// Fetch, derive, fill idle slots with pending rows in board order, sleep. Until killed.
pub fn serve(repo: &Git, project: &Project) -> Result<()> {
    Server::open(repo, project)?.run()
}

struct Server<'a> {
    repo: &'a Git,
    project: &'a Project,
    pool: Pool,
    host: Host,
    /// Slot set the pool opened with. Later edit of it is drift.
    slots: Slots,
    /// Remote last written to the bare repo.
    origin: Option<Remote>,
    /// Trunk tip last reported.
    head: String,
    /// Fetch fault last reported.
    skipped: Option<String>,
}

impl<'a> Server<'a> {
    /// Config precedes slot check: image name lives on trunk.
    fn open(repo: &'a Git, project: &'a Project) -> Result<Self> {
        let host = Host::load(project)?;
        let mut origin = None;
        sync(repo, &host.remote, &mut origin)?;
        let config = Config::from_trunk(repo)?;
        let stamp = host.slots.stamp(&config.image_name(repo, project)?)?;
        slot::verify(project, &host.slots, &stamp)?;
        let pool = Pool::open(repo, project, &host.slots)?;
        let slots = host.slots.clone();
        Ok(Self {
            repo,
            project,
            pool,
            host,
            slots,
            origin,
            head: String::new(),
            skipped: None,
        })
    }

    fn run(mut self) -> Result<()> {
        loop {
            self.cycle()?;
            thread::sleep(POLL);
            self.reload()?;
        }
    }

    fn cycle(&mut self) -> Result<()> {
        self.pool.collect()?;
        let Some(sha) = self.trunk() else {
            return Ok(());
        };
        if sha != self.head {
            event::fetch(&sha);
            self.head.clone_from(&sha);
        }
        let config = Arc::new(Config::from_trunk(self.repo)?);
        let board = Board::from_trunk(self.repo, &config)?;
        let states = state::derive(&board, self.repo)?;
        for (index, row) in board.rows().iter().enumerate() {
            if !pending(&states[index]) || self.pool.holds(&row.id) {
                continue;
            }
            let Some(slot) = self.pool.idle() else {
                return Ok(());
            };
            self.pool
                .assign(slot, job(&board, &states, index, &config, &sha))?;
        }
        Ok(())
    }

    /// Trunk tip after fetch. `None` when git refuses: absent branch or lost checkout
    /// costs one cycle, not the loop. One report per cause.
    fn trunk(&mut self) -> Option<String> {
        match sync(self.repo, &self.host.remote, &mut self.origin) {
            Ok(sha) => {
                self.skipped = None;
                Some(sha)
            }
            Err(why) => {
                let cause = why.to_string();
                if self.skipped.as_deref() != Some(&cause) {
                    event::skip(&why);
                    self.skipped = Some(cause);
                }
                None
            }
        }
    }

    /// Reread host file each cycle. Unreadable: keep last good copy, editor mid-save
    /// must not stop the loop. Changed slot set: stop, `init` writes that set.
    fn reload(&mut self) -> Result<()> {
        let host = match Host::load(self.project) {
            Ok(host) => host,
            Err(why) => {
                event::stale(&why);
                return Ok(());
            }
        };
        if let Some(field) = self.slots.drift(&host.slots) {
            return Err(slot::drifted(self.project, field));
        }
        self.host = host;
        Ok(())
    }
}

/// Write origin on change, fetch, heal hand-edited repo with one retry.
fn sync(repo: &Git, remote: &Remote, origin: &mut Option<Remote>) -> Result<String> {
    if origin.as_ref() != Some(remote) {
        repo.set_origin(&remote.path, &remote.trunk)?;
        *origin = Some(remote.clone());
    }
    if repo.fetch().is_err() {
        repo.set_origin(&remote.path, &remote.trunk)?;
        repo.fetch()?;
    }
    repo.tip(git::TRUNK)
}

/// Ready rows start. A running row holds a branch no land ref names, so its
/// session was cut short: it replays from base.
fn pending(state: &State) -> bool {
    matches!(*state, State::Ready | State::Running)
}

fn job(board: &Board, states: &[State], index: usize, config: &Arc<Config>, base: &str) -> Job {
    let row = &board.rows()[index];
    Job {
        row: row.clone(),
        config: Arc::clone(config),
        stack: stack(board, states, row),
        base: base.to_string(),
    }
}

/// Base is trunk. Each landed unmerged parent rides in as merge, so branch starts
/// with work it waits on.
fn stack(board: &Board, states: &[State], row: &Row) -> Vec<String> {
    row.after
        .iter()
        .filter(|&&parent| states[parent] == State::Landed)
        .map(|&parent| state::branch(&board.rows()[parent].id))
        .collect()
}
