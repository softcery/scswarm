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
use crate::{paths, stage};

/// Outcome of running a job's stages: `Err` carries the stage in flight.
type Staged<T> = std::result::Result<T, (usize, Error)>;

/// One task on one slot: row, branches its base needs, stage to start at.
pub struct Job {
    pub row: Row,
    pub config: Arc<Config>,
    pub stack: Vec<String>,
    pub from: usize,
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
        let at = event::At {
            id: &job.row.id,
            stage: job.from,
            total: job.config.stages.len(),
        };
        event::assign(&at, index);
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
        let total = job.config.stages.len();
        let outcome = stages(slot, project, &job);
        if let Err(why) = slot.release() {
            panic!("slot {index} must detach its worktree after a task: {why}");
        }
        let mark = state::fail_ref(&id);
        let recorded = match outcome {
            Ok(()) => {
                event::land(&id);
                repo.ref_delete(&mark)
            }
            Err((stage, why)) => {
                event::fail(
                    &event::At {
                        id: &id,
                        stage,
                        total,
                    },
                    &why,
                );
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

fn stages(slot: &Slot, project: &Project, job: &Job) -> Staged<()> {
    let id = job.row.id.as_str();
    let resolve = |parent: &str| {
        let log = paths::transcript(project, id, stage::MERGE)?;
        stage::resolve(slot, &job.config, &job.row, parent, &log)
    };
    slot.checkout(&state::branch(id), git::TRUNK, &job.stack, &resolve)
        .map_err(|why| (job.from, why))?;
    let total = job.config.stages.len();
    for number in job.from..=total {
        let log = paths::transcript(project, id, number).map_err(|why| (number, why))?;
        stage::run(slot, &job.config, &job.row, number, &log).map_err(|why| (number, why))?;
    }
    Ok(())
}
