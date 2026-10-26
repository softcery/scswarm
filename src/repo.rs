use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git::Git;
use crate::paths::{self, Project};

const HEAD: &str = "HEAD";
/// Bare repo still being made.
const PARTIAL: &str = ".partial";

/// Bare repo, made aside on first call with origin, fetch and remote kept refs,
/// then renamed in whole: no mirror runs before kept refs arrive. `true`: made here.
pub fn open_or_make(project: &Project, remote: &str) -> Result<(Git, bool)> {
    let path = paths::repo(project)?;
    if made(&path) {
        return Ok((Git::at(path), false));
    }
    let partial = PathBuf::from(format!("{}{PARTIAL}", path.display()));
    if partial.exists() {
        fs::remove_dir_all(&partial).map_err(|why| failed(&partial, &why))?;
    }
    let repo = Git::init_bare(&partial)?;
    repo.set_origin(remote)?;
    repo.fetch()?;
    repo.fetch_kept()?;
    fs::rename(&partial, &path).map_err(|why| failed(&path, &why))?;
    Ok((Git::at(path), true))
}

/// Bare repo of a project `serve` already opened.
pub fn open(project: &Project) -> Result<Git> {
    let path = paths::repo(project)?;
    if !made(&path) {
        return Err(Error::Failed(format!(
            "bare repo {} is absent: list project `{project}` in the host file and run `scswarm serve`",
            path.display()
        )));
    }
    Ok(Git::at(path))
}

fn made(path: &Path) -> bool {
    path.join(HEAD).is_file()
}

fn failed(path: &Path, why: &std::io::Error) -> Error {
    Error::Failed(format!("bare repo {}: {why}", path.display()))
}
