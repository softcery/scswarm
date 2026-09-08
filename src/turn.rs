use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Agent, SESSION_SLOT};
use crate::error::{Error, Result};
use crate::event;
use crate::podman::Container;

/// Wait before a turn runs again. Doubles up to the cap while agent keeps failing.
pub const RETRY_WAIT: Duration = Duration::from_secs(1);
const RETRY_WAIT_MAX: Duration = Duration::from_secs(60);
/// Tries per turn. Every try spends agent budget, so a stuck agent must stop the row.
pub const ATTEMPTS: usize = 3;
const SHELL: &str = "sh";
/// Shell `$0`, so prompt stays `$1`.
const ARGV0: &str = "scswarm";

/// One prompt for one agent, sent again after every failing exit.
pub struct Turn<'a> {
    pub agent: &'a Agent,
    pub container: &'a Container,
    pub session: Option<&'a str>,
    pub prompt: &'a str,
    pub dir: &'a Path,
    pub log: &'a Path,
    pub number: usize,
    /// Task and stage this turn belongs to: event context only.
    pub at: event::At<'a>,
}

pub struct Reply {
    pub session: Option<String>,
    pub result: String,
}

/// Transcript line. Written here, read by `log`. One schema, both directions.
#[derive(Default, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub turn: usize,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Turn<'_> {
    pub fn run(&self) -> Result<Reply> {
        event::turn(&self.at, self.number);
        let mut wait = RETRY_WAIT;
        let mut last = None;
        for attempt in 1..=ATTEMPTS {
            let out = self.command()?.output().map_err(|why| {
                Error::Failed(format!("agent command `{}`: {why}", self.agent.exec))
            })?;
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            if out.status.success() {
                let reply = parse(&stdout);
                self.record(Entry {
                    session: reply.session.clone(),
                    result: Some(reply.result.clone()),
                    ..self.entry()
                })?;
                return Ok(reply);
            }
            last = out.status.code();
            event::retry(&self.at, self.number, attempt, last);
            self.record(Entry {
                exit: last,
                stdout: Some(stdout.trim().to_string()),
                error: Some(String::from_utf8_lossy(&out.stderr).trim().to_string()),
                ..self.entry()
            })?;
            if attempt < ATTEMPTS {
                thread::sleep(wait);
                wait = (wait * 2).min(RETRY_WAIT_MAX);
            }
        }
        Err(Error::Failed(format!(
            "agent `{}` must exit zero: {ATTEMPTS} tries, last exit {}",
            self.agent.exec,
            last.map_or_else(|| "signal".to_string(), |code| code.to_string())
        )))
    }

    /// `sh -c '<exec> "$@"' scswarm [<resume>] <prompt>`: agent text never reaches the shell.
    fn command(&self) -> Result<Command> {
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
        argv.push(self.prompt.to_string());
        self.container.exec(self.dir, &argv)
    }

    fn entry(&self) -> Entry {
        Entry {
            at: now(),
            turn: self.number,
            prompt: self.prompt.to_string(),
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
