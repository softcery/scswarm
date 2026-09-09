use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const SWARM_DIR: &str = "swarm";
const SLOT_DIR: &str = "slot";
const LOG_DIR: &str = "log";
const LOG_EXT: &str = "jsonl";
const REPO_SUFFIX: &str = ".git";
const HOME_PREFIX: &str = "~/";
const HOME_VAR: &str = "HOME";
const CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const CONFIG_FALLBACK: &str = ".config";
const CONFIG_DIR: &str = "scswarm";
const CONFIG_EXT: &str = "ron";

/// Project name: stem of the host file. Rides into image and container names.
pub fn project(name: &str) -> Result<String> {
    let plain = |letter: char| letter.is_ascii_alphanumeric() || "._-".contains(letter);
    if name.is_empty() || !name.chars().all(plain) {
        return Err(misnamed());
    }
    Ok(name.to_string())
}

/// Host file of one project.
pub fn host(project: &str) -> Result<PathBuf> {
    Ok(config_home()?
        .join(CONFIG_DIR)
        .join(format!("{project}.{CONFIG_EXT}")))
}

/// Bare repo the workers read and write. Project pushes the trunk branch here.
pub fn repo(project: &str) -> Result<PathBuf> {
    Ok(swarm()?.join(format!("{project}{REPO_SUFFIX}")))
}

pub fn slot(project: &str, index: usize) -> Result<PathBuf> {
    Ok(slots(project)?.join(index.to_string()))
}

/// Slot number of a worktree path. `None` when the path is no slot of this project.
pub fn slot_index(project: &str, path: &Path) -> Result<Option<usize>> {
    let slots = real(&slots(project)?);
    let path = real(path);
    Ok(path
        .parent()
        .filter(|parent| *parent == slots)
        .and_then(|_| path.file_name()?.to_str()?.parse().ok()))
}

/// Every transcript of one task, one file per stage.
pub fn transcripts(project: &str, id: &str) -> Result<PathBuf> {
    Ok(swarm()?.join(LOG_DIR).join(project).join(id))
}

pub fn transcript(project: &str, id: &str, stage: usize) -> Result<PathBuf> {
    Ok(transcripts(project, id)?.join(format!("{stage}.{LOG_EXT}")))
}

/// Stage number of a transcript path. `None` when the file is no transcript.
pub fn transcript_stage(path: &Path) -> Option<usize> {
    if path.extension().and_then(OsStr::to_str)? != LOG_EXT {
        return None;
    }
    path.file_stem()?.to_str()?.parse().ok()
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

/// git reports worktree paths with every symlink resolved. Compare resolved.
fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn slots(project: &str) -> Result<PathBuf> {
    Ok(swarm()?.join(SLOT_DIR).join(project))
}

fn swarm() -> Result<PathBuf> {
    Ok(home()?.join(SWARM_DIR))
}

/// XDG config directory, `~/.config` when the variable is unset or empty.
fn config_home() -> Result<PathBuf> {
    match env::var_os(CONFIG_HOME).filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home()?.join(CONFIG_FALLBACK)),
    }
}

fn home() -> Result<PathBuf> {
    env::var_os(HOME_VAR).map(PathBuf::from).ok_or_else(|| {
        Error::Failed("environment variable HOME must hold a home directory path".to_string())
    })
}

fn misnamed() -> Error {
    Error::Invalid("project name must hold ascii letters, digits, `.`, `_` or `-` only".to_string())
}
