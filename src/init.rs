use std::path::Path;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::host::Host;
use crate::paths;
use crate::podman::{self, Spec};
use crate::slot::Slot;

const HEAD: &str = "HEAD";

/// Bare repo, image, one worktree and container per slot. Safe to run again.
pub fn init(project: &str) -> Result<()> {
    let host = Host::load(project)?;
    let checkout = Git::discover_at(&host.checkout()?)?;
    let repo = repo(project, checkout.root(), &host.trunk)?;
    let config = Config::from_trunk(&repo)?;
    let image = image(&config, project, checkout.root())?;
    println!("image {image}");
    for index in 0..host.workers {
        let slot = Slot::open(&repo, project, index, git::TRUNK, &host.env)?;
        slot.container().create(&Spec {
            image: &image,
            worktree: slot.path(),
            repo: repo.root(),
            mounts: &host.mounts,
        })?;
        println!("slot {index} {}", slot.path().display());
    }
    Ok(())
}

/// Bare repo of a project already built by `init`.
pub fn open(project: &str) -> Result<Git> {
    let path = paths::repo(project)?;
    if !path.join(HEAD).is_file() {
        return Err(Error::Failed(format!(
            "bare repo {} is absent: run `scswarm init {project}`",
            path.display()
        )));
    }
    Ok(Git::at(path))
}

fn repo(project: &str, checkout: &Path, trunk: &str) -> Result<Git> {
    let path = paths::repo(project)?;
    let repo = match path.join(HEAD).is_file() {
        true => Git::at(path),
        false => Git::init_bare(&path)?,
    };
    repo.set_origin(checkout, trunk)?;
    repo.fetch()?;
    Ok(repo)
}

/// Containerfile in the checkout: build it. Anything else: an image podman resolves.
fn image(config: &Config, project: &str, checkout: &Path) -> Result<String> {
    let file = checkout.join(&config.image);
    if !file.is_file() {
        return Ok(config.image.clone());
    }
    let tag = podman::tag(project);
    podman::build(&tag, &file, checkout)?;
    Ok(tag)
}
