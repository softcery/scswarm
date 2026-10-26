use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::{env, fs};

use crate::error::{Error, Result};
use crate::event;
use crate::lock;
use crate::paths::{self, Project};
use crate::slot::{self, Slot};
use crate::state::Key;
use crate::work::Job;

/// Workers by slot. Slot or row is taken while own worker runs it, or a worker
/// left by an earlier `serve` holds its lock.
pub struct Pool {
    /// Resolved once: Linux names a replaced binary `<path> (deleted)`.
    exe: PathBuf,
    workers: BTreeMap<usize, Worker>,
    /// Slots released in this `serve` life.
    attached: BTreeSet<usize>,
}

struct Worker {
    child: Child,
    project: Project,
    key: Key,
}

/// Queued rows of one track, in file order, with what orders the track.
pub struct Queue {
    /// Index of the track's project: rows running count per project too.
    pub project: usize,
    pub priority: i32,
    pub running: usize,
    pub rows: Vec<usize>,
}

/// Up to `slots` (queue, row) picks. Rank: rows running in the project, priority,
/// rows running in the track, file order. No preemption.
pub fn schedule(queues: &[Queue], slots: usize) -> Vec<(usize, usize)> {
    let mut running: Vec<usize> = queues.iter().map(|queue| queue.running).collect();
    let mut shares: BTreeMap<usize, usize> = BTreeMap::new();
    for queue in queues {
        *shares.entry(queue.project).or_default() += queue.running;
    }
    let mut next = vec![0; queues.len()];
    let mut picked = Vec::new();
    while picked.len() < slots {
        let Some(at) = (0..queues.len())
            .filter(|&at| next[at] < queues[at].rows.len())
            .min_by_key(|&at| {
                let queue = &queues[at];
                let share = shares.get(&queue.project).copied().unwrap_or_default();
                (share, Reverse(queue.priority), running[at], at)
            })
        else {
            break;
        };
        picked.push((at, queues[at].rows[next[at]]));
        next[at] += 1;
        running[at] += 1;
        *shares.entry(queues[at].project).or_default() += 1;
    }
    picked
}

impl Pool {
    /// No slot yet: `converge` opens them.
    pub fn open() -> Result<Self> {
        let exe = env::current_exe().map_err(|why| {
            Error::Failed(format!("path of the running `scswarm` must resolve: {why}"))
        })?;
        Ok(Self {
            exe,
            workers: BTreeMap::new(),
            attached: BTreeSet::new(),
        })
    }

    /// Drop idle slots past `workers`, list idle slots below, lowest first. A busy
    /// slot drops once idle.
    pub fn converge(&mut self, workers: usize) -> Result<Vec<usize>> {
        let mut indices: BTreeSet<usize> = (0..workers).collect();
        indices.extend(slot::indices()?);
        let mut idle = Vec::new();
        for index in indices {
            if self.busy(index)? {
                continue;
            }
            if index >= workers {
                self.remove(index)?;
                continue;
            }
            self.attach(index)?;
            idle.push(index);
        }
        Ok(idle)
    }

    /// Free each slot whose worker ended. A non-zero exit is a machine fault of its
    /// project, returned with it: no mark, the row replays.
    pub fn reap(&mut self) -> Result<Vec<(Project, Error)>> {
        let mut ended = Vec::new();
        for (&index, worker) in &mut self.workers {
            let status = worker
                .child
                .try_wait()
                .map_err(|why| stopped(index, &why.to_string()))?;
            ended.extend(status.map(|status| (index, status)));
        }
        let mut faults = Vec::new();
        for (index, status) in ended {
            let Some(worker) = self.workers.remove(&index) else {
                continue;
            };
            if !status.success() {
                let why = format!(
                    "row {} on slot {index}: worker {status}: row replays",
                    worker.key
                );
                faults.push((worker.project, Error::Failed(why)));
            }
        }
        Ok(faults)
    }

    pub fn holds(&self, project: &Project, key: &Key) -> Result<bool> {
        if self
            .workers
            .values()
            .any(|worker| &worker.project == project && &worker.key == key)
        {
            return Ok(true);
        }
        lock::held(&paths::transcripts(project, key)?)
    }

    /// Spawn worker on an idle slot. Row lock taken here, inherited as stdin: held
    /// from spawn. Lock held since the scan, by a `status` probe say: next cycle.
    pub fn assign(&mut self, project: &Project, job: Job) -> Result<()> {
        let key = job.key();
        let logs = paths::transcripts(project, &key)?;
        fs::create_dir_all(&logs)
            .map_err(|why| Error::Failed(format!("directory {}: {why}", logs.display())))?;
        let Some(row) = lock::take(&logs)? else {
            return Ok(());
        };
        event::assign(key.label(project), job.slot);
        let child = Command::new(&self.exe)
            .args(job.argv(project))
            .stdin(row.into_stdio())
            .spawn()
            .map_err(|why| stopped(job.slot, &format!("worker must start: {why}")))?;
        self.workers.insert(
            job.slot,
            Worker {
                child,
                project: project.clone(),
                key,
            },
        );
        Ok(())
    }

    fn busy(&self, index: usize) -> Result<bool> {
        if self.workers.contains_key(&index) {
            return Ok(true);
        }
        lock::held(&paths::slot(index)?)
    }

    /// Idle slot: free its branch once per `serve` life.
    fn attach(&mut self, index: usize) -> Result<()> {
        if self.attached.insert(index) {
            Slot::open(index)?.release()?;
            event::attach(index);
        }
        Ok(())
    }

    fn remove(&mut self, index: usize) -> Result<()> {
        event::remove(index);
        self.attached.remove(&index);
        slot::remove(index)
    }
}

fn stopped(index: usize, why: &str) -> Error {
    Error::Failed(format!(
        "slot {index} stopped: {why}: restart `scswarm serve`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_take_turns_then_priority() {
        let queue = |project, priority, running, rows: &[usize]| Queue {
            project,
            priority,
            running,
            rows: rows.to_vec(),
        };
        let queues = [
            queue(0, 0, 1, &[0, 1]),
            queue(0, 0, 0, &[2, 3]),
            queue(0, 5, 0, &[4]),
            queue(1, 0, 0, &[5, 6]),
        ];
        assert_eq!(
            schedule(&queues, 5),
            [(3, 5), (2, 4), (3, 6), (1, 2), (0, 0)]
        );
    }
}
