use crate::board::{self, NextFile, Row, Track};
use crate::call::{Call, Message, Reply, Shell, Turn};
use crate::config::{self, Agent, Config, ID_SLOT, PARENT_SLOT};
use crate::credential::Credential;
use crate::error::{Error, Result};
use crate::event;
use crate::git::{self, Git, Push};
use crate::host::Host;
use crate::paths::Project;
use crate::slot::Slot;
use crate::state::Address;
use crate::step::{Outcome, Policy, Step};
use crate::transcript::{Entry, Transcript};

/// First character of every slot name.
const SLOT_OPEN: char = '{';

/// Agent session of one row: the row steps, then any `merge` steps, so a merge
/// resumes the session the row ran.
pub struct Session<'a> {
    slot: &'a Slot,
    /// Bare repo of the project: writes and pushes a published track file.
    repo: &'a Git,
    credentials: &'a [Credential],
    config: &'a Config,
    track: &'a Track,
    row: &'a Row,
    agent: &'a Agent,
    project: &'a Project,
    /// Row label: event context.
    key: String,
    /// Session id of the agent, known from its first reply on. Spans every step.
    session: Option<String>,
}

/// Where one pass of steps writes, the branch it merges in, the file a commit publishes.
#[derive(Clone, Copy)]
struct Pass<'p> {
    transcript: &'p Transcript,
    /// `None` off merge.
    parent: Option<&'p str>,
    /// Worktree name and checked text. Last row step alone.
    publish: Option<(&'p str, &'p NextFile)>,
}

impl<'a> Session<'a> {
    pub fn open(
        slot: &'a Slot,
        repo: &'a Git,
        credentials: &'a [Credential],
        config: &'a Config,
        track: &'a Track,
        row: &'a Row,
        address: &'a Address,
    ) -> Result<Self> {
        Ok(Self {
            slot,
            repo,
            credentials,
            config,
            track,
            row,
            agent: agent(config, &row.agent)?,
            project: &address.project,
            key: address.to_string(),
            session: None,
        })
    }

    /// Every step of the row. `Done` holds the last commit. A published file is
    /// checked before the last step, so a refused file costs no commit turn.
    pub fn play(&mut self, transcript: &Transcript) -> Result<Outcome<String>> {
        let row = self.row;
        let number = row.steps.len();
        let (last, first) = row
            .steps
            .split_last()
            .expect("`Steps` must hold one step or more");
        let pass = Pass {
            transcript,
            parent: None,
            publish: None,
        };
        if let Outcome::Spent(why) = self.run(first, pass)? {
            return Ok(Outcome::Spent(why));
        }
        let next = match row.publish.as_deref().map(|file| self.next_file(file)) {
            None => None,
            Some(Ok(next)) => Some(next),
            Some(Err(why)) => return Ok(Outcome::Spent(format!("step {number}: {why}"))),
        };
        let publish = row.publish.as_deref().zip(next.as_ref());
        let outcome = self.step(number, last, Pass { publish, ..pass })?;
        Ok(outcome.map(|commit| commit.expect("`Steps` must end with a commit step")))
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
            publish: None,
        };
        self.run(steps, pass)
    }

    /// Steps in order, up to the first spent one.
    fn run(&mut self, steps: &[Step], pass: Pass) -> Result<Outcome<()>> {
        for (index, step) in steps.iter().enumerate() {
            if let Outcome::Spent(why) = self.step(index + 1, step, pass)? {
                return Ok(Outcome::Spent(why));
            }
        }
        Ok(Outcome::Done(()))
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
                    Outcome::Done(message) => self.commit(number, message.trim(), pass),
                    Outcome::Spent(why) => Ok(Outcome::Spent(why)),
                }
            }
            Step::Commit { message } => self.commit(number, message, pass),
        }
    }

    /// Text of `file`, checked as the next track file against the project file in the
    /// worktree. `Err` names what the row got wrong. No git or podman call runs here.
    fn next_file(&self, file: &str) -> std::result::Result<NextFile, String> {
        let text = self.slot.regular_text(file).ok_or_else(|| {
            "field `publish` must name a regular text file at the worktree root".to_string()
        })?;
        let config = self.slot.regular_text(config::FILE).ok_or_else(|| {
            format!(
                "{} must stand as a regular text file at the worktree root",
                config::FILE
            )
        })?;
        Config::parse(&config)
            .and_then(|config| self.track.check_next(text, &self.row.id, &config))
            .map_err(|why| why.to_string())
    }

    /// Commit `next` as the sole file on the track tip, push it with no force, again
    /// onto the new tip after a fetch when behind.
    fn publish(&self, number: usize, next: &NextFile, pass: Pass) -> Result<Outcome<()>> {
        let repo = self.repo;
        let host = Host::load()?;
        let branch = format!("{}{}", host.entry(self.project)?.tracks, self.track.name);
        let tracking = git::remote_branch(&branch);
        let message = format!("publish {}", self.key);
        let commit = loop {
            let tip = repo.commit_of(&tracking)?.ok_or_else(|| {
                Error::Failed(format!("track branch `{branch}` must stand on the remote"))
            })?;
            let commit = repo.commit_file(&tip, board::FILE, next.text(), &message)?;
            match repo.push(&commit, &branch)? {
                Push::Done => break commit,
                Push::Declined(why) => {
                    return Ok(Outcome::Spent(format!(
                        "step {number}: push to `{branch}` must land: {why}"
                    )));
                }
                Push::Behind => repo.fetch()?,
            }
            if repo.commit_of(&tracking)?.as_deref() == Some(tip.as_str()) {
                return Err(Error::Failed(format!(
                    "push to `{branch}` must land: the remote refused it, a fetch brought no new commit"
                )));
            }
        };
        event::publish(&self.key, &commit);
        record(number, commit, &message, pass)?;
        Ok(Outcome::Done(()))
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

    /// Drop and push any published file, then commit the row.
    fn commit(&self, number: usize, message: &str, pass: Pass) -> Result<Outcome<Option<String>>> {
        if let Some((file, next)) = pass.publish {
            self.slot.drop_path(file)?;
            if let Outcome::Spent(why) = self.publish(number, next, pass)? {
                return Ok(Outcome::Spent(why));
            }
        }
        let commit = self.slot.commit(message)?;
        event::commit(&self.key, &commit);
        record(number, commit.clone(), message, pass)?;
        Ok(Outcome::Done(Some(commit)))
    }
}

/// Transcript line of one commit.
fn record(number: usize, commit: String, message: &str, pass: Pass) -> Result<()> {
    pass.transcript.append(&Entry {
        commit: Some(commit),
        message: Some(message.to_string()),
        ..Entry::new(number)
    })
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
