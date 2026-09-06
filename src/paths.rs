use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const SWARM_DIR: &str = "swarm";
const SLOT_DIR: &str = "slot";
const LOG_DIR: &str = "log";
const REPO_SUFFIX: &str = ".git";
const HOME_PREFIX: &str = "~/";

/// Project name: last part of repo root. Rides into image and container names.
pub fn project(root: &Path) -> Result<String> {
    let name = root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| misnamed(root))?;
    let plain = |letter: char| letter.is_ascii_alphanumeric() || "._-".contains(letter);
    if name.is_empty() || !name.chars().all(plain) {
        return Err(misnamed(root));
    }
    Ok(name.to_string())
}

/// Bare repo the workers read and write. Project pushes `main` here.
pub fn repo(project: &str) -> Result<PathBuf> {
    Ok(swarm()?.join(format!("{project}{REPO_SUFFIX}")))
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

/// Path as a command argument.
pub fn arg(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Failed(format!("path must be utf-8: {}", path.display())))
}

/// Leading `~/` stands for the home directory.
pub fn expand(path: &str) -> Result<String> {
    match path.strip_prefix(HOME_PREFIX) {
        Some(rest) => arg(&home()?.join(rest)),
        None => Ok(path.to_string()),
    }
}

fn swarm() -> Result<PathBuf> {
    Ok(home()?.join(SWARM_DIR))
}

fn home() -> Result<PathBuf> {
    env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        Error::Failed("environment variable HOME must hold a home directory path".to_string())
    })
}

fn misnamed(root: &Path) -> Error {
    Error::Failed(format!(
        "repo root must end in a name of ascii letters, digits, `.`, `_` or `-`: {}",
        root.display()
    ))
}
