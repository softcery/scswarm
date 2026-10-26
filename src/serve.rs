use std::collections::BTreeMap;
use std::fs;
use std::panic;
use std::thread;
use std::time::Instant;

use crate::board::{self, Track};
use crate::credential;
use crate::error::{Error, Result};
use crate::event::{self, Latch};
use crate::git::{self, Git, Push};
use crate::host::{Entry, Host};
use crate::init;
use crate::lock;
use crate::paths::{self, Project};
use crate::pool::{self, Pool, Queue};
use crate::state::{self, Snapshot, State};
use crate::work::Job;

/// Refresh every project, spawn a worker per idle slot and queued row, sleep.
/// Until killed. The lock on the swarm root keeps it one per user.
pub fn serve() -> Result<()> {
    let root = paths::swarm()?;
    fs::create_dir_all(&root)
        .map_err(|why| Error::Failed(format!("directory {}: {why}", root.display())))?;
    let _sole = lock::take(&root)?.ok_or_else(|| {
        Error::Failed(format!(
            "swarm {} is held: another `scswarm serve` runs",
            root.display()
        ))
    })?;
    Server::open()?.run()
}

struct Server {
    pool: Pool,
    host: Host,
    /// One per project of the host file. A project dropped from the file drops
    /// its feed; its rows in flight run on.
    feeds: BTreeMap<Project, Feed>,
    stale: Latch,
}

/// What `serve` knows of one project between cycles. A restart rebuilds it.
#[derive(Default)]
struct Feed {
    /// `None` until the bare repo opens.
    repo: Option<Git>,
    /// Remote URL last written to the bare repo.
    origin: Option<String>,
    fetched: Option<Instant>,
    /// Kept refs at the last mirror. `None`: a restart mirrors once.
    mirrored: Option<BTreeMap<String, String>>,
    /// Track tip and base last reported, by track name.
    tips: BTreeMap<String, (String, String)>,
    /// Faults of the project as a whole, then of single tracks.
    offline: Latch,
    skipped: Latch,
}

