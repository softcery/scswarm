use std::path::Path;

use crate::board::Row;
use crate::call::{Call, Input, Outcome, Turn};
use crate::config::{self, Agent, Config, ID_SLOT, PARENT_SLOT};
use crate::error::{Error, Result};
use crate::event;
use crate::slot::Slot;
use crate::step::{Policy, Step};

/// First character of every slot name.
const SLOT_OPEN: char = '{';

/// Run every step of one row in one session. Returns the last commit.
pub fn play(slot: &Slot, config: &Config, row: &Row, log: &Path) -> Result<String> {
    Session::open(slot, config, row, None, log)?.play(&row.steps)
}

/// Hand conflicted worktree to project merge steps.
/// `false` when project sets none: caller keeps git error.
pub fn resolve(slot: &Slot, config: &Config, row: &Row, parent: &str, log: &Path) -> Result<bool> {
    let Some(steps) = config.merge.as_deref() else {
        return Ok(false);
    };
    event::merge(&row.id, parent);
    Session::open(slot, config, row, Some(parent), log)?.play(steps)?;
    Ok(true)
}

struct Session<'a> {
    slot: &'a Slot,
    agent: &'a Agent,
    /// Config `commit`: prompt of every agent commit step.
    commit_prompt: &'a str,
    id: &'a str,
    log: &'a Path,
    /// Branch merged in this session. `None` off merge.
    parent: Option<&'a str>,
    /// Session id of the agent, known from its first reply on. Spans every step.
    session: Option<String>,
}

impl<'a> Session<'a> {
    fn open(
        slot: &'a Slot,
        config: &'a Config,
        row: &'a Row,
        parent: Option<&'a str>,
        log: &'a Path,
    ) -> Result<Self> {
        Ok(Self {
            slot,
            agent: agent(config, &row.agent)?,
            commit_prompt: &config.commit,
            id: &row.id,
            log,
            parent,
            session: None,
        })
    }

    /// Steps in order. Every settled list ends with a commit step.
    fn play(&mut self, steps: &[Step]) -> Result<String> {
        let mut last = None;
        for (index, step) in steps.iter().enumerate() {
            let number = index + 1;
            match step {
                Step::Prompt { text, policy } => {
                    let text = self.fill(text);
                    let outcome = self.call(Input::Prompt(self.turn(&text)), policy, number)?;
                    self.reply(outcome)?;
                }
                Step::Run { command, policy } => self.run(command, policy, number)?,
                Step::AgentCommit { policy } => {
                    let turn = self.turn(self.commit_prompt);
                    let outcome = self.call(Input::AgentCommit(turn), policy, number)?;
                    let message = self.reply(outcome)?;
                    last = Some(self.commit(message.trim())?);
                }
                Step::Commit { message } => last = Some(self.commit(message)?),
            }
        }
        Ok(last.expect("a settled step list must end with a commit step"))
    }

    /// Board steps go as typed. Merge prompts take the two names the tool knows.
    fn fill(&self, text: &str) -> String {
        match self.parent {
            Some(parent) => fill(text, &[(ID_SLOT, self.id), (PARENT_SLOT, parent)]),
            None => text.to_string(),
        }
    }

    /// Next agent turn of this session.
    fn turn<'t>(&'t self, text: &'t str) -> Turn<'t> {
        Turn {
            agent: self.agent,
            session: self.session.as_deref(),
            text,
        }
    }

    /// Agent reply: session id kept for the next turn, result returned. Spent
    /// tries fail the row.
    fn reply(&mut self, outcome: Outcome) -> Result<String> {
        let reply = match outcome {
            Outcome::Done(reply) => reply,
            Outcome::Spent(why) => return Err(Error::Failed(why)),
        };
        if reply.session.is_some() {
            self.session = reply.session;
        }
        Ok(reply.result)
    }

    /// Shell step. Spent tries fail row unless `continue_on_error`.
    fn run(&self, command: &str, policy: &Policy, number: usize) -> Result<()> {
        match self.call(Input::Run(command), policy, number)? {
            Outcome::Spent(why) if !policy.continue_on_error => Err(Error::Failed(why)),
            Outcome::Done(_) | Outcome::Spent(_) => Ok(()),
        }
    }

    fn call(&self, input: Input, policy: &Policy, number: usize) -> Result<Outcome> {
        Call {
            slot: self.slot,
            input,
            policy,
            log: self.log,
            id: self.id,
            step: number,
        }
        .run()
    }

    fn commit(&self, message: &str) -> Result<String> {
        let commit = self.slot.commit(message)?;
        event::commit(self.id, &commit);
        Ok(commit)
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
