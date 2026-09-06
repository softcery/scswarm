use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::Git;
use crate::paths;
use crate::podman::Container;

const WORKTREE_LINK: &str = ".git";

/// Worker slot: worktree outside the project checkout, container over that worktree.
pub struct Slot {
    git: Git,
    path: PathBuf,
    container: Container,
}

impl Slot {
    pub fn open(
        repo: &Git,
        project: &str,
        index: usize,
        base: &str,
        env: &[String],
    ) -> Result<Self> {
        let path = paths::slot(project, index)?;
        if !path.join(WORKTREE_LINK).exists() {
            repo.worktree_add(&path, base)?;
        }
        Ok(Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(project, index, env),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Put `branch` in the worktree at its tip, work tree clean of the last run.
    pub fn checkout(&self, branch: &str, base: &str) -> Result<()> {
        if !self.git.branch_exists(branch)? {
            self.git.branch_create(branch, base)?;
        }
        self.git.checkout(branch)
    }

    pub fn commit(&self, message: &str, trailers: &[(&str, String)]) -> Result<String> {
        self.git.commit(message, trailers)
    }
}
