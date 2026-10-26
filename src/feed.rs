use std::collections::BTreeMap;
use std::panic;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::board::{self, Track};
use crate::credential;
use crate::error::{Error, Result};
use crate::event::{self, Latch};
use crate::git::{self, Git, Push};
use crate::host::{self, Entry};
use crate::paths::{self, Project};
use crate::repo;

/// Remote side of one project, as its feed last left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Bare repo not made yet.
    Opening,
    /// Last sync went well: rows of the project can run.
    Up,
    /// Last sync failed: no row until one goes well.
    Down,
}

/// Thread that makes every network call of one project: open, fetch, mirror, the
/// first push of `to`. Drop ends it after its current call.
pub struct Feed {
    entry: Entry,
    tick: Duration,
    orders: Sender<(Entry, Duration)>,
    health: Arc<Mutex<Health>>,
    thread: Option<JoinHandle<()>>,
}

/// Rest after faults in a row: `first`, doubling, up to `host::REST_MAX`.
pub struct Backoff {
    until: Instant,
    wait: Duration,
}

impl Feed {
    pub fn start(project: &Project, entry: &Entry, tick: Duration) -> Result<Self> {
        let (orders, inbox) = mpsc::channel();
        let health = Arc::new(Mutex::new(Health::Opening));
        let feeder = Feeder {
            project: project.clone(),
            entry: entry.clone(),
            tick,
            inbox,
            health: Arc::clone(&health),
            opened: false,
            origin: false,
            fetched: None,
            mirrored: None,
            rest: None,
            offline: Latch::default(),
            opening: Latch::default(),
        };
        let thread = thread::Builder::new()
            .name(format!("feed {project}"))
            .spawn(move || feeder.run())
            .map_err(|why| {
                Error::Failed(format!(
                    "feed thread of project `{project}` must start: {why}"
                ))
            })?;
        Ok(Self {
            entry: entry.clone(),
            tick,
            orders,
            health,
            thread: Some(thread),
        })
    }

    /// Pass a changed entry or tick on. The thread reads it at its next wake.
    pub fn update(&mut self, entry: &Entry, tick: Duration) {
        if &self.entry == entry && self.tick == tick {
            return;
        }
        self.entry = entry.clone();
        self.tick = tick;
        // Send fails once the thread ended alone, by panic: `health` raises it.
        let _ = self.orders.send((entry.clone(), tick));
    }

    /// End the feed: its thread stops after its current call. The thread comes
    /// back, so no second feed of the project starts while it runs.
    pub fn retire(mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }

