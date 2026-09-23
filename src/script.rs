use crate::board::Row;
use crate::call::{Call, Message, Reply, Shell, Turn};
use crate::config::{self, Agent, Config, ID_SLOT, PARENT_SLOT};
use crate::error::{Error, Result};
use crate::event;
use crate::slot::Slot;
use crate::step::{Outcome, Policy, Step, Steps};
use crate::transcript::{Entry, Transcript};

/// First character of every slot name.
const SLOT_OPEN: char = '{';

/// Run every step of one row in one session. `Done` holds the last commit.
pub fn play(
    slot: &Slot,
    config: &Config,
    row: &Row,
    transcript: &Transcript,
) -> Result<Outcome<String>> {
    Session::open(slot, config, row, None, transcript)?.play(&row.steps)
}

/// Hand conflicted worktree to project merge steps. No `merge` steps: nothing
/// runs, conflict stands.
pub fn resolve(
    slot: &Slot,
    config: &Config,
    row: &Row,
    parent: &str,
    transcript: &Transcript,
) -> Result<Outcome<()>> {
    let Some(steps) = &config.merge else {
        return Ok(Outcome::Done(()));
    };
    event::merge(&row.id, parent);
    let played = Session::open(slot, config, row, Some(parent), transcript)?.play(steps)?;
    Ok(played.map(drop))
}

struct Session<'a> {
    slot: &'a Slot,
    agent: &'a Agent,
    /// Config `commit`: prompt of every agent commit step.
    commit_prompt: &'a str,
    id: &'a str,
    transcript: &'a Transcript,
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
        transcript: &'a Transcript,
    ) -> Result<Self> {
        Ok(Self {
            slot,
            agent: agent(config, &row.agent)?,
            commit_prompt: &config.commit,
            id: &row.id,
            transcript,
            parent,
            session: None,
        })
    }

    /// Steps in order, up to the first spent one.
    fn play(&mut self, steps: &Steps) -> Result<Outcome<String>> {
        let mut last = None;
        for (index, step) in steps.iter().enumerate() {
            match self.step(index + 1, step)? {
                Outcome::Done(commit) => last = commit.or(last),
                Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
            }
        }
        Ok(Outcome::Done(
            last.expect("`Steps` must end with a commit step"),
        ))
    }

    /// One step. `Done` holds sha of commit step.
    fn step(&mut self, number: usize, step: &Step) -> Result<Outcome<Option<String>>> {
        match step {
            Step::Prompt { text, policy } => {
                let text = self.fill(text);
                let outcome = self.call(policy, number).ask(&self.turn(&text))?;
                Ok(self.reply(outcome).map(|_| None))
            }
            Step::Run {
                command,
                policy,
                continue_on_error,
            } => Ok(match self.call(policy, number).ask(&Shell(command))? {
                Outcome::Spent(_) if *continue_on_error => Outcome::Done(None),
                outcome => outcome.map(|()| None),
            }),
            Step::AgentCommit { policy } => {
                let ask = Message(self.turn(self.commit_prompt));
                let outcome = self.call(policy, number).ask(&ask)?;
                match self.reply(outcome) {
                    Outcome::Done(message) => {
                        Ok(Outcome::Done(Some(self.commit(number, message.trim())?)))
                    }
                    Outcome::Spent(why) => Ok(Outcome::Spent(why)),
                }
            }
            Step::Commit { message } => Ok(Outcome::Done(Some(self.commit(number, message)?))),
        }
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

    /// Session id kept for the next turn, result returned.
    fn reply(&mut self, outcome: Outcome<Reply>) -> Outcome<String> {
        outcome.map(|reply| {
            self.session = reply.session;
            reply.result
        })
    }

    fn call<'c>(&'c self, policy: &'c Policy, number: usize) -> Call<'c> {
        Call {
            slot: self.slot,
            policy,
            transcript: self.transcript,
            id: self.id,
            step: number,
        }
    }

    fn commit(&self, number: usize, message: &str) -> Result<String> {
        let commit = self.slot.commit(message)?;
        event::commit(self.id, &commit);
        self.transcript.append(&Entry {
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
