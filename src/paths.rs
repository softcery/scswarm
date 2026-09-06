use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const SWARM_DIR: &str = "swarm";
const SLOT_DIR: &str = "slot";
const LOG_DIR: &str = "log";

/// Project name: last part of repo root.
pub fn project(root: &Path) -> Result<String> {
    root.file_name()
        .and_then(OsStr::to_str)
        .map(str::to_string)
        .ok_or_else(|| {
            Error::Failed(format!(
                "repo root must end in a utf-8 name: {}",
                root.display()
            ))
        })
}

pub fn slot(project: &str, index: usize) -> Result<PathBuf> {
    Ok(swarm()?
        .join(SLOT_DIR)
        .join(project)
        .join(index.to_string()))
}

pub fn transcript(project: &str, id: &str, stage: usize) -> Result<PathBuf> {
    Ok(swarm()?
        .join(LOG_DIR)
        .join(project)
        .join(id)
        .join(format!("{stage}.jsonl")))
}

fn swarm() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| {
        Error::Failed("environment variable HOME must hold a home directory path".to_string())
    })?;
    Ok(PathBuf::from(home).join(SWARM_DIR))
}
