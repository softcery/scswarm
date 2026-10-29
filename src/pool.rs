use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::{env, fs};

use crate::error::{Error, Result, INVALID, STOPPED};
use crate::event;
use crate::lock;
use crate::paths::{self, Project};
use crate::slot::{self, Slot};
use crate::state::Address;
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
    row: Address,
}

/// Worker that exited, by project: `Ok` once it pushed the row or marked it.
pub struct Ended {
    pub project: Project,
    pub result: Result<()>,
}

/// Queued work of one project, with the turn it last got a row.
pub struct Demand {
    pub served: u64,
    pub queues: Vec<Queue>,
}

/// Queued rows of one track, in file order, with what orders the track.
pub struct Queue {
    pub priority: i32,
    pub running: usize,
    pub rows: Vec<usize>,
}

/// One row picked, each part by index.
#[derive(Debug, PartialEq, Eq)]
pub struct Pick {
    pub project: usize,
    pub queue: usize,
    pub row: usize,
}

/// Up to `slots` picks. Project by rows running, turn, index; inside it priority,
/// rows running in the track, file order. No preemption.
pub fn schedule(demands: &[Demand], slots: usize) -> Vec<Pick> {
    let mut running: Vec<Vec<usize>> = demands
        .iter()
        .map(|demand| demand.queues.iter().map(|queue| queue.running).collect())
        .collect();
    let mut next: Vec<Vec<usize>> = demands
        .iter()
        .map(|demand| vec![0; demand.queues.len()])
        .collect();
    let mut picked = Vec::new();
    while picked.len() < slots {
        let Some((project, queue)) = demands
            .iter()
            .enumerate()
            .flat_map(|(project, demand)| {
                (0..demand.queues.len()).map(move |queue| (project, queue))
            })
            .filter(|&(project, queue)| {
                next[project][queue] < demands[project].queues[queue].rows.len()
            })
            .min_by_key(|&(project, queue)| {
                let share: usize = running[project].iter().sum();
                let track = &demands[project].queues[queue];
                let own = (Reverse(track.priority), running[project][queue], queue);
                (share, demands[project].served, project, own)
            })
        else {
            break;
        };
        let row = demands[project].queues[queue].rows[next[project][queue]];
        picked.push(Pick {
            project,
            queue,
            row,
        });
        next[project][queue] += 1;
        running[project][queue] += 1;
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

    /// Free each slot whose worker ended. Exit 1 is input invalid, exit 3 a stop
    /// that ends nothing for its project, any other failure a machine fault: no
    /// mark, the row replays.
    pub fn reap(&mut self) -> Result<Vec<Ended>> {
        let mut done = Vec::new();
        for (&index, worker) in &mut self.workers {
            let status = worker
                .child
                .try_wait()
                .map_err(|why| stopped(index, &why.to_string()))?;
            done.extend(status.map(|status| (index, status)));
        }
        let mut ended = Vec::new();
        for (index, status) in done {
            let Some(worker) = self.workers.remove(&index) else {
                continue;
            };
            if status.code() == Some(STOPPED) {
                continue;
            }
            let why = format!("row {} on slot {index}: worker {status}", worker.row.key);
            let result = match status.code() {
                _ if status.success() => Ok(()),
                Some(INVALID) => Err(Error::Invalid(why)),
                _ => Err(Error::Failed(format!("{why}: row replays"))),
            };
            ended.push(Ended {
                project: worker.row.project,
                result,
            });
        }
        Ok(ended)
    }

    pub fn holds(&self, row: &Address) -> Result<bool> {
        if self.workers.values().any(|worker| &worker.row == row) {
            return Ok(true);
        }
        lock::held(&paths::transcripts(row)?)
    }

    /// Spawn worker on an idle slot. Row lock taken here, inherited as stdin: held
    /// from spawn. Lock held since the scan, by a `status` probe say: next cycle.
    pub fn assign(&mut self, job: Job) -> Result<()> {
        let row = job.address();
        let logs = paths::transcripts(&row)?;
        fs::create_dir_all(&logs)
            .map_err(|why| Error::Failed(format!("directory {}: {why}", logs.display())))?;
        let Some(lock) = lock::take(&logs)? else {
            return Ok(());
        };
        event::assign(&row, job.slot);
        let child = Command::new(&self.exe)
            .args(job.argv())
            .stdin(lock.into_stdio())
            .spawn()
            .map_err(|why| stopped(job.slot, &format!("worker must start: {why}")))?;
        self.workers.insert(job.slot, Worker { child, row });
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

    fn queue(priority: i32, running: usize, rows: &[usize]) -> Queue {
        Queue {
            priority,
            running,
            rows: rows.to_vec(),
        }
    }

    fn pick(project: usize, queue: usize, row: usize) -> Pick {
        Pick {
            project,
            queue,
            row,
        }
    }

    #[test]
    fn projects_take_turns_then_priority() {
        let demands = [
            Demand {
                served: 0,
                queues: vec![
                    queue(0, 1, &[0, 1]),
                    queue(0, 0, &[2, 3]),
                    queue(5, 0, &[4]),
                ],
            },
            Demand {
                served: 0,
                queues: vec![queue(0, 0, &[5, 6])],
            },
        ];
        assert_eq!(
            schedule(&demands, 5),
            [
                pick(1, 0, 5),
                pick(0, 2, 4),
                pick(1, 0, 6),
                pick(0, 1, 2),
                pick(0, 0, 0)
            ]
        );
    }

    /// One slot, both projects idle each cycle: neither name order nor a
    /// higher `priority` keeps the slot.
    #[test]
    fn idle_projects_alternate_across_cycles() {
        let mut demands = [
            Demand {
                served: 0,
                queues: vec![queue(10, 0, &[0])],
            },
            Demand {
                served: 0,
                queues: vec![queue(0, 0, &[1])],
            },
        ];
        let mut rows = Vec::new();
        for turn in 1..=4 {
            let [Pick { project, row, .. }] = schedule(&demands, 1)[..] else {
                panic!("one slot must take one row");
            };
            demands[project].served = turn;
            rows.push(row);
        }
        assert_eq!(rows, [0, 1, 0, 1]);
    }
}
