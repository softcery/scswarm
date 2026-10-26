use std::collections::BTreeMap;
use std::thread;

use crate::board::{self, Track};
use crate::credential;
use crate::error::{Error, Result};
use crate::event::{self, Latch};
use crate::git::{self, Git, Push};
use crate::host::Host;
use crate::lock;
use crate::paths::Project;
use crate::pool::{self, Pool, Queue};
use crate::state::{self, Snapshot, State};
use crate::work::Job;

/// Fetch, read every track, spawn a worker per idle slot and queued row, sleep.
/// Until killed. Bare repo lock keeps it one per project.
pub fn serve(repo: &Git, project: &Project) -> Result<()> {
    let _sole = lock::take(repo.root())?.ok_or_else(|| {
        Error::Failed(format!(
            "bare repo {} is held: another `scswarm serve {project}` runs",
            repo.root().display()
        ))
    })?;
    Server::open(repo, project)?.run()
}

struct Server<'a> {
    repo: &'a Git,
    project: &'a Project,
    pool: Pool,
    host: Host,
    /// Remote URL last written to the bare repo.
    origin: Option<String>,
    /// Track tip and base last reported, by track name.
    tips: BTreeMap<String, (String, String)>,
    /// Kept refs at the last mirror. `None`: a restart mirrors once.
    mirrored: Option<BTreeMap<String, String>>,
    /// One latch per fault kind.
    skipped: Latch,
    offline: Latch,
    stale: Latch,
}

impl<'a> Server<'a> {
    fn open(repo: &'a Git, project: &'a Project) -> Result<Self> {
        Ok(Self {
            repo,
            project,
            pool: Pool::open(repo, project)?,
            host: Host::load(project)?,
            origin: None,
            tips: BTreeMap::new(),
            mirrored: None,
            skipped: Latch::default(),
            offline: Latch::default(),
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
        let unfetched = self.sync().err();
        let unmirrored = self.mirror().err();
        let fetched = unfetched.is_none();
        let faults: Vec<Error> = [unfetched, unmirrored].into_iter().flatten().collect();
        self.offline.report(&faults, event::skip);
        if !fetched {
            return Ok(());
        }
        let (snapshots, faults) = self.read()?;
        self.skipped.report(&faults, event::skip);
        self.report(&snapshots);
        let idle = self.pool.converge(self.host.workers)?;
        let queues: Vec<Queue> = snapshots.iter().map(queue).collect();
        for ((at, row), slot) in pool::schedule(&queues, idle.len()).into_iter().zip(idle) {
            let snapshot = &snapshots[at];
            let track = &snapshot.track;
            self.pool.assign(Job {
                slot,
                track: track.name.clone(),
                id: snapshot.board.rows()[row].id.clone(),
                track_tip: track.tip.clone(),
                base: snapshot.base.clone(),
                env: self.host.env(&track.name).to_vec(),
                mounts: self.host.mounts.clone(),
                devices: self.host.devices.clone(),
            })?;
        }
        Ok(())
    }

    /// Write origin on change, fetch, heal hand-edited repo with one retry. A
    /// refusal costs one cycle, not the loop.
    fn sync(&mut self) -> Result<()> {
        if self.origin.as_ref() != Some(&self.host.remote) {
            self.repo.set_origin(&self.host.remote)?;
            self.origin = Some(self.host.remote.clone());
        }
        if self.repo.fetch().is_err() {
            self.repo.set_origin(&self.host.remote)?;
            self.repo.fetch()?;
        }
        Ok(())
    }

    /// Kept refs to the remote, when changed.
    fn mirror(&mut self) -> Result<()> {
        let kept = self.repo.refs(git::KEPT)?;
        if self.mirrored.as_ref() == Some(&kept) {
            return Ok(());
        }
        self.repo.mirror_kept()?;
        event::mirror(kept.len());
        self.mirrored = Some(kept);
        Ok(())
    }

    /// Every track that passes its checks, with rows and states, plus the fault of
    /// each track skipped. Git failing stays fatal.
    fn read(&self) -> Result<(Vec<Snapshot>, Vec<Error>)> {
        let scan = board::scan(self.repo, &self.host.tracks)?;
        let mut faults = scan.faults;
        let mut snapshots = Vec::new();
        let held = |key: &state::Key| self.pool.holds(key);
        for track in scan.tracks {
            if let Err(why) = credential::check(self.host.env(&track.name)) {
                faults.push(track.invalid(&why.to_string()));
                continue;
            }
            match self.open_to(&track) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(why @ Error::Invalid(_)) => {
                    faults.push(why);
                    continue;
                }
                Err(why) => return Err(why),
            }
            match state::snapshot(self.repo, track, &held) {
                Ok(snapshot) => snapshots.push(snapshot),
                Err(why @ Error::Invalid(_)) => faults.push(why),
                Err(why) => return Err(why),
            }
        }
        Ok((snapshots, faults))
    }

    /// True once `to` stands: made here at its start and pushed, when absent.
    /// `false`: another push made it first, the next fetch reads it.
    fn open_to(&self, track: &Track) -> Result<bool> {
        if self
            .repo
            .commit_of(&git::remote_branch(&track.to))?
            .is_some()
        {
            return Ok(true);
        }
        let start = track.start(self.repo)?;
        match self.repo.push(&start, &track.to)? {
            Push::Done => {
                event::open(&track.name, &track.to, &start);
                Ok(true)
            }
            Push::Behind => Ok(false),
            Push::Declined(why) => Err(track.invalid(&format!(
                "field `to` must name a branch the remote lets this host make: {why}"
            ))),
        }
    }

    /// One `fetch` line per track whose tip or base moved.
    fn report(&mut self, snapshots: &[Snapshot]) {
        let mut tips = BTreeMap::new();
        for snapshot in snapshots {
            let name = &snapshot.track.name;
            let now = (snapshot.track.tip.clone(), snapshot.base.clone());
            if self.tips.get(name) != Some(&now) {
                event::fetch(name, &now.0, &now.1);
            }
            tips.insert(name.clone(), now);
        }
        self.tips = tips;
    }

    /// Reread host file each cycle. Unreadable: keep last good copy, editor mid-save
    /// must not stop the loop.
    fn reload(&mut self) {
        match Host::load(self.project) {
            Ok(host) => {
                self.host = host;
                self.stale.report(&[], event::stale);
            }
            Err(why) => self.stale.report(&[why], event::stale),
        }
    }
}

/// Queued rows of one track: new rows, rows cut short, rows whose land commit
/// left `to`.
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
