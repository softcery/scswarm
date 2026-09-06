use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::board::Row;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::slot::Slot;
use crate::state;
use crate::{paths, stage};

/// One task on one slot: row, branches its base needs, stage to start at.
pub struct Job {
    pub row: Row,
    pub config: Arc<Config>,
    pub stack: Vec<String>,
    pub from: usize,
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
    task: Option<String>,
}

impl Pool {
    /// One worktree, container and thread per configured worker.
    pub fn open(repo: &Git, project: &str, config: &Config) -> Result<Self> {
        let (done, reports) = mpsc::channel();
        let workers = (0..config.workers)
            .map(|index| spawn(repo, project, index, config, done.clone()))
            .collect::<Result<Vec<Worker>>>()?;
        Ok(Self { workers, reports })
    }

    /// Free every slot that finished since the last call. A dead thread is fatal:
    /// its slot would stay busy for good.
    pub fn collect(&mut self) -> Result<()> {
        let Self { workers, reports } = self;
        for index in reports.try_iter() {
            workers[index].task = None;
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
            .any(|worker| worker.task.as_deref() == Some(id))
    }

    pub fn idle(&self) -> Option<usize> {
        self.workers.iter().position(|worker| worker.task.is_none())
    }

    /// Hand `job` to a slot. The slot counts busy until its thread reports back.
    pub fn assign(&mut self, index: usize, job: Job) -> Result<()> {
        let worker = &mut self.workers[index];
        worker.task = Some(job.row.id().to_string());
        worker.jobs.send(job).map_err(|_| stopped(index))
    }
}

fn spawn(
    repo: &Git,
    project: &str,
    index: usize,
    config: &Config,
    done: Sender<usize>,
) -> Result<Worker> {
    let slot = Slot::open(repo, project, index, git::MAIN, &config.env)?;
    slot.container().start()?;
    let (jobs, queue) = mpsc::channel();
    let project = project.to_string();
    let thread = thread::spawn(move || work(&slot, &project, index, &queue, &done));
    Ok(Worker {
        jobs,
        thread,
        task: None,
    })
}

fn stopped(index: usize) -> Error {
    Error::Failed(format!("slot {index} stopped: restart `sercswarm serve`"))
}

/// One job at a time until scheduler drops queue. Failed job frees slot.
fn work(slot: &Slot, project: &str, index: usize, queue: &Receiver<Job>, done: &Sender<usize>) {
    for job in queue {
        if let Err(why) = run(slot, project, &job) {
            eprintln!("{}: {why}", job.row.id());
        }
        if done.send(index).is_err() {
            return;
        }
    }
}

/// Release runs however the stages end: a held branch blocks every other slot.
fn run(slot: &Slot, project: &str, job: &Job) -> Result<()> {
    let outcome = stages(slot, project, job);
    outcome.and(slot.release())
}

fn stages(slot: &Slot, project: &str, job: &Job) -> Result<()> {
    let id = job.row.id();
    slot.checkout(&state::branch(id), git::MAIN, &job.stack)?;
    let total = job.config.stages.len();
    for number in job.from..=total {
        let log = paths::transcript(project, id, number)?;
        stage::run(slot, &job.config, &job.row, number, &log)?;
    }
    Ok(())
}
