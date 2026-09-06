use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::Git;
use crate::paths;
use crate::podman::Container;

const WORKTREE_LINK: &str = ".git";

/// Worker slot: worktree outside project checkout, container over it.
/// An open slot is idle, so it holds no branch: a crash leaves one checked out.
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
        let slot = Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(project, index, env),
        };
        slot.release()?;
        Ok(slot)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Put `branch` in worktree at tip, tree clean. New branch: `base` plus merge of `stack`.
    pub fn checkout(&self, branch: &str, base: &str, stack: &[String]) -> Result<()> {
        if self.git.branch_exists(branch)? {
            return self.git.checkout(branch);
        }
        self.git.branch_create(branch, base)?;
        self.git.checkout(branch)?;
        if let Err(why) = stack.iter().try_for_each(|parent| self.git.merge(parent)) {
            self.unmake(branch)?;
            return Err(why);
        }
        Ok(())
    }

    /// Half-stacked branch hides a missing parent. Drop it: next try starts over.
    fn unmake(&self, branch: &str) -> Result<()> {
        self.git.merge_abort()?;
        self.git.detach()?;
        self.git.branch_delete(branch)
    }

    pub fn release(&self) -> Result<()> {
        self.git.detach()
    }

    pub fn commit(&self, message: &str, trailers: &[(&str, String)]) -> Result<String> {
        self.git.commit(message, trailers)
    }
}
