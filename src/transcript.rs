use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Session transcript: one JSON line per attempt start, try and commit. Sole
/// reader and writer of the file.
pub struct Transcript {
    path: PathBuf,
}

/// Transcript line. Written here, read by `log` and `status`. One schema, both directions.
#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    /// Attempt begins against `base`, the `to` tip it starts from.
    Start {
        base: String,
    },
    Try(Try),
    Commit {
        step: usize,
        commit: String,
        message: String,
    },
}

/// One exec'd try of a step.
#[derive(Serialize, Deserialize)]
pub struct Try {
    pub step: usize,
    #[serde(flatten)]
    pub sent: Sent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Host reason for a failed try beyond exit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<String>,
}

/// What a try sent: agent prompt or shell text.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sent {
    Prompt(String),
    Run(String),
}

/// Agent context at the last API call of a turn, in tokens.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Context {
    pub used: u64,
    pub window: u64,
}

impl Transcript {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn append(&self, kind: Kind) -> Result<()> {
        let entry = Entry { at: now(), kind };
        let line = serde_json::to_string(&entry)
            .map_err(|why| self.unwritable(&format!("entry: {why}")))?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|why| self.unwritable(&why.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|why| self.unwritable(&why.to_string()))?;
        writeln!(file, "{line}").map_err(|why| self.unwritable(&why.to_string()))
    }

    /// Raw text. `None` when no such session ran.
    pub fn read(&self) -> Result<Option<String>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(why) if why.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(why) => Err(Error::Failed(format!(
                "transcript {}: {why}",
                self.path.display()
            ))),
        }
    }

    /// Lines that parse, in order. A line of another schema drops.
    pub fn entries(&self) -> Result<Vec<Entry>> {
        Ok(self
            .read()?
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect())
    }

    fn unwritable(&self, why: &str) -> Error {
        Error::Failed(format!("transcript {}: {why}", self.path.display()))
    }
}

impl Try {
    /// Try of step `step` that sent `sent`, nothing kept yet.
    pub fn new(step: usize, sent: Sent) -> Self {
        Self {
            step,
            sent,
            exit: None,
            session: None,
            result: None,
            context: None,
            stdout: None,
            stderr: None,
            fault: None,
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_each_kind() {
        let lines = [
            r#"{"at":1,"kind":"start","base":"abc"}"#,
            r#"{"at":2,"kind":"try","step":1,"prompt":"Work.","exit":0,"session":"s","result":"done","context":{"used":14431,"window":1000000}}"#,
            r#"{"at":3,"kind":"try","step":2,"run":"make lint","exit":2,"stdout":"","stderr":"bad"}"#,
            r#"{"at":4,"kind":"commit","step":3,"commit":"def","message":"feat: x"}"#,
        ];
        for line in lines {
            let entry: Entry = serde_json::from_str(line).expect("line must parse");
            let back = serde_json::to_string(&entry).expect("entry must serialise");
            assert_eq!(back, line);
        }
    }
}
