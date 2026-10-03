use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::{env, fs};

use crate::error::{Error, Result};
use crate::event;
use crate::git::Git;
use crate::host::{Slots, Stamp};
use crate::paths::{self, Project};
use crate::work::Job;
use crate::{lock, slot};

/// Workers by slot. Slot or row is taken while own worker runs it, or a worker
/// left by an earlier `serve` holds its lock.
pub struct Pool {
    repo: Git,
    project: Project,
    /// Resolved once: Linux names a replaced binary `<path> (deleted)`.
    exe: PathBuf,
    workers: BTreeMap<usize, Worker>,
    /// Slots attached in this `serve` life.
    attached: BTreeSet<usize>,
}

struct Worker {
    child: Child,
    id: String,
}

impl Pool {
    /// No slot yet: `converge` opens them.
    pub fn open(repo: &Git, project: &Project) -> Result<Self> {
        let exe = env::current_exe().map_err(|why| {
            Error::Failed(format!("path of the running `scswarm` must resolve: {why}"))
        })?;
        Ok(Self {
            repo: Git::at(repo.root().to_path_buf()),
            project: project.clone(),
            exe,
            workers: BTreeMap::new(),
            attached: BTreeSet::new(),
        })
    }

    /// Bring the machine to `slots` and `stamp`, then list idle slots, lowest
    /// first. A busy slot keeps its row and changes on a later cycle, once idle.
    pub fn converge(&mut self, slots: &Slots, stamp: &Stamp) -> Result<Vec<usize>> {
        let held = slot::survey(&self.project)?;
        let mut indices: BTreeSet<usize> = (0..slots.workers).collect();
        indices.extend(held.keys());
        let mut idle = Vec::new();
        for index in indices {
            if self.busy(index)? {
                continue;
            }
            if index >= slots.workers {
                self.remove(index)?;
                continue;
            }
            match held.get(&index).and_then(Option::as_ref) == Some(stamp) {
                true => self.attach(index)?,
                false => self.create(index, slots, stamp)?,
            }
            idle.push(index);
        }
        Ok(idle)
    }

    /// Free each slot whose worker ended. Non-zero exit is a machine fault: fatal.
    pub fn reap(&mut self) -> Result<()> {
        let mut ended = Vec::new();
        for (&index, worker) in &mut self.workers {
            let status = worker
                .child
                .try_wait()
                .map_err(|why| stopped(index, &why.to_string()))?;
            ended.extend(status.map(|status| (index, status)));
        }
        for (index, _) in &ended {
            self.workers.remove(index);
        }
        match ended.into_iter().find(|(_, status)| !status.success()) {
            Some((index, status)) => Err(stopped(index, &format!("worker {status}"))),
            None => Ok(()),
        }
    }

    pub fn holds(&self, id: &str) -> Result<bool> {
        if self.workers.values().any(|worker| worker.id == id) {
            return Ok(true);
        }
        lock::held(&paths::transcripts(&self.project, id)?)
    }

    /// Spawn worker on a slot `converge` listed idle. Busy until `reap` sees it end.
    /// Row lock taken here, inherited as stdin: held from spawn, with no gap.
    pub fn assign(&mut self, job: Job) -> Result<()> {
        let logs = paths::transcripts(&self.project, &job.id)?;
        fs::create_dir_all(&logs)
            .map_err(|why| Error::Failed(format!("directory {}: {why}", logs.display())))?;
        let row = lock::take(&logs)?.ok_or_else(|| stopped(job.slot, "row lock held"))?;
        event::assign(&job.id, job.slot);
        let child = Command::new(&self.exe)
            .args(job.argv(&self.project))
            .stdin(row.into_stdio())
            .spawn()
            .map_err(|why| stopped(job.slot, &format!("worker must start: {why}")))?;
        self.workers.insert(job.slot, Worker { child, id: job.id });
        Ok(())
    }

    fn busy(&self, index: usize) -> Result<bool> {
        if self.workers.contains_key(&index) {
            return Ok(true);
        }
        lock::held(&paths::slot(&self.project, index)?)
    }

    /// Fitting idle slot: attach once per `serve` life.
    fn attach(&mut self, index: usize) -> Result<()> {
        if !self.attached.contains(&index) {
            slot::attach(&self.project, index)?;
            event::attach(index);
            self.attached.insert(index);
        }
        Ok(())
    }

    fn create(&mut self, index: usize, slots: &Slots, stamp: &Stamp) -> Result<()> {
        slot::create(&self.repo, &self.project, index, slots, stamp)?;
        event::create(index);
        self.attached.insert(index);
        Ok(())
    }

    fn remove(&mut self, index: usize) -> Result<()> {
        event::remove(index);
        self.attached.remove(&index);
        slot::remove(&self.repo, &self.project, index)
    }
}

fn stopped(index: usize, why: &str) -> Error {
    Error::Failed(format!(
        "slot {index} stopped: {why}: restart `scswarm serve`"
    ))
}
