use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::Git;

/// Worker worktree. Sits outside project checkout.
pub struct Slot {
    git: Git,
    path: PathBuf,
}

impl Slot {
    pub fn open(project: &Git, path: PathBuf, base: &str) -> Result<Self> {
        if !path.join(".git").exists() {
            project.worktree_add(&path, base)?;
        }
        Ok(Self {
            git: Git::at(path.clone()),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
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
