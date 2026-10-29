use std::collections::BTreeMap;
use std::fs;
use std::panic;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::board;
use crate::credential;
use crate::error::{Error, Result};
use crate::event::{self, Latch, Seen};
use crate::feed::{Backoff, Feed, Health};
use crate::git::{self, Git};
use crate::host::{self, Entry, Host};
use crate::lock;
use crate::paths::{self, Project};
use crate::pool::{self, Demand, Pool, Queue};
use crate::repo;
use crate::state::{self, Address, Snapshot, State};
use crate::work::Job;

/// Read every project from local refs, spawn a worker per idle slot and queued
/// row, sleep. Until killed. The lock on the swarm root keeps it one per user.
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
    /// its lane and feed; its rows in flight run on.
    lanes: BTreeMap<Project, Lane>,
    /// Feed threads of dropped lanes, until each ends its last call.
    retired: BTreeMap<Project, JoinHandle<()>>,
    /// Rows assigned in this `serve` life: orders projects by turn.
    turn: u64,
    stale: Latch,
}

/// What `serve` holds for one project between cycles. A restart rebuilds it.
struct Lane {
    feed: Feed,
    /// Turn of the last row assigned, 0 for none.
    served: u64,
    /// Rest after workers that failed in a row. A worker that ends well ends it.
    rest: Option<Backoff>,
    /// Worker faults since a worker ended well, then read faults, then track faults.
    workers: Seen,
    offline: Latch,
    skipped: Latch,
    /// Track tip and base last reported, by track name.
    tips: BTreeMap<String, (String, String)>,
}

