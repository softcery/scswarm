use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::state::Key;

const SWARM_DIR: &str = "swarm";
const SLOT_DIR: &str = "slot";
const BUILD_DIR: &str = "build";
const LOG_DIR: &str = "log";
const LOG_EXT: &str = "jsonl";
const REPO_SUFFIX: &str = ".git";
const HOME_PREFIX: &str = "~/";
const HOME_VAR: &str = "HOME";
const CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const CONFIG_FALLBACK: &str = ".config";
const CONFIG_DIR: &str = "scswarm";
const CONFIG_EXT: &str = "yaml";

/// Project name: stem of host file. Rides into paths, image and container names.
/// Lowercase only: container names are lowercase.
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl fmt::Display for Project {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Host file of one project.
pub fn host_file(project: &Project) -> Result<PathBuf> {
    Ok(config_home()?
        .join(CONFIG_DIR)
        .join(format!("{project}.{CONFIG_EXT}")))
}

/// Bare repo the workers read and write. Fetches from and pushes to the remote.
pub fn repo(project: &Project) -> Result<PathBuf> {
    Ok(swarm()?.join(format!("{project}{REPO_SUFFIX}")))
}

/// Throwaway worktree the image of one slot builds from.
pub fn build(project: &Project, index: usize) -> Result<PathBuf> {
    Ok(swarm()?
        .join(BUILD_DIR)
        .join(project.to_string())
        .join(index.to_string()))
}

pub fn slot(project: &Project, index: usize) -> Result<PathBuf> {
    Ok(slots(project)?.join(index.to_string()))
}

/// Slot number of a worktree path. `None` when the path is no slot of this project.
pub fn slot_index(project: &Project, path: &Path) -> Result<Option<usize>> {
    let slots = real(&slots(project)?);
    let path = real(path);
    Ok(path
        .parent()
        .filter(|parent| *parent == slots)
        .and_then(|_| path.file_name()?.to_str()?.parse().ok()))
}

/// Slot numbers with a worktree directory. Absent directory: no slots.
pub fn slot_dirs(project: &Project) -> Result<Vec<usize>> {
    let dir = slots(project)?;
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

/// Every transcript of one row, one file per session.
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

/// Working directory: the clone `status --tracks` reads.
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

/// git reports worktree paths with every symlink resolved. Compare resolved.
fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn slots(project: &Project) -> Result<PathBuf> {
    Ok(swarm()?.join(SLOT_DIR).join(project.to_string()))
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
