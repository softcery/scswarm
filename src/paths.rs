use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::state::Key;

const SWARM_DIR: &str = "swarm";
const SLOT_DIR: &str = "slot";
const SLOT_TREE: &str = "tree";
const BUILD_DIR: &str = "build";
const BUILD_TREE: &str = "tree";
const LOG_DIR: &str = "log";
const LOG_EXT: &str = "jsonl";
const REPO_SUFFIX: &str = ".git";
const HOME_PREFIX: &str = "~/";
const HOME_VAR: &str = "HOME";
const CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const CONFIG_FALLBACK: &str = ".config";
const CONFIG_DIR: &str = "scswarm";
const HOST_FILE: &str = "host.yaml";
const CREDENTIALS_DIR: &str = "credentials";

/// Key of a host file entry. Rides into paths.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub struct Project(String);

impl TryFrom<&str> for Project {
    type Error = Error;

    fn try_from(name: &str) -> Result<Self> {
        let plain = |letter: char| {
            letter.is_ascii_lowercase() || letter.is_ascii_digit() || "._-".contains(letter)
        };
        if name.is_empty() || !name.chars().all(plain) {
            return Err(Error::Invalid(
                "project must hold ascii lowercase letters, digits, `.`, `_` or `-` only"
                    .to_string(),
            ));
        }
        Ok(Self(name.to_string()))
    }
}

impl TryFrom<String> for Project {
    type Error = Error;

    fn try_from(name: String) -> Result<Self> {
        Self::try_from(name.as_str())
    }
}

impl fmt::Display for Project {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn host_file() -> Result<PathBuf> {
    Ok(config_home()?.join(CONFIG_DIR).join(HOST_FILE))
}

/// One file per credential, shared by every project.
pub fn credentials() -> Result<PathBuf> {
    Ok(config_home()?.join(CONFIG_DIR).join(CREDENTIALS_DIR))
}

/// `serve` locks it: one scheduler per user.
pub fn swarm() -> Result<PathBuf> {
    Ok(home()?.join(SWARM_DIR))
}

pub fn repo(project: &Project) -> Result<PathBuf> {
    Ok(swarm()?.join(format!("{project}{REPO_SUFFIX}")))
}

/// Build lock: one build at a time keeps one cache and one image ID.
pub fn builds(project: &Project) -> Result<PathBuf> {
    Ok(swarm()?.join(BUILD_DIR).join(project.to_string()))
}

pub fn build(project: &Project) -> Result<PathBuf> {
    Ok(builds(project)?.join(BUILD_TREE))
}

/// Locked by its worker. Outlives each worktree it holds.
pub fn slot(index: usize) -> Result<PathBuf> {
    Ok(slots()?.join(index.to_string()))
}

/// Worktree of the project its last row ran.
pub fn tree(index: usize) -> Result<PathBuf> {
    Ok(slot(index)?.join(SLOT_TREE))
}

/// `None`: path is no slot worktree.
pub fn slot_index(path: &Path) -> Result<Option<usize>> {
    let slots = real(&slots()?);
    let path = real(path);
    Ok(path
        .parent()
        .filter(|slot| slot.parent() == Some(slots.as_path()))
        .filter(|_| path.file_name().is_some_and(|name| name == SLOT_TREE))
        .and_then(|slot| slot.file_name()?.to_str()?.parse().ok()))
}

/// Slot numbers with a dir.
pub fn slot_dirs() -> Result<Vec<usize>> {
    let dir = slots()?;
    let listing = match fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(why) if why.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(why) => return Err(Error::Failed(format!("directory {}: {why}", dir.display()))),
    };
    Ok(listing
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect())
}

/// Sessions a row can run, each with its own transcript.
pub const RUN_LOG: &str = "run";
pub const MERGE_LOG: &str = "merge";

/// One transcript per session, and the row lock.
pub fn transcripts(project: &Project, key: &Key) -> Result<PathBuf> {
    Ok(swarm()?
        .join(LOG_DIR)
        .join(project.to_string())
        .join(&key.track)
        .join(&key.id))
}

pub fn transcript(project: &Project, key: &Key, name: &str) -> Result<PathBuf> {
    Ok(transcripts(project, key)?.join(format!("{name}.{LOG_EXT}")))
}

pub fn here() -> Result<PathBuf> {
    env::current_dir()
        .map_err(|why| Error::Failed(format!("working directory must resolve: {why}")))
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

/// git reports paths with every symlink resolved. Compare resolved.
pub fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn slots() -> Result<PathBuf> {
    Ok(swarm()?.join(SLOT_DIR))
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
