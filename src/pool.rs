use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::board::Row;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::event;
use crate::git::{self, Git};
use crate::host::{Slots, Stamp};
use crate::paths::Project;
use crate::slot::{self, Slot};
use crate::state;
use crate::step::Outcome;
use crate::transcript::Transcript;
use crate::{paths, script};

/// One task on one slot: row and the branches its base needs.
pub struct Job {
    pub row: Row,
    pub config: Arc<Config>,
    pub stack: Vec<String>,
    /// Host variables each exec gets, read when the job starts.
    pub env: Vec<String>,
    /// Trunk tip this attempt runs against. Failure marks it, so the row waits for a new tip.
    pub base: String,
}

/// Slot threads keyed by slot number, and what each holds. The assignment rule
/// lives in the caller.
pub struct Pool {
    repo: Git,
    project: Project,
    workers: BTreeMap<usize, Worker>,
    done: Sender<Report>,
    reports: Receiver<Report>,
}

/// Slot that finished a job. `Err`: git or podman failed, the slot stopped.
type Report = (usize, Result<()>);

struct Worker {
    jobs: Sender<Job>,
    thread: JoinHandle<()>,
    /// Row id in flight. `None` while the slot waits for work.
    id: Option<String>,
}

impl Pool {
    /// No slot yet: `converge` opens them.
    pub fn open(repo: &Git, project: &Project) -> Self {
        let (done, reports) = mpsc::channel();
        Self {
            repo: Git::at(repo.root().to_path_buf()),
            project: project.clone(),
            workers: BTreeMap::new(),
            done,
            reports,
        }
    }

    /// Bring the machine to `slots` and `stamp`, then list idle slots, lowest
    /// first. A busy slot keeps its row and changes on a later cycle, once idle.
    pub fn converge(&mut self, slots: &Slots, stamp: &Stamp) -> Result<Vec<usize>> {
        let held = slot::survey(&self.project)?;
        let mut indices: BTreeSet<usize> = (0..slots.workers).collect();
        indices.extend(held.keys());
        indices.extend(self.workers.keys());
        for index in indices {
            if self.busy(index) {
                continue;
            }
            let made = held.get(&index).and_then(Option::as_ref);
            match index < slots.workers && made == Some(stamp) {
                true => self.own(index)?,
                false => self.rebuild(index, slots, stamp)?,
            }
        }
        Ok(self
            .workers
            .iter()
            .filter(|(_, worker)| worker.id.is_none())
            .map(|(&index, _)| index)
            .collect())
    }

    /// Free each slot done since last call. Machine fault or dead thread is
    /// fatal: its slot stays busy for good.
    pub fn collect(&mut self) -> Result<()> {
        for (index, report) in self.reports.try_iter() {
            if let Some(worker) = self.workers.get_mut(&index) {
                worker.id = None;
            }
            report.map_err(|why| stopped(index, &why.to_string()))?;
        }
        match self
            .workers
            .iter()
            .find(|(_, worker)| worker.thread.is_finished())
        {
            Some((&index, _)) => Err(stopped(index, "thread ended")),
            None => Ok(()),
        }
    }

    pub fn holds(&self, id: &str) -> bool {
        self.workers
            .values()
            .any(|worker| worker.id.as_deref() == Some(id))
    }

    /// Hand `job` to a slot `converge` listed idle. The slot counts busy until its
    /// thread reports back.
    pub fn assign(&mut self, index: usize, job: Job) -> Result<()> {
        let worker = self
            .workers
            .get_mut(&index)
            .expect("`converge` must list owned slots only");
        worker.id = Some(job.row.id.clone());
        event::assign(&job.row.id, index);
        worker
            .jobs
            .send(job)
            .map_err(|_| stopped(index, "thread ended"))
    }

    fn busy(&self, index: usize) -> bool {
        self.workers
            .get(&index)
            .is_some_and(|worker| worker.id.is_some())
    }

    /// Fitting slot: give it a thread unless it has one.
    fn own(&mut self, index: usize) -> Result<()> {
        if !self.workers.contains_key(&index) {
            let slot = Slot::attach(&self.project, index)?;
            event::attach(index);
            self.spawn(index, slot);
        }
        Ok(())
    }

    /// Idle slot that differs: below `workers` build again, at or above remove.
    fn rebuild(&mut self, index: usize, slots: &Slots, stamp: &Stamp) -> Result<()> {
        if let Some(worker) = self.workers.remove(&index) {
            worker.stop(index)?;
        }
        if index >= slots.workers {
            event::remove(index);
            return slot::remove(&self.repo, &self.project, index);
        }
        let slot = Slot::create(&self.repo, &self.project, index, slots, stamp)?;
        event::create(index);
        self.spawn(index, slot);
        Ok(())
    }

    fn spawn(&mut self, index: usize, slot: Slot) {
        let (jobs, queue) = mpsc::channel();
        let project = self.project.clone();
        let repo = Git::at(self.repo.root().to_path_buf());
        let done = self.done.clone();
        let thread = thread::spawn(move || work(&slot, &repo, &project, index, &queue, &done));
        self.workers.insert(
            index,
            Worker {
                jobs,
                thread,
                id: None,
            },
        );
    }
}

impl Worker {
    /// Close the queue of an idle slot, wait for its thread to end.
    fn stop(self, index: usize) -> Result<()> {
        drop(self.jobs);
        self.thread
            .join()
            .map_err(|_| stopped(index, "thread panicked"))
    }
}

fn stopped(index: usize, why: &str) -> Error {
    Error::Failed(format!(
        "slot {index} stopped: {why}: restart `scswarm serve`"
    ))
}

/// Serve jobs until queue closes or machine fault stops slot.
fn work(
    slot: &Slot,
    repo: &Git,
    project: &Project,
    index: usize,
    queue: &Receiver<Job>,
    done: &Sender<Report>,
) {
    for job in queue {
        let report = attempt(slot, repo, project, &job);
        let stop = report.is_err();
        if done.send((index, report)).is_err() || stop {
            return;
        }
    }
}

/// One row attempt. Release first: no fault below leaves branch held. Then land
/// ref or fail mark. `Err`: git or podman failed, row not at fault, no mark.
fn attempt(slot: &Slot, repo: &Git, project: &Project, job: &Job) -> Result<()> {
    let id = job.row.id.as_str();
    let outcome = run(slot, project, job);
    slot.release()?;
    let mark = state::fail_ref(id);
    match outcome.map_err(|why| Error::Failed(format!("task {id}: {why}")))? {
        Outcome::Done(commit) => {
            event::land(id);
            repo.ref_set(&state::land_ref(id), &commit)?;
            repo.ref_delete(&mark)
        }
        Outcome::Spent(why) => {
            event::fail(id, &why);
            repo.ref_set(&mark, &job.base)
        }
    }
}

/// Branch, then the one session of the row. `Done` holds its last commit.
fn run(slot: &Slot, project: &Project, job: &Job) -> Result<Outcome<String>> {
    let id = job.row.id.as_str();
    let merge = Transcript::at(paths::transcript(project, id, paths::MERGE_LOG)?);
    let resolve =
        |parent: &str| script::resolve(slot, &job.env, &job.config, &job.row, parent, &merge);
    if let Outcome::Spent(why) =
        slot.checkout(&state::branch(id), git::TRUNK, &job.stack, &resolve)?
    {
        return Ok(Outcome::Spent(why));
    }
    let transcript = Transcript::at(paths::transcript(project, id, paths::RUN_LOG)?);
    script::play(slot, &job.env, &job.config, &job.row, &transcript)
}
