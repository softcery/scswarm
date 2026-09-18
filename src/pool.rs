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
    reports: Receiver<usize>,
}

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

    /// Free every slot that finished since the last call. A dead thread is fatal:
    /// its slot would stay busy for good.
    pub fn collect(&mut self) -> Result<()> {
        let Self { workers, reports } = self;
        for index in reports.try_iter() {
            workers[index].id = None;
        }
        match workers
            .iter()
            .position(|worker| worker.thread.is_finished())
        {
            Some(index) => Err(stopped(index)),
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
        worker.jobs.send(job).map_err(|_| stopped(index))
    }
}

fn spawn(
    repo: &Git,
    project: &Project,
    index: usize,
    env: &[String],
    done: Sender<usize>,
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

fn stopped(index: usize) -> Error {
    Error::Failed(format!("slot {index} stopped: restart `scswarm serve`"))
}

/// Release first: no panic below leaves a branch held. Mark next: spent attempt
/// must not repeat free. Either failure panics, `collect()` reads it as `stopped`.
fn work(
    slot: &Slot,
    repo: &Git,
    project: &Project,
    index: usize,
    queue: &Receiver<Job>,
    done: &Sender<usize>,
) {
    for job in queue {
        let id = job.row.id.clone();
        let outcome = run(slot, project, &job);
        if let Err(why) = slot.release() {
            panic!("slot {index} must detach its worktree after a task: {why}");
        }
        let mark = state::fail_ref(&id);
        let recorded = match outcome {
            Ok(commit) => {
                event::land(&id);
                repo.ref_set(&state::land_ref(&id), &commit)
                    .and_then(|()| repo.ref_delete(&mark))
            }
            Err(why) => {
                event::fail(&id, &why);
                repo.ref_set(&mark, &job.base)
            }
        };
        if let Err(why) = recorded {
            panic!("slot {index} must record the outcome of task {id}: {why}");
        }
        if done.send(index).is_err() {
            return;
        }
    }
}

/// Branch, then the one session of the row. Returns its commit.
fn run(slot: &Slot, project: &Project, job: &Job) -> Result<String> {
    let id = job.row.id.as_str();
    let resolve = |parent: &str| {
        let log = paths::transcript(project, id, paths::MERGE_LOG)?;
        script::resolve(slot, &job.config, &job.row, parent, &log)
    };
    slot.checkout(&state::branch(id), git::TRUNK, &job.stack, &resolve)?;
    let log = paths::transcript(project, id, paths::RUN_LOG)?;
    script::play(slot, &job.config, &job.row, &log)
}
