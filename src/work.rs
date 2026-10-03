use std::io;
use std::os::fd::AsFd;

use clap::Args;

use crate::board::{self, Board, Row};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::event;
use crate::git::Git;
use crate::paths::{self, Project};
use crate::slot::Slot;
use crate::state;
use crate::step::Outcome;
use crate::transcript::Transcript;
use crate::{lock, script};

pub const COMMAND: &str = "work";
const SLOT: &str = "slot";
const ROW: &str = "row";
const BASE: &str = "base";
const MERGE: &str = "merge";
const ENV: &str = "env";

/// One row attempt on one slot. Argv of `scswarm work`, row lock on stdin: old
/// `serve` spawns new binary across `cargo install`, so a change needs a release note.
#[derive(Args, Debug, PartialEq, Eq)]
pub struct Job {
    #[arg(long = SLOT)]
    pub slot: usize,
    #[arg(long = ROW, value_name = "ID")]
    pub id: String,
    /// Trunk tip: board, config, branch base, and the commit a failure marks.
    #[arg(long = BASE, value_name = "SHA")]
    pub base: String,
    /// Landed parent branches merged onto base, in order.
    #[arg(long = MERGE, value_name = "BRANCH")]
    pub stack: Vec<String>,
    /// Host variables each exec gets, values inherited.
    #[arg(long = ENV, value_name = "NAME")]
    pub env: Vec<String>,
}

/// Row lock from stdin, slot lock once its last holder ends. `Ok`: land ref or
/// fail mark written. `Err`: machine fault, no mark.
pub fn work(repo: &Git, project: &Project, job: &Job) -> Result<()> {
    let logs = paths::transcripts(project, &job.id)?;
    let _row = lock::adopt(io::stdin().as_fd(), &logs)?;
    let _slot = lock::hold(&paths::slot(project, job.slot)?)?;
    let config = Config::at(repo, &job.base)?;
    let board = Board::at(repo, &job.base, &config)?;
    let row = board.row(&job.id).ok_or_else(|| {
        Error::Invalid(format!(
            "argument `--{ROW}` must name a row of {} at `--{BASE}`",
            board::FILE
        ))
    })?;
    Attempt {
        slot: &Slot::open(project, job.slot)?,
        repo,
        project,
        config: &config,
        row,
        job,
    }
    .run()
}

impl Job {
    pub fn argv(&self, project: &Project) -> Vec<String> {
        let mut argv = vec![COMMAND.to_string(), project.to_string()];
        let mut flag = |name: &str, value: &str| argv.extend([format!("--{name}"), value.into()]);
        flag(SLOT, &self.slot.to_string());
        flag(ROW, &self.id);
        flag(BASE, &self.base);
        self.stack.iter().for_each(|branch| flag(MERGE, branch));
        self.env.iter().for_each(|name| flag(ENV, name));
        argv
    }
}

/// Everything one attempt reads.
struct Attempt<'a> {
    slot: &'a Slot,
    repo: &'a Git,
    project: &'a Project,
    config: &'a Config,
    row: &'a Row,
    job: &'a Job,
}

impl Attempt<'_> {
    /// Release first: no fault below leaves branch held. Then land ref or fail mark.
    fn run(&self) -> Result<()> {
        let id = self.row.id.as_str();
        let outcome = self.session();
        self.slot.release()?;
        let mark = state::fail_ref(id);
        match outcome.map_err(|why| Error::Failed(format!("task {id}: {why}")))? {
            Outcome::Done(commit) => {
                event::land(id);
                self.repo.ref_set(&state::land_ref(id), &commit)?;
                self.repo.ref_delete(&mark)
            }
            Outcome::Spent(why) => {
                event::fail(id, &why);
                self.repo.ref_set(&mark, &self.job.base)
            }
        }
    }

    /// Branch, then the one session of the row. `Done` holds its last commit.
    fn session(&self) -> Result<Outcome<String>> {
        let (slot, config, row, job) = (self.slot, self.config, self.row, self.job);
        let merge = self.transcript(paths::MERGE_LOG)?;
        let resolve = |parent: &str| script::resolve(slot, &job.env, config, row, parent, &merge);
        let branch = state::branch(&row.id);
        if let Outcome::Spent(why) = slot.checkout(&branch, &job.base, &job.stack, &resolve)? {
            return Ok(Outcome::Spent(why));
        }
        let transcript = self.transcript(paths::RUN_LOG)?;
        script::play(slot, &job.env, config, row, &transcript)
    }

    fn transcript(&self, name: &str) -> Result<Transcript> {
        let path = paths::transcript(self.project, &self.row.id, name)?;
        Ok(Transcript::at(path))
    }
}
