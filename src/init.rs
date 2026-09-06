use std::path::Path;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::paths;
use crate::podman::{self, Spec};
use crate::slot::Slot;

const HEAD: &str = "HEAD";

/// Bare repo, image, one worktree and container per slot. Safe to run again.
pub fn init(root: &Path) -> Result<()> {
    let checkout = Git::discover_at(root)?;
    let project = paths::project(checkout.root())?;
    let repo = repo(&project, checkout.root())?;
    let config = Config::from_main(&repo)?;
    let image = image(&config, &project, checkout.root())?;
    println!("image {image}");
    for index in 0..config.workers {
        let slot = Slot::open(&repo, &project, index, git::MAIN, &config.env)?;
        slot.container().create(&Spec {
            image: &image,
            worktree: slot.path(),
            repo: repo.root(),
            mounts: &config.mounts,
        })?;
        println!("slot {index} {}", slot.path().display());
    }
    Ok(())
}

/// Project name and bare repo behind the checkout at the working directory.
pub fn open() -> Result<(String, Git)> {
    let project = paths::project(Git::discover()?.root())?;
    let path = paths::repo(&project)?;
    if !path.join(HEAD).is_file() {
        return Err(Error::Failed(format!(
            "bare repo {} is absent: run `sercswarm init`",
            path.display()
        )));
    }
    Ok((project, Git::at(path)))
}

fn repo(project: &str, checkout: &Path) -> Result<Git> {
    let path = paths::repo(project)?;
    let repo = match path.join(HEAD).is_file() {
        true => Git::at(path),
        false => Git::init_bare(&path)?,
    };
    repo.set_origin(checkout)?;
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
