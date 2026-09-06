use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config::{Agent, SESSION_SLOT};
use crate::error::{Error, Result};

/// Wait before a turn runs again. Doubles up to the cap while agent keeps failing.
pub const RETRY_WAIT: Duration = Duration::from_secs(1);
const RETRY_WAIT_MAX: Duration = Duration::from_secs(60);

/// One prompt for one agent, sent again after every failing exit.
pub struct Turn<'a> {
    pub agent: &'a Agent,
    pub session: Option<&'a str>,
    pub prompt: &'a str,
    pub dir: &'a Path,
    pub log: &'a Path,
    pub index: usize,
}

pub struct Reply {
    pub session: Option<String>,
    pub result: String,
}

impl Turn<'_> {
    pub fn run(&self) -> Result<Reply> {
        let mut wait = RETRY_WAIT;
        loop {
            let out = self.command().output().map_err(|why| {
                Error::Failed(format!("agent command `{}`: {why}", self.agent.exec))
            })?;
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            if out.status.success() {
                let reply = parse(&stdout);
                self.record(&json!({
                    "at": now(),
                    "turn": self.index,
                    "prompt": self.prompt,
                    "session": reply.session,
                    "result": reply.result,
                }))?;
                return Ok(reply);
            }
            self.record(&json!({
                "at": now(),
                "turn": self.index,
                "prompt": self.prompt,
                "exit": out.status.code(),
                "stdout": stdout.trim(),
                "error": String::from_utf8_lossy(&out.stderr).trim(),
            }))?;
            thread::sleep(wait);
            wait = (wait * 2).min(RETRY_WAIT_MAX);
        }
    }

    /// `sh -c '<exec> "$@"' sercswarm [<resume>] <prompt>`: agent text never reaches the shell.
    fn command(&self) -> Command {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("{} \"$@\"", self.agent.exec))
            .arg("sercswarm")
            .current_dir(self.dir);
        if let (Some(session), Some(resume)) = (self.session, self.agent.resume.as_deref()) {
            command.args(
                resume
                    .split_whitespace()
                    .map(|token| token.replace(SESSION_SLOT, session)),
            );
        }
        command.arg(self.prompt);
        command
    }

    fn record(&self, entry: &Value) -> Result<()> {
        if let Some(dir) = self.log.parent() {
            fs::create_dir_all(dir).map_err(|why| self.unwritable(&why.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log)
            .map_err(|why| self.unwritable(&why.to_string()))?;
        writeln!(file, "{entry}").map_err(|why| self.unwritable(&why.to_string()))
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
