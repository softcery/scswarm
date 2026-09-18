use std::path::Path;
use std::thread;

use crate::board::Row;
use crate::config::{self, Agent, Config, ID_SLOT, PARENT_SLOT};
use crate::error::{Error, Result};
use crate::event;
use crate::slot::Slot;
use crate::turn::{Turn, ATTEMPTS, RETRY_WAIT};

/// First character of every slot name.
const SLOT_OPEN: char = '{';

/// Send every turn of one row in one session, then commit the worktree with the
/// message the agent replies. Returns the commit.
pub fn play(slot: &Slot, config: &Config, row: &Row, log: &Path) -> Result<String> {
    Session {
        slot,
        agent: agent(config, &row.agent)?,
        id: &row.id,
        parent: None,
        log,
        session: None,
    }
    .play(&row.turns, &config.commit)
}

/// Hand conflicted worktree to project merge turns.
/// `false` when project sets none: caller keeps git error.
pub fn resolve(slot: &Slot, config: &Config, row: &Row, parent: &str, log: &Path) -> Result<bool> {
    let Some(turns) = config.merge.as_deref() else {
        return Ok(false);
    };
    event::merge(&row.id, parent);
    Session {
        slot,
        agent: agent(config, &row.agent)?,
        id: &row.id,
        parent: Some(parent),
        log,
        session: None,
    }
    .play(turns, &config.commit)?;
    Ok(true)
}

struct Session<'a> {
    slot: &'a Slot,
    agent: &'a Agent,
    id: &'a str,
    log: &'a Path,
    /// Branch merged in this session. `None` off merge.
    parent: Option<&'a str>,
    /// Session id of the agent, known from its first reply on.
    session: Option<String>,
}

impl Session<'_> {
    fn play(&mut self, turns: &[String], commit: &str) -> Result<String> {
        for (index, turn) in turns.iter().enumerate() {
            self.turn(&self.prompt(turn), index + 1)?;
        }
        self.commit(commit, turns.len() + 1)
    }

    /// Board turns go as typed. Merge turns take the two names the tool knows.
    fn prompt(&self, turn: &str) -> String {
        match self.parent {
            Some(parent) => fill(turn, &[(ID_SLOT, self.id), (PARENT_SLOT, parent)]),
            None => turn.to_string(),
        }
    }

    fn turn(&mut self, prompt: &str, number: usize) -> Result<String> {
        let reply = Turn {
            agent: self.agent,
            container: self.slot.container(),
            session: self.session.as_deref(),
            prompt,
            dir: self.slot.path(),
            log: self.log,
            number,
            id: self.id,
        }
        .run()?;
        if reply.session.is_some() {
            self.session = reply.session;
        }
        Ok(reply.result)
    }

    /// Ask agent for the message, commit on host. Blank reply costs one try.
    fn commit(&mut self, prompt: &str, number: usize) -> Result<String> {
        for attempt in 1..=ATTEMPTS {
            let reply = self.turn(prompt, number)?;
            let message = reply.trim();
            if message.is_empty() {
                if attempt < ATTEMPTS {
                    thread::sleep(RETRY_WAIT);
                }
                continue;
            }
            let commit = self.slot.commit(message)?;
            event::commit(self.id, &commit);
            return Ok(commit);
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

fn agent<'a>(config: &'a Config, key: &str) -> Result<&'a Agent> {
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
            "Resolve {parent} into {id}. {gone}",
            &[("{id}", "search-01"), ("{parent}", "swarm/{id}")],
        );
        assert_eq!(filled, "Resolve swarm/{id} into search-01. {gone}");
    }
}
