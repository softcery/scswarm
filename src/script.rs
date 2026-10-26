use crate::board::Row;
use crate::call::{Call, Message, Reply, Shell, Turn};
use crate::config::{self, Agent, Config, ID_SLOT, PARENT_SLOT};
use crate::credential::Credential;
use crate::error::{Error, Result};
use crate::event;
use crate::slot::Slot;
use crate::state::Key;
use crate::step::{Outcome, Policy, Step, Steps};
use crate::transcript::{Entry, Transcript};

/// First character of every slot name.
const SLOT_OPEN: char = '{';

/// Agent session of one row: the row steps, then any `merge` steps, so a merge
/// resumes the session the row ran.
pub struct Session<'a> {
    slot: &'a Slot,
    credentials: &'a [Credential],
    config: &'a Config,
    row: &'a Row,
    agent: &'a Agent,
    /// Row key: event context.
    key: String,
    /// Session id of the agent, known from its first reply on. Spans every step.
    session: Option<String>,
}

/// Where one pass of steps writes, and the branch it merges in. `None` off merge.
#[derive(Clone, Copy)]
struct Pass<'p> {
    transcript: &'p Transcript,
    parent: Option<&'p str>,
}

impl<'a> Session<'a> {
    pub fn open(
        slot: &'a Slot,
        credentials: &'a [Credential],
        config: &'a Config,
        row: &'a Row,
        key: &Key,
    ) -> Result<Self> {
        Ok(Self {
            slot,
            credentials,
            config,
            row,
            agent: agent(config, &row.agent)?,
            key: key.to_string(),
            session: None,
        })
    }

    /// Every step of the row. `Done` holds the last commit.
    pub fn play(&mut self, transcript: &Transcript) -> Result<Outcome<String>> {
        let row = self.row;
        let pass = Pass {
            transcript,
            parent: None,
        };
        self.run(&row.steps, pass)
    }

    /// Hand the conflicted worktree to the project `merge` steps. No `merge` steps:
    /// nothing runs, the conflict stands.
    pub fn resolve(&mut self, parent: &str, transcript: &Transcript) -> Result<Outcome<()>> {
        let config = self.config;
        let Some(steps) = &config.merge else {
            return Ok(Outcome::Done(()));
        };
        event::merge(&self.key, parent);
        let pass = Pass {
            transcript,
            parent: Some(parent),
        };
        Ok(self.run(steps, pass)?.map(drop))
    }

    /// Steps in order, up to the first spent one.
    fn run(&mut self, steps: &Steps, pass: Pass) -> Result<Outcome<String>> {
        let mut last = None;
        for (index, step) in steps.iter().enumerate() {
            match self.step(index + 1, step, pass)? {
                Outcome::Done(commit) => last = commit.or(last),
                Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
            }
        }
        Ok(Outcome::Done(
            last.expect("`Steps` must end with a commit step"),
        ))
    }

    /// One step. `Done` holds sha of commit step.
    fn step(&mut self, number: usize, step: &Step, pass: Pass) -> Result<Outcome<Option<String>>> {
        match step {
            Step::Prompt { text, policy } => {
                let text = self.fill(text, pass);
                let outcome = self.call(policy, number, pass).ask(&self.turn(&text))?;
                Ok(self.reply(outcome).map(|_| None))
            }
            Step::Run {
                command,
                policy,
                continue_on_error,
            } => Ok(
                match self.call(policy, number, pass).ask(&Shell(command))? {
                    Outcome::Spent(_) if *continue_on_error => Outcome::Done(None),
                    outcome => outcome.map(|()| None),
                },
            ),
            Step::AgentCommit { policy } => {
                let ask = Message(self.turn(&self.config.commit));
                let outcome = self.call(policy, number, pass).ask(&ask)?;
                match self.reply(outcome) {
                    Outcome::Done(message) => Ok(Outcome::Done(Some(self.commit(
                        number,
                        message.trim(),
                        pass,
                    )?))),
                    Outcome::Spent(why) => Ok(Outcome::Spent(why)),
                }
            }
            Step::Commit { message } => {
                Ok(Outcome::Done(Some(self.commit(number, message, pass)?)))
            }
        }
    }

    /// Row steps go as typed. Merge prompts take the two names the tool knows.
    fn fill(&self, text: &str, pass: Pass) -> String {
        match pass.parent {
            Some(parent) => fill(text, &[(ID_SLOT, &self.row.id), (PARENT_SLOT, parent)]),
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

    /// Session id kept for the next turn, result returned.
    fn reply(&mut self, outcome: Outcome<Reply>) -> Outcome<String> {
        outcome.map(|reply| {
            self.session = reply.session;
            reply.result
        })
    }

    fn call<'c>(&'c self, policy: &'c Policy, number: usize, pass: Pass<'c>) -> Call<'c> {
        Call {
            slot: self.slot,
            credentials: self.credentials,
            policy,
            transcript: pass.transcript,
            id: &self.key,
            step: number,
        }
    }

    fn commit(&self, number: usize, message: &str, pass: Pass) -> Result<String> {
        let commit = self.slot.commit(message)?;
        event::commit(&self.key, &commit);
        pass.transcript.append(&Entry {
            commit: Some(commit.clone()),
            message: Some(message.to_string()),
            ..Entry::new(number)
        })?;
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
