use std::path::Path;
use std::thread;

use crate::board::Row;
use crate::config::{self, Agent, Config, COMMIT_TURN, ID_SLOT, PARENT_SLOT, TASK_SLOT};
use crate::error::{Error, Result};
use crate::event;
use crate::slot::Slot;
use crate::state::{STAGE_TRAILER, TASK_TRAILER};
use crate::turn::{Turn, ATTEMPTS, RETRY_WAIT};

/// First character of every slot name.
const SLOT_OPEN: char = '{';

const COMMIT_PROMPT: &str =
    "Reply with the commit message for the work in this session. Message only.";

/// Merge session number. Runs before stage 1, commits merge.
pub const MERGE: usize = 0;

/// Send every turn of one stage in one session, commit worktree on `{commit}`.
pub fn run(slot: &Slot, config: &Config, row: &Row, number: usize, log: &Path) -> Result<()> {
    let stage = &config.stages[number - 1];
    StageRun {
        slot,
        agent: agent(config, row, stage)?,
        row,
        number,
        total: config.stages.len(),
        log,
        parent: None,
        session: None,
    }
    .play(stage)
}

/// Hand conflicted worktree to project merge turns.
/// `false` when project sets none: caller keeps git error.
pub fn resolve(slot: &Slot, config: &Config, row: &Row, parent: &str, log: &Path) -> Result<bool> {
    let Some(stage) = config.merge.as_ref() else {
        return Ok(false);
    };
    let total = config.stages.len();
    event::merge(&at(row, MERGE, total), parent);
    StageRun {
        slot,
        agent: agent(config, row, stage)?,
        row,
        number: MERGE,
        total,
        log,
        parent: Some(parent),
        session: None,
    }
    .play(stage)?;
    Ok(true)
}

struct StageRun<'a> {
    slot: &'a Slot,
    agent: &'a Agent,
    row: &'a Row,
    number: usize,
    total: usize,
    log: &'a Path,
    /// Branch merged in this session. `None` off merge.
    parent: Option<&'a str>,
    /// Session id of the agent, known from its first reply on.
    session: Option<String>,
}

impl StageRun<'_> {
    fn play(&mut self, stage: &config::Stage) -> Result<()> {
        for (index, template) in stage.turns.iter().enumerate() {
            let number = index + 1;
            if template == COMMIT_TURN {
                self.commit(number)?;
            } else {
                self.turn(&self.prompt(template), number)?;
            }
        }
        Ok(())
    }

    fn prompt(&self, template: &str) -> String {
        let mut slots = vec![
            (ID_SLOT, self.row.id.as_str()),
            (TASK_SLOT, self.row.task.as_str()),
        ];
        slots.extend(self.parent.map(|parent| (PARENT_SLOT, parent)));
        fill(template, &slots)
    }

    fn at(&self) -> event::At<'_> {
        at(self.row, self.number, self.total)
    }

    fn turn(&mut self, prompt: &str, turn: usize) -> Result<String> {
        let reply = Turn {
            agent: self.agent,
            container: self.slot.container(),
            session: self.session.as_deref(),
            prompt,
            dir: self.slot.path(),
            log: self.log,
            number: turn,
            at: self.at(),
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
        for attempt in 1..=ATTEMPTS {
            let reply = self.turn(COMMIT_PROMPT, turn)?;
            let message = reply.trim();
            if message.is_empty() {
                if attempt < ATTEMPTS {
                    thread::sleep(RETRY_WAIT);
                }
                continue;
            }
            let trailers = [
                (TASK_TRAILER, self.row.id.clone()),
                (STAGE_TRAILER, format!("{number}/{total}")),
            ];
            let commit = self.slot.commit(message, &trailers)?;
            event::commit(&self.at(), &commit);
            return Ok(());
        }
        Err(Error::Failed(format!(
            "agent must reply a commit message of one line or more: {ATTEMPTS} tries, each blank"
        )))
    }
}

/// Substitute in one pass: filled text never reads as template again.
fn fill(template: &str, slots: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find(SLOT_OPEN) {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        match slots
            .iter()
            .find_map(|(slot, value)| Some((rest.strip_prefix(slot)?, *value)))
        {
            Some((tail, value)) => {
                out.push_str(value);
                rest = tail;
            }
            None => {
                out.push(SLOT_OPEN);
                rest = &rest[SLOT_OPEN.len_utf8()..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn at<'a>(row: &'a Row, number: usize, total: usize) -> event::At<'a> {
    event::At {
        id: &row.id,
        stage: number,
        total,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_no_filled_text_as_template() {
        let filled = fill(
            "/execute {id}: {task} {gone}",
            &[("{id}", "search-01"), ("{task}", "Keep {id} literal.")],
        );
        assert_eq!(filled, "/execute search-01: Keep {id} literal. {gone}");
    }
}
