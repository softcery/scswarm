use std::path::Path;
use std::thread;

use crate::board::Row;
use crate::config::{self, Agent, Config, COMMIT_TURN, TASK_SLOT};
use crate::error::{Error, Result};
use crate::slot::Slot;
use crate::state::{STAGE_TRAILER, TASK_TRAILER};
use crate::turn::{Turn, RETRY_WAIT};

const COMMIT_PROMPT: &str =
    "Reply with the commit message for the work in this session. Message only.";

/// Send every turn of one stage in one session, commit the worktree on `{commit}`.
pub fn run(slot: &Slot, config: &Config, row: &Row, number: usize, log: &Path) -> Result<()> {
    let stage = &config.stages[number - 1];
    let mut run = StageRun {
        slot,
        agent: agent(config, row, stage)?,
        row,
        number,
        total: config.stages.len(),
        log,
        session: None,
    };
    for (index, template) in stage.turns.iter().enumerate() {
        let number = index + 1;
        if template == COMMIT_TURN {
            run.commit(number)?;
        } else {
            run.turn(&template.replace(TASK_SLOT, &row.task), number)?;
        }
    }
    Ok(())
}

struct StageRun<'a> {
    slot: &'a Slot,
    agent: &'a Agent,
    row: &'a Row,
    number: usize,
    total: usize,
    log: &'a Path,
    /// Session id of the agent, known from its first reply on.
    session: Option<String>,
}

impl StageRun<'_> {
    fn turn(&mut self, prompt: &str, turn: usize) -> Result<String> {
        let reply = Turn {
            agent: self.agent,
            container: self.slot.container(),
            session: self.session.as_deref(),
            prompt,
            dir: self.slot.path(),
            log: self.log,
            number: turn,
        }
        .run()?;
        if reply.session.is_some() {
            self.session = reply.session;
        }
        Ok(reply.result)
    }

    /// Ask agent for message, commit on host with stage trailers.
    fn commit(&mut self, turn: usize) -> Result<()> {
        let (number, total) = (self.number, self.total);
        loop {
            let reply = self.turn(COMMIT_PROMPT, turn)?;
            let message = reply.trim();
            if message.is_empty() {
                thread::sleep(RETRY_WAIT);
                continue;
            }
            let trailers = [
                (TASK_TRAILER, self.row.id().to_string()),
                (STAGE_TRAILER, format!("{number}/{total}")),
            ];
            let commit = self.slot.commit(message, &trailers)?;
            println!(
                "{} stage {number}/{total} {}",
                self.row.id(),
                short(&commit)
            );
            return Ok(());
        }
    }
}

fn agent<'a>(config: &'a Config, row: &Row, stage: &config::Stage) -> Result<&'a Agent> {
    let key = stage.agent.as_deref().unwrap_or(&row.agent);
    config.agents.get(key).ok_or_else(|| {
        Error::Invalid(format!(
            "agent `{key}` must name a key of `agents` in {}",
            config::FILE
        ))
    })
}

fn short(commit: &str) -> String {
    commit.chars().take(7).collect()
}
