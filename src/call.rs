use std::fmt;

use serde_json::Value;

use crate::config::{Agent, SESSION_SLOT};
use crate::credential::Credential;
use crate::error::Result;
use crate::event;
use crate::proc::{self, Captured, Exit, Stop};
use crate::slot::Slot;
use crate::step::{Outcome, Policy};
use crate::transcript::{Entry, Transcript};

const SHELL: &str = "sh";
/// Shell `$0`, so prompt stays `$1`.
const ARGV0: &str = "scswarm";

/// Exec'd step, tried under its policy. Each try: transcript line, then
/// container restart, so no process outlives its try.
pub struct Call<'a> {
    pub slot: &'a Slot,
    /// Credentials each try gets.
    pub credentials: &'a [Credential],
    pub policy: &'a Policy,
    pub transcript: &'a Transcript,
    /// Row the step belongs to: event context only.
    pub id: &'a str,
    pub step: usize,
    /// Asked each tick of a try and of the pause between tries.
    pub stop: &'a Stop<'a>,
}

/// What one exec'd step sends, and what a try that exits 0 gives back.
pub trait Ask {
    type Reply;

    fn argv(&self) -> Vec<String>;

    /// `entry` with what this step sent.
    fn sent(&self, entry: Entry) -> Entry;

    /// Reply read from stdout of a try that exited 0.
    fn reply(&self, stdout: &str) -> Judged<Self::Reply>;

    /// `entry` with what a passed try keeps.
    fn kept(&self, reply: &Self::Reply, captured: &Captured, entry: Entry) -> Entry;
}

pub type Judged<T> = std::result::Result<T, Fault>;

/// One prompt to one agent.
pub struct Turn<'a> {
    pub agent: &'a Agent,
    /// Session id of the agent, known from its first reply on.
    pub session: Option<&'a str>,
    pub text: &'a str,
}

/// Agent turn whose reply is a commit message: blank costs one try.
pub struct Message<'a>(pub Turn<'a>);

/// Shell text as written.
pub struct Shell<'a>(pub &'a str);

pub struct Reply {
    pub session: Option<String>,
    pub result: String,
}

/// Why one try failed.
pub enum Fault {
    Exit(i32),
    Signal,
    Timeout,
    Stopped,
    Blank,
    Sessionless,
}

impl Call<'_> {
    pub fn ask<A: Ask>(&self, ask: &A) -> Result<Outcome<A::Reply>> {
        event::step(self.id, self.step);
        let mut wait = self.policy.wait;
        let mut attempt = 1;
        loop {
            let fault = match self.once(ask)? {
                Ok(reply) => return Ok(Outcome::Done(reply)),
                Err(Fault::Stopped) => return Ok(Outcome::Stopped),
                Err(fault) => fault,
            };
            event::attempt(self.id, self.step, attempt, &fault.to_string());
            if attempt == self.policy.tries {
                return Ok(Outcome::Spent(format!(
                    "step {} must succeed within `tries`: {attempt} spent, last: {fault}",
                    self.step
                )));
            }
            if proc::pause(wait, self.stop)? {
                return Ok(Outcome::Stopped);
            }
            wait = wait.saturating_mul(2).min(self.policy.wait_max);
            attempt += 1;
        }
    }

    /// One try, then restart, even when the try broke: no process outlives it.
    fn once<A: Ask>(&self, ask: &A) -> Result<Judged<A::Reply>> {
        let tried = self.tried(ask);
        self.slot.container().restart()?;
        tried
    }

    /// Exec, judge, record.
    fn tried<A: Ask>(&self, ask: &A) -> Result<Judged<A::Reply>> {
        let command =
            self.slot
                .container()
                .exec(self.slot.path(), self.credentials, &ask.argv())?;
        let captured = proc::capture(command, self.policy.timeout, self.stop)?;
        let judged = judge(ask, &captured);
        let entry = ask.sent(Entry {
            exit: match captured.exit {
                Exit::Code(code) => Some(code),
                Exit::Signal | Exit::Timeout | Exit::Stopped => None,
            },
            ..Entry::new(self.step)
        });
        self.transcript.append(&match &judged {
            Ok(reply) => ask.kept(reply, &captured, entry),
            Err(Fault::Exit(_)) => output(&captured, entry),
            Err(fault) => Entry {
                fault: Some(fault.to_string()),
                ..output(&captured, entry)
            },
        })?;
        Ok(judged)
    }
}

