use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Agent, SESSION_SLOT};
use crate::error::{Error, Result};
use crate::event;
use crate::proc::{self, Captured, Exit};
use crate::slot::Slot;
use crate::step::Policy;

const SHELL: &str = "sh";
/// Shell `$0`, so prompt stays `$1`.
const ARGV0: &str = "scswarm";

/// Exec'd step, tried under its policy. Each try: transcript line, then
/// container restart, so no process outlives its try.
pub struct Call<'a> {
    pub slot: &'a Slot,
    pub input: Input<'a>,
    pub policy: &'a Policy,
    pub log: &'a Path,
    /// Row the step belongs to: event context only.
    pub id: &'a str,
    pub step: usize,
}

pub enum Input<'a> {
    Prompt(Turn<'a>),
    /// Agent turn, reply is commit message: blank costs one try.
    AgentCommit(Turn<'a>),
    /// Shell text as written.
    Run(&'a str),
}

/// One prompt to one agent.
pub struct Turn<'a> {
    pub agent: &'a Agent,
    /// Session id of the agent, known from its first reply on.
    pub session: Option<&'a str>,
    pub text: &'a str,
}

pub enum Outcome {
    Done(Reply),
    /// Every try failed. Text names step and last fault.
    Spent(String),
}

pub struct Reply {
    pub session: Option<String>,
    /// Agent result, or script stdout.
    pub result: String,
}

/// Transcript line. Written here, read by `log`. One schema, both directions.
#[derive(Default, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub step: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Host reason for a failed try beyond exit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<String>,
}

/// Why one try failed.
enum Fault {
    Exit(i32),
    Signal,
    Timeout,
    Blank,
}

impl Call<'_> {
    pub fn run(&self) -> Result<Outcome> {
        event::step(self.id, self.step);
        let mut wait = self.policy.wait;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let captured = proc::capture(self.command()?, self.policy.timeout);
            self.slot.container().restart()?;
            let captured = captured?;
            let fault = match self.judge(&captured) {
                Ok(reply) => {
                    self.record(self.passed(&captured, &reply))?;
                    return Ok(Outcome::Done(reply));
                }
                Err(fault) => fault,
            };
            self.record(self.failed(&captured, &fault))?;
            event::attempt(self.id, self.step, attempt, &fault.to_string());
            if attempt == self.policy.tries {
                return Ok(Outcome::Spent(format!(
                    "step {} must succeed within `tries`: {attempt} spent, last: {fault}",
                    self.step
                )));
            }
            thread::sleep(wait);
            wait = (wait * 2).min(self.policy.wait_max);
        }
    }

    fn command(&self) -> Result<Command> {
        let argv = match &self.input {
            Input::Prompt(turn) | Input::AgentCommit(turn) => turn.argv(),
            Input::Run(text) => vec![SHELL.to_string(), "-c".to_string(), text.to_string()],
        };
        self.slot.container().exec(self.slot.path(), &argv)
    }

    /// Exit 0 and usable reply, else fault.
    fn judge(&self, captured: &Captured) -> std::result::Result<Reply, Fault> {
        match captured.exit {
            Exit::Code(0) => {}
            Exit::Code(code) => return Err(Fault::Exit(code)),
            Exit::Signal => return Err(Fault::Signal),
            Exit::Timeout => return Err(Fault::Timeout),
        }
        match self.input {
            Input::Prompt(_) => Ok(parse(&captured.stdout)),
            Input::AgentCommit(_) => {
                let reply = parse(&captured.stdout);
                match reply.result.trim().is_empty() {
                    true => Err(Fault::Blank),
                    false => Ok(reply),
                }
            }
            Input::Run(_) => Ok(Reply {
                session: None,
                result: captured.stdout.trim().to_string(),
            }),
        }
    }

    /// Agent try keeps parsed reply, script keeps raw output.
    fn passed(&self, captured: &Captured, reply: &Reply) -> Entry {
        match self.input {
            Input::Run(_) => self.output(captured),
            Input::Prompt(_) | Input::AgentCommit(_) => Entry {
                session: reply.session.clone(),
                result: Some(reply.result.clone()),
                ..self.entry(captured)
            },
        }
    }

    fn failed(&self, captured: &Captured, fault: &Fault) -> Entry {
        Entry {
            fault: match fault {
                Fault::Exit(_) => None,
                other => Some(other.to_string()),
            },
            ..self.output(captured)
        }
    }

    fn output(&self, captured: &Captured) -> Entry {
        Entry {
            stdout: Some(captured.stdout.trim().to_string()),
            stderr: Some(captured.stderr.trim().to_string()),
            ..self.entry(captured)
        }
    }

    fn entry(&self, captured: &Captured) -> Entry {
        let (prompt, run) = match &self.input {
            Input::Prompt(turn) | Input::AgentCommit(turn) => (Some(turn.text), None),
            Input::Run(text) => (None, Some(*text)),
        };
        Entry {
            at: now(),
            step: self.step,
            prompt: prompt.map(str::to_string),
            run: run.map(str::to_string),
            exit: match captured.exit {
                Exit::Code(code) => Some(code),
                Exit::Signal | Exit::Timeout => None,
            },
            ..Entry::default()
        }
    }

    fn record(&self, entry: Entry) -> Result<()> {
        let line = serde_json::to_string(&entry)
            .map_err(|why| self.unwritable(&format!("entry: {why}")))?;
        if let Some(dir) = self.log.parent() {
            fs::create_dir_all(dir).map_err(|why| self.unwritable(&why.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log)
            .map_err(|why| self.unwritable(&why.to_string()))?;
        writeln!(file, "{line}").map_err(|why| self.unwritable(&why.to_string()))
    }

    fn unwritable(&self, why: &str) -> Error {
        Error::Failed(format!("transcript {}: {why}", self.log.display()))
    }
}

impl Turn<'_> {
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
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Exit(code) => write!(f, "exit {code}"),
            Fault::Signal => f.write_str("killed by a signal"),
            Fault::Timeout => f.write_str("killed at `timeout`"),
            Fault::Blank => f.write_str("reply must hold a commit message"),
        }
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}