    /// A feed thread that panicked panics `serve`: a bug, no fault.
    pub fn health(&mut self) -> Health {
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(Err(panicked)) = self.thread.take().map(JoinHandle::join) {
                panic::resume_unwind(panicked);
            }
        }
        *self.health.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Backoff {
    pub fn after(last: Option<&Backoff>, first: Duration) -> Self {
        let cap = host::REST_MAX.max(first);
        let wait = last.map_or(first, |last| (last.wait * 2).min(cap));
        Self {
            until: Instant::now() + wait,
            wait,
        }
    }

    pub fn holds(&self) -> bool {
        Instant::now() < self.until
    }
}

/// Thread side of a feed. A restart rebuilds it.
struct Feeder {
    project: Project,
    entry: Entry,
    tick: Duration,
    inbox: Receiver<(Entry, Duration)>,
    health: Arc<Mutex<Health>>,
    opened: bool,
    /// `origin` checked against `entry.remote` and written.
    origin: bool,
    fetched: Option<Instant>,
    /// Kept refs at the last mirror. `None`: a start mirrors once.
    mirrored: Option<BTreeMap<String, String>>,
    rest: Option<Backoff>,
    /// Faults of the project, then of tracks whose `to` failed to open.
    offline: Latch,
    opening: Latch,
}

impl Feeder {
    /// Sync unless at rest, then wait one tick or a new entry. Ends once `serve`
    /// drops the feed.
    fn run(mut self) {
        loop {
            if !self.rest.as_ref().is_some_and(Backoff::holds) {
                let health = match self.sync() {
                    Ok(()) => {
                        self.rest = None;
                        let project = &self.project;
                        self.offline
                            .report(&[], |why: &Error| event::skip(project, why));
                        Health::Up
                    }
                    Err(why) => {
                        self.fail(why);
                        Health::Down
                    }
                };
                *self.health.lock().unwrap_or_else(PoisonError::into_inner) = health;
            }
            match self.inbox.recv_timeout(self.tick) {
                Ok((entry, tick)) => {
                    self.origin &= entry.remote == self.entry.remote;
                    self.entry = entry;
                    self.tick = tick;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Report once per cause. A git failure rests the feed; an entry the bare repo
    /// refuses retries each tick, with no network call.
    fn fail(&mut self, why: Error) {
        if matches!(why, Error::Failed(_)) {
            self.rest = Some(Backoff::after(self.rest.as_ref(), self.entry.fetch));
        }
        let project = &self.project;
        self.offline
            .report(&[why], |why: &Error| event::skip(project, why));
    }

    /// Bare repo on first sight, `origin` checked, fetch once due, kept refs
    /// mirrored on change, each absent `to` opened after a fetch.
    fn sync(&mut self) -> Result<()> {
        let mut fresh = false;
        let repo = match self.opened {
            true => repo::open(&self.project)?,
            false => {
                let (repo, made) = repo::open_or_make(&self.project, &self.entry.remote)?;
                self.opened = true;
                if made {
                    self.origin = true;
                    self.fetched = Some(Instant::now());
                    fresh = true;
                }
                repo
            }
        };
        if !self.origin {
            self.check_origin(&repo)?;
            repo.set_origin(&self.entry.remote)?;
            self.origin = true;
        }
        if self
            .fetched
            .is_none_or(|last| last.elapsed() >= self.entry.fetch)
        {
            if let Err(why) = repo.fetch() {
                self.origin = false;
                return Err(why);
            }
            self.fetched = Some(Instant::now());
            fresh = true;
        }
        let kept = repo.refs(git::KEPT)?;
        if self.mirrored.as_ref() != Some(&kept) {
            repo.mirror_kept()?;
            event::mirror(&self.project, kept.len());
            self.mirrored = Some(kept);
        }
        if fresh {
            let faults = open_all(&repo, &self.project, &self.entry)?;
            let project = &self.project;
            self.opening
                .report(&faults, |why: &Error| event::skip(project, why));
        }
        Ok(())
    }

    /// A bare repo of another remote keeps its land and fail refs: a mirror to
    /// the new remote would delete that remote's own.
    fn check_origin(&self, repo: &Git) -> Result<()> {
        match repo.origin()? {
            Some(url) if url != self.entry.remote => Err(Error::Invalid(format!(
                "host file field `projects.{}.remote` must match `origin` of bare repo {}: delete the bare repo to move remotes",
                self.project,
                paths::arg(repo.root())?
            ))),
            _ => Ok(()),
        }
    }
}

/// Make and push each absent `to` of a track that passes its credential check.
/// Returns the fault of each track whose `to` cannot open; `serve` reports the rest.
fn open_all(repo: &Git, project: &Project, entry: &Entry) -> Result<Vec<Error>> {
    let mut faults = Vec::new();
    for track in board::scan(repo, &entry.tracks)?.tracks {
        if credential::check(entry.env(&track.name)).is_err() {
            continue;
        }
        match open_to(repo, project, &track) {
            Ok(()) => {}
            Err(why @ Error::Invalid(_)) => faults.push(why),
            Err(why) => return Err(why),
        }
    }
    Ok(faults)
}

/// `to` made at its start and pushed, when absent. Another push first: the next
/// fetch reads it.
fn open_to(repo: &Git, project: &Project, track: &Track) -> Result<()> {
    if repo.commit_of(&git::remote_branch(&track.to))?.is_some() {
        return Ok(());
    }
    let start = track.start(repo)?;
    match repo.push(&start, &track.to)? {
        Push::Done => {
            event::open(project, &track.name, &track.to, &start);
            Ok(())
        }
        Push::Behind => Ok(()),
        Push::Declined(why) => Err(track.invalid(&format!(
            "field `to` must name a branch the remote lets this host make: {why}"
        ))),
    }
}