impl Ask for Turn<'_> {
    type Reply = Reply;

    /// `sh -c '<exec> "$@"' scswarm [<resume>] <text>`: agent text never reaches the shell.
    fn argv(&self) -> Vec<String> {
        let mut argv = vec![
            SHELL.to_string(),
            "-c".to_string(),
            format!("{} \"$@\"", self.agent.exec),
            ARGV0.to_string(),
        ];
        if let (Some(session), Some(resume)) = (self.session, self.agent.resume.as_deref()) {
            argv.extend(
                resume
                    .split_whitespace()
                    .map(|token| token.replace(SESSION_SLOT, session)),
            );
        }
        argv.push(self.text.to_string());
        argv
    }

    fn sent(&self, entry: Entry) -> Entry {
        Entry {
            prompt: Some(self.text.to_string()),
            ..entry
        }
    }

    /// An agent that resumes must name its session, or the next turn starts blank.
    fn reply(&self, stdout: &str) -> Judged<Reply> {
        let reply = parse(stdout);
        match (&self.agent.resume, &reply.session) {
            (Some(_), None) => Err(Fault::Sessionless),
            _ => Ok(reply),
        }
    }

    fn kept(&self, reply: &Reply, _: &Captured, entry: Entry) -> Entry {
        Entry {
            session: reply.session.clone(),
            result: Some(reply.result.clone()),
            ..entry
        }
    }
}

impl Ask for Message<'_> {
    type Reply = Reply;

    fn argv(&self) -> Vec<String> {
        self.0.argv()
    }

    fn sent(&self, entry: Entry) -> Entry {
        self.0.sent(entry)
    }

    fn reply(&self, stdout: &str) -> Judged<Reply> {
        let reply = self.0.reply(stdout)?;
        match reply.result.trim().is_empty() {
            true => Err(Fault::Blank),
            false => Ok(reply),
        }
    }

    fn kept(&self, reply: &Reply, captured: &Captured, entry: Entry) -> Entry {
        self.0.kept(reply, captured, entry)
    }
}

impl Ask for Shell<'_> {
    type Reply = ();

    fn argv(&self) -> Vec<String> {
        vec![SHELL.to_string(), "-c".to_string(), self.0.to_string()]
    }

    fn sent(&self, entry: Entry) -> Entry {
        Entry {
            run: Some(self.0.to_string()),
            ..entry
        }
    }

    fn reply(&self, _: &str) -> Judged<()> {
        Ok(())
    }

    fn kept(&self, (): &(), captured: &Captured, entry: Entry) -> Entry {
        output(captured, entry)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Exit(code) => write!(f, "exit {code}"),
            Fault::Signal => f.write_str("killed by a signal"),
            Fault::Timeout => f.write_str("killed at `timeout`"),
            Fault::Stopped => f.write_str("killed on scswarm stop"),
            Fault::Blank => f.write_str("reply must hold a commit message"),
            Fault::Sessionless => {
                f.write_str("reply must hold `session_id`: the agent sets `resume`")
            }
        }
    }
}

/// Exit 0 hands stdout to the step kind; anything else is a fault.
fn judge<A: Ask>(ask: &A, captured: &Captured) -> Judged<A::Reply> {
    match captured.exit {
        Exit::Code(0) => ask.reply(&captured.stdout),
        Exit::Code(code) => Err(Fault::Exit(code)),
        Exit::Signal => Err(Fault::Signal),
        Exit::Timeout => Err(Fault::Timeout),
        Exit::Stopped => Err(Fault::Stopped),
    }
}

fn output(captured: &Captured, entry: Entry) -> Entry {
    Entry {
        stdout: Some(captured.stdout.trim().to_string()),
        stderr: Some(captured.stderr.trim().to_string()),
        ..entry
    }
}

fn parse(stdout: &str) -> Reply {
    let value = serde_json::from_str::<Value>(stdout).ok();
    let field = |key: &str| {
        value
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Reply {
        session: field("session_id"),
        result: field("result").unwrap_or_else(|| stdout.trim().to_string()),
    }
}
