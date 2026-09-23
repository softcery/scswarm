use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Session transcript: one JSON line per try and per commit. Sole writer of the file.
pub struct Transcript {
    path: PathBuf,
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
    /// Sha from commit step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Transcript {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn append(&self, entry: &Entry) -> Result<()> {
        let line = serde_json::to_string(entry)
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

    fn unwritable(&self, why: &str) -> Error {
        Error::Failed(format!("transcript {}: {why}", self.path.display()))
    }
}

impl Entry {
    /// Line of step `step`, stamped now.
    pub fn new(step: usize) -> Self {
        Self {
            at: now(),
            step,
            ..Self::default()
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}