impl Server {
    fn open() -> Result<Self> {
        Ok(Self {
            pool: Pool::open()?,
            host: Host::load()?,
            lanes: BTreeMap::new(),
            retired: BTreeMap::new(),
            turn: 0,
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
        self.follow()?;
        for ended in self.pool.reap()? {
            let project = &ended.project;
            match self.lanes.get_mut(project) {
                Some(lane) => lane.ended(project, ended.result),
                None => {
                    if let Err(why) = &ended.result {
                        event::skip(project, why);
                    }
                }
            }
        }
        let snapshots = self.read();
        let idle = self.pool.converge(self.host.workers)?;
        let demands: Vec<Demand> = snapshots
            .iter()
            .map(|(project, held)| Demand {
                served: self.lanes.get(project).map_or(0, |lane| lane.served),
                queues: held.iter().map(queue).collect(),
            })
            .collect();
        for (pick, slot) in pool::schedule(&demands, idle.len()).into_iter().zip(idle) {
            let (project, held) = &snapshots[pick.project];
            let (snapshot, row) = (&held[pick.queue], pick.row);
            let entry = self.host.entry(project)?;
            self.turn += 1;
            if let Some(lane) = self.lanes.get_mut(project) {
                lane.served = self.turn;
            }
            let track = &snapshot.track;
            self.pool.assign(Job {
                project: project.clone(),
                slot,
                tracks: entry.tracks.clone(),
                track: track.name.clone(),
                id: snapshot.board.rows()[row].id.clone(),
                track_tip: track.tip.clone(),
                base: snapshot.base.clone(),
                env: entry.env(&track.name).to_vec(),
                mounts: entry.mounts.clone(),
                devices: self.host.devices.clone(),
            })?;
        }
        Ok(())
    }

    /// One lane per project of the host file: opened on first sight once no
    /// retired feed of it runs, dropped with its entry, its feed told of each change.
    fn follow(&mut self) -> Result<()> {
        let ended: Vec<Project> = self
            .retired
            .iter()
            .filter(|(_, thread)| thread.is_finished())
            .map(|(project, _)| project.clone())
            .collect();
        for project in ended {
            if let Some(Err(panicked)) = self.retired.remove(&project).map(JoinHandle::join) {
                panic::resume_unwind(panicked);
            }
        }
        let (projects, poll) = (&self.host.projects, self.host.poll);
        let gone: Vec<Project> = self
            .lanes
            .keys()
            .filter(|project| !projects.contains_key(*project))
            .cloned()
            .collect();
        for project in gone {
            if let Some(thread) = self
                .lanes
                .remove(&project)
                .and_then(|lane| lane.feed.retire())
            {
                self.retired.insert(project, thread);
            }
        }
        for (project, entry) in projects {
            match self.lanes.get_mut(project) {
                Some(lane) => lane.feed.update(entry, poll),
                None if !self.retired.contains_key(project) => {
                    self.lanes
                        .insert(project.clone(), Lane::open(project, entry, poll)?);
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Every project at once, one thread each, local refs alone. Snapshots by
    /// project, in host file order.
    fn read(&mut self) -> Vec<(Project, Vec<Snapshot>)> {
        let (host, pool) = (&self.host, &self.pool);
        thread::scope(|scope| {
            let running: Vec<_> = self
                .lanes
                .iter_mut()
                .filter_map(|(project, lane)| {
                    let entry = host.projects.get(project)?;
                    Some((
                        project,
                        scope.spawn(move || lane.read(project, entry, pool)),
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

impl Lane {
    fn open(project: &Project, entry: &Entry, tick: Duration) -> Result<Self> {
        Ok(Self {
            feed: Feed::start(project, entry, tick)?,
            served: 0,
            rest: None,
            workers: Seen::default(),
            offline: Latch::default(),
            skipped: Latch::default(),
            tips: BTreeMap::new(),
        })
    }

    /// Exit 0 ends the rest. Exit 1 reports once. Any other end reports once and
    /// rests the project, doubling per fault in a row.
    fn ended(&mut self, project: &Project, result: Result<()>) {
        let why = match result {
            Ok(()) => {
                self.rest = None;
                self.workers.clear();
                return;
            }
            Err(why) => why,
        };
        if matches!(why, Error::Failed(_)) {
            self.rest = Some(Backoff::after(self.rest.as_ref(), host::REST_FIRST));
        }
        self.workers
            .once(&why, |why: &Error| event::skip(project, why));
    }

    /// Tracks of a project whose feed is up and whose workers do not rest. A read
    /// fault skips the project this cycle.
    fn read(&mut self, project: &Project, entry: &Entry, pool: &Pool) -> Vec<Snapshot> {
        if self.feed.health() != Health::Up || self.rest.as_ref().is_some_and(Backoff::holds) {
            return Vec::new();
        }
        let held = |key: &state::Key| {
            pool.holds(&Address {
                project: project.clone(),
                key: key.clone(),
            })
        };
        let report = |why: &Error| event::skip(project, why);
        match repo::open(project).and_then(|repo| read(&repo, entry, &held)) {
            Ok((snapshots, faults)) => {
                self.offline.report(&[], report);
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

    /// One `tip` line per track whose tip or base moved.
    fn report(&mut self, project: &Project, snapshots: &[Snapshot]) {
        let mut tips = BTreeMap::new();
        for snapshot in snapshots {
            let name = &snapshot.track.name;
            let now = (snapshot.track.tip.clone(), snapshot.base.clone());
            if self.tips.get(name) != Some(&now) {
                event::tip(project, name, &now.0, &now.1);
            }
            tips.insert(name.clone(), now);
        }
        self.tips = tips;
    }
}

/// Every track that passes its checks and whose `to` stands, with rows and
/// states, plus the fault of each track skipped. Git failing faults the project.
fn read(
    repo: &Git,
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
        if repo.commit_of(&git::remote_branch(&track.to))?.is_none() {
            continue;
        }
        match state::snapshot(repo, track, held) {
            Ok(snapshot) => snapshots.push(snapshot),
            Err(why @ Error::Invalid(_)) => faults.push(why),
            Err(why) => return Err(why),
        }
    }
    Ok((snapshots, faults))
}

/// Queued rows of one track: new rows, rows cut short, rows whose land commit left `to`.
fn queue(snapshot: &Snapshot) -> Queue {
    let rows = |state: State| {
        snapshot
            .states
            .iter()
            .enumerate()
            .filter(move |(_, held)| **held == state)
            .map(|(row, _)| row)
    };
    Queue {
        priority: snapshot.track.priority,
        running: rows(State::Running).count(),
        rows: rows(State::Queued).collect(),
    }
}
