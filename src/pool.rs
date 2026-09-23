use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::board::Row;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::event;
use crate::git::{self, Git};
use crate::host::Slots;
use crate::paths::Project;
use crate::slot::Slot;
use crate::state;
use crate::step::Outcome;
use crate::transcript::Transcript;
use crate::{paths, script};

/// One task on one slot: row and the branches its base needs.
pub struct Job {
    pub row: Row,
    pub config: Arc<Config>,
    pub stack: Vec<String>,
    /// Trunk tip this attempt runs against. Failure marks it, so the row waits for a new tip.
    pub base: String,
}

/// Slot threads and what each holds. The assignment rule lives in the caller.
pub struct Pool {
    workers: Vec<Worker>,
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
    /// One thread per worker, each bound to the slot `init` made.
    pub fn open(repo: &Git, project: &Project, slots: &Slots) -> Result<Self> {
        let (done, reports) = mpsc::channel();
        let workers = (0..slots.workers)
            .map(|index| spawn(repo, project, index, &slots.env, done.clone()))
            .collect::<Result<Vec<Worker>>>()?;
        Ok(Self { workers, reports })
    }

    /// Free each slot done since last call. Machine fault or dead thread is
    /// fatal: its slot stays busy for good.
    pub fn collect(&mut self) -> Result<()> {
        let Self { workers, reports } = self;
        for (index, report) in reports.try_iter() {
            workers[index].id = None;
            report.map_err(|why| stopped(index, &why.to_string()))?;
        }
        match workers
            .iter()
            .position(|worker| worker.thread.is_finished())
        {
            Some(index) => Err(stopped(index, "thread ended")),
            None => Ok(()),
        }
    }

    pub fn holds(&self, id: &str) -> bool {
        self.workers
            .iter()
            .any(|worker| worker.id.as_deref() == Some(id))
    }

    pub fn idle(&self) -> Option<usize> {
        self.workers.iter().position(|worker| worker.id.is_none())
    }

    /// Hand `job` to a slot. The slot counts busy until its thread reports back.
    pub fn assign(&mut self, index: usize, job: Job) -> Result<()> {
        let worker = &mut self.workers[index];
        worker.id = Some(job.row.id.clone());
        event::assign(&job.row.id, index);
        worker
            .jobs
            .send(job)
            .map_err(|_| stopped(index, "thread ended"))
    }
}

fn spawn(
    repo: &Git,
    project: &Project,
    index: usize,
    env: &[String],
    done: Sender<Report>,
) -> Result<Worker> {
    let slot = Slot::attach(project, index, env)?;
    let (jobs, queue) = mpsc::channel();
    let project = project.clone();
    let own = Git::at(repo.root().to_path_buf());
    let thread = thread::spawn(move || work(&slot, &own, &project, index, &queue, &done));
    Ok(Worker {
        jobs,
        thread,
        id: None,
    })
}

fn stopped(index: usize, why: &str) -> Error {
    Error::Failed(format!(
        "slot {index} stopped: {why}: restart `scswarm serve`"
    ))
}

/// Serve jobs until pool drops or machine fault stops slot.
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
    let resolve = |parent: &str| script::resolve(slot, &job.config, &job.row, parent, &merge);
    if let Outcome::Spent(why) =
        slot.checkout(&state::branch(id), git::TRUNK, &job.stack, &resolve)?
    {
        return Ok(Outcome::Spent(why));
    }
    let transcript = Transcript::at(paths::transcript(project, id, paths::RUN_LOG)?);
    script::play(slot, &job.config, &job.row, &transcript)
}
