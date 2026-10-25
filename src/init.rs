use crate::error::{Error, Result};
use crate::git::Git;
use crate::host::Host;
use crate::paths::{self, Project};

const HEAD: &str = "HEAD";

/// Bare repo, `origin` and a first fetch. A new bare repo takes back the land and
/// fail refs the remote holds. Safe to run again, `serve` running or not.
pub fn init(project: &Project) -> Result<()> {
    let host = Host::load(project)?;
    let path = paths::repo(project)?;
    let fresh = !path.join(HEAD).is_file();
    let repo = match fresh {
        true => Git::init_bare(&path)?,
        false => Git::at(path),
    };
    repo.set_origin(&host.remote)?;
    repo.fetch()?;
    if fresh {
        repo.fetch_kept()?;
    }
    println!("repo {}", repo.root().display());
    Ok(())
}

/// Bare repo of a project already built by `init`.
pub fn open(project: &Project) -> Result<Git> {
    let path = paths::repo(project)?;
    if !path.join(HEAD).is_file() {
        return Err(Error::Failed(format!(
            "bare repo {} is absent: run `scswarm init {project}`",
            path.display()
        )));
    }
    Ok(Git::at(path))
}