impl Server {
    fn open() -> Result<Self> {
        Ok(Self {
            pool: Pool::open()?,
            host: Host::load()?,
            feeds: BTreeMap::new(),
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
        self.pool.reap()?;
        let projects = &self.host.projects;
        self.feeds
            .retain(|project, _| projects.contains_key(project));
        for project in projects.keys() {
            self.feeds.entry(project.clone()).or_default();
        }
        let snapshots = self.refresh();
        let idle = self.pool.converge(self.host.workers)?;
        let queues: Vec<Queue> = snapshots
            .iter()
            .enumerate()
            .flat_map(|(at, (_, project))| project.iter().map(move |one| queue(at, one)))
            .collect();
        let flat: Vec<(&Project, &Snapshot)> = snapshots
            .iter()
            .flat_map(|(project, held)| held.iter().map(move |one| (project, one)))
            .collect();
        for ((at, row), slot) in pool::schedule(&queues, idle.len()).into_iter().zip(idle) {
            let (project, snapshot) = flat[at];
            let entry = self.host.entry(project)?;
            let track = &snapshot.track;
            self.pool.assign(
                project,
                Job {
                    slot,
                    track: track.name.clone(),
                    id: snapshot.board.rows()[row].id.clone(),
                    track_tip: track.tip.clone(),
                    base: snapshot.base.clone(),
                    env: entry.env(&track.name).to_vec(),
                    mounts: entry.mounts.clone(),
                    devices: self.host.devices.clone(),
                },
            )?;
        }
        Ok(())
    }

    /// Every project at once, one thread each: a slow or dead remote holds up
    /// none of the others. Snapshots by project, in host file order.
    fn refresh(&mut self) -> Vec<(Project, Vec<Snapshot>)> {
        let (host, pool) = (&self.host, &self.pool);
        thread::scope(|scope| {
            let running: Vec<_> = self
                .feeds
                .iter_mut()
                .filter_map(|(project, feed)| {
                    let entry = host.projects.get(project)?;
                    Some((
                        project,
                        scope.spawn(move || feed.refresh(project, entry, pool)),
                    ))
                })
                .collect();
            running
                .into_iter()
                .map(|(project, thread)| {
                    let snapshots = thread
                        .join()
                        .unwrap_or_else(|panicked| panic::resume_unwind(panicked));
                    (project.clone(), snapshots)
                })
                .collect()
        })
    }

    /// Reread host file each cycle. Unreadable: keep last good copy, editor mid-save
    /// must not stop the loop.
    fn reload(&mut self) {
        match Host::load() {
            Ok(host) => {
                self.host = host;
                self.stale.report(&[], event::stale);
            }
            Err(why) => self.stale.report(&[why], event::stale),
        }
    }
}

impl Feed {
    /// Open, fetch when due, mirror on change, read every track. A fault of the
    /// project skips it this cycle with no rows; the others run.
    fn refresh(&mut self, project: &Project, entry: &Entry, pool: &Pool) -> Vec<Snapshot> {
        let report = |why: &Error| event::skip(project, why);
        match self.sync(project, entry) {
            Ok(()) => self.offline.report(&[], report),
            Err(why) => {
                self.offline.report(&[why], report);
                return Vec::new();
            }
        }
        let Some(repo) = &self.repo else {
            return Vec::new();
        };
        let held = |key: &state::Key| pool.holds(project, key);
        match read(repo, project, entry, &held) {
            Ok((snapshots, faults)) => {
                self.skipped.report(&faults, report);
                self.report(project, &snapshots);
                snapshots
            }
            Err(why) => {
                self.offline.report(&[why], report);
                Vec::new()
            }
        }
    }

    /// Bare repo made on first sight, origin written on change, fetch once due
    /// with one retry that heals a hand-edited repo, kept refs mirrored on change.
    fn sync(&mut self, project: &Project, entry: &Entry) -> Result<()> {
        let repo = match self.repo.take() {
            Some(repo) => repo,
            None => {
                let (repo, made) = init::open_or_make(project, &entry.remote)?;
                if made {
                    self.origin = Some(entry.remote.clone());
                    self.fetched = Some(Instant::now());
                }
                repo
            }
        };
        let repo = self.repo.insert(repo);
        if self.origin.as_ref() != Some(&entry.remote) {
            repo.set_origin(&entry.remote)?;
            self.origin = Some(entry.remote.clone());
        }
        if self
            .fetched
            .is_none_or(|last| last.elapsed() >= entry.fetch)
        {
            if repo.fetch().is_err() {
                repo.set_origin(&entry.remote)?;
                repo.fetch()?;
            }
            self.fetched = Some(Instant::now());
        }
        let kept = repo.refs(git::KEPT)?;
        if self.mirrored.as_ref() != Some(&kept) {
            repo.mirror_kept()?;
            event::mirror(project, kept.len());
            self.mirrored = Some(kept);
        }
        Ok(())
    }

    /// One `fetch` line per track whose tip or base moved.
    fn report(&mut self, project: &Project, snapshots: &[Snapshot]) {
        let mut tips = BTreeMap::new();
        for snapshot in snapshots {
            let name = &snapshot.track.name;
            let now = (snapshot.track.tip.clone(), snapshot.base.clone());
            if self.tips.get(name) != Some(&now) {
                event::fetch(project, name, &now.0, &now.1);
            }
            tips.insert(name.clone(), now);
        }
        self.tips = tips;
    }
}

/// Every track that passes its checks, with rows and states, plus the fault of
/// each track skipped. Git failing faults the project.
fn read(
    repo: &Git,
    project: &Project,
    entry: &Entry,
    held: &dyn Fn(&state::Key) -> Result<bool>,
) -> Result<(Vec<Snapshot>, Vec<Error>)> {
    let scan = board::scan(repo, &entry.tracks)?;
    let mut faults = scan.faults;
    let mut snapshots = Vec::new();
    for track in scan.tracks {
        if let Err(why) = credential::check(entry.env(&track.name)) {
            faults.push(track.invalid(&why.to_string()));
            continue;
        }
        match open_to(repo, project, &track) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(why @ Error::Invalid(_)) => {
                faults.push(why);
                continue;
            }
            Err(why) => return Err(why),
        }
        match state::snapshot(repo, track, held) {
            Ok(snapshot) => snapshots.push(snapshot),
            Err(why @ Error::Invalid(_)) => faults.push(why),
            Err(why) => return Err(why),
        }
    }
    Ok((snapshots, faults))
}

/// True once `to` stands: made here at its start and pushed, when absent.
/// `false`: another push made it first, the next fetch reads it.
fn open_to(repo: &Git, project: &Project, track: &Track) -> Result<bool> {
    if repo.commit_of(&git::remote_branch(&track.to))?.is_some() {
        return Ok(true);
    }
    let start = track.start(repo)?;
    match repo.push(&start, &track.to)? {
        Push::Done => {
            event::open(project, &track.name, &track.to, &start);
            Ok(true)
        }
        Push::Behind => Ok(false),
        Push::Declined(why) => Err(track.invalid(&format!(
            "field `to` must name a branch the remote lets this host make: {why}"
        ))),
    }
}

/// Queued rows of one track of project number `project`: new rows, rows cut
/// short, rows whose land commit left `to`.
fn queue(project: usize, snapshot: &Snapshot) -> Queue {
    let rows = |state: State| {
        snapshot
            .states
            .iter()
            .enumerate()
            .filter(move |(_, held)| **held == state)
            .map(|(row, _)| row)
    };
    Queue {
        project,
        priority: snapshot.track.priority,
        running: rows(State::Running).count(),
        rows: rows(State::Queued).collect(),
    }
}
