use std::sync::Arc;
use std::thread;

use crate::board::{Board, Row};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::event::{self, Latch};
use crate::git::{self, Git};
use crate::host::{Host, Remote, Stamp};
use crate::paths::Project;
use crate::podman;
use crate::pool::{Job, Pool};
use crate::state::{self, State};

/// Fetch, converge slots, fill idle slots with pending rows in board order, sleep.
/// Until killed.
pub fn serve(repo: &Git, project: &Project) -> Result<()> {
    Server::open(repo, project)?.run()
}

struct Server<'a> {
    repo: &'a Git,
    project: &'a Project,
    pool: Pool,
    host: Host,
    /// Remote last written to the bare repo.
    origin: Option<Remote>,
    /// Trunk tip last reported.
    head: String,
    skipped: Latch,
    /// Host file fault, kept apart: it repeats while cycles pass.
    stale: Latch,
}

impl<'a> Server<'a> {
    fn open(repo: &'a Git, project: &'a Project) -> Result<Self> {
        Ok(Self {
            repo,
            project,
            pool: Pool::open(repo, project),
            host: Host::load(project)?,
            origin: None,
            head: String::new(),
            skipped: Latch::default(),
            stale: Latch::default(),
        })
    }

    fn run(mut self) -> Result<()> {
        loop {
            self.cycle()?;
            thread::sleep(self.host.poll);
            self.reload();
        }
    }

    fn cycle(&mut self) -> Result<()> {
        self.pool.collect()?;
        let Some(sha) = self.trunk() else {
            return Ok(());
        };
        let Some((config, board)) = self.inputs()? else {
            return Ok(());
        };
        let Some(stamp) = self.stamp(&config)? else {
            return Ok(());
        };
        self.skipped.clear();
        let mut idle = self.pool.converge(&self.host.slots, &stamp)?.into_iter();
        let states = state::derive(&board, self.repo)?;
        for (index, row) in board.rows().iter().enumerate() {
            if !pending(&states[index]) || self.pool.holds(&row.id) {
                continue;
            }
            let Some(slot) = idle.next() else {
                return Ok(());
            };
            let env = &self.host.slots.env;
            let job = job(&board, &states, index, &config, &sha, env);
            self.pool.assign(slot, job)?;
        }
        Ok(())
    }

    /// Trunk tip after fetch. `None` when git refuses: absent branch or lost checkout
    /// costs one cycle, not the loop.
    fn trunk(&mut self) -> Option<String> {
        let fetched = sync(self.repo, &self.host.remote, &mut self.origin);
        match fetched {
            Ok(sha) => {
                if sha != self.head {
                    event::fetch(&sha);
                    self.head.clone_from(&sha);
                }
                Some(sha)
            }
            Err(why) => {
                self.skipped.report(&why, event::skip);
                None
            }
        }
    }

    /// Config and board at the trunk tip. `None` when either is invalid: a bad push
    /// skips cycles until fixed, rows in flight run on. Git failing stays fatal.
    fn inputs(&mut self) -> Result<Option<(Arc<Config>, Board)>> {
        let read = Config::from_trunk(self.repo).and_then(|config| {
            let board = Board::from_trunk(self.repo, &config)?;
            Ok((Arc::new(config), board))
        });
        match read {
            Ok(read) => Ok(Some(read)),
            Err(why @ Error::Invalid(_)) => {
                self.skipped.report(&why, event::skip);
                Ok(None)
            }
            Err(why) => Err(why),
        }
    }

    /// Stamp every slot must carry. `None` when the trunk names an image the host
    /// lacks: no slot runs work on an image no one declared.
    fn stamp(&mut self, config: &Config) -> Result<Option<Stamp>> {
        let image = config.image_name(self.repo, self.project)?;
        match podman::image_id(&image)? {
            Some(id) => self.host.slots.stamp(&id).map(Some),
            None => {
                let why = Error::Failed(format!(
                    "image {image} is absent: run `scswarm init {}`",
                    self.project
                ));
                self.skipped.report(&why, event::skip);
                Ok(None)
            }
        }
    }

    /// Reread host file each cycle. Unreadable: keep last good copy, editor mid-save
    /// must not stop the loop.
    fn reload(&mut self) {
        match Host::load(self.project) {
            Ok(host) => {
                self.host = host;
                self.stale.clear();
            }
            Err(why) => self.stale.report(&why, event::stale),
        }
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

fn job(
    board: &Board,
    states: &[State],
    index: usize,
    config: &Arc<Config>,
    base: &str,
    env: &[String],
) -> Job {
    let row = &board.rows()[index];
    Job {
        row: row.clone(),
        config: Arc::clone(config),
        stack: stack(board, states, row),
        env: env.to_vec(),
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
