use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::host::{Host, Remote};
use crate::paths::{self, Project};
use crate::podman;

const HEAD: &str = "HEAD";

/// Bare repo and image. `serve` builds the slots from both. Safe to run again,
/// `serve` running or not.
pub fn init(project: &Project) -> Result<()> {
    let host = Host::load(project)?;
    let repo = repo(project, &host.remote)?;
    let config = Config::from_trunk(&repo)?;
    println!("image {}", image(&repo, project, &config)?);
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

fn repo(project: &Project, remote: &Remote) -> Result<Git> {
    let path = paths::repo(project)?;
    let repo = match path.join(HEAD).is_file() {
        true => Git::at(path),
        false => Git::init_bare(&path)?,
    };
    repo.set_origin(&remote.path, &remote.trunk)?;
    repo.fetch()?;
    Ok(repo)
}

/// Build the trunk Containerfile, or pull a named image the host lacks.
fn image(repo: &Git, project: &Project, config: &Config) -> Result<String> {
    let name = config.image_name(repo, project)?;
    if config.builds_image(repo)? {
        build(repo, project, config, &name)?;
    } else if !podman::image_exists(&name)? {
        podman::pull(&name)?;
    }
    Ok(name)
}

/// Context is throwaway worktree of trunk ref: image content has a commit behind
/// it. Untracked and ignored files stay out.
fn build(repo: &Git, project: &Project, config: &Config, name: &str) -> Result<()> {
    let tree = paths::build(project)?;
    repo.worktree_remove(&tree)?;
    repo.worktree_add(&tree, git::TRUNK)?;
    let built = podman::build(name, &tree.join(&config.image), &tree);
    repo.worktree_remove(&tree)?;
    built
}
