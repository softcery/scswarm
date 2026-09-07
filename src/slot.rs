use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::Git;
use crate::paths;
use crate::podman::Container;

const WORKTREE_LINK: &str = ".git";

/// One try at conflicted merge. `false`: no try made.
pub type Resolve<'a> = dyn Fn(&str) -> Result<bool> + 'a;

/// Worker slot: worktree outside project checkout, container over it.
/// Open slot is idle, holds no branch. Crash leaves one checked out.
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

    /// Put `branch` in worktree at tip, tree clean. New branch: `base` plus merge
    /// of `stack`, named last. Crash mid-merge leaves no branch.
    pub fn checkout(
        &self,
        branch: &str,
        base: &str,
        stack: &[String],
        resolve: &Resolve,
    ) -> Result<()> {
        if self.git.branch_exists(branch)? {
            return self.git.checkout(branch);
        }
        self.git.detach_at(base)?;
        if let Err(why) = self.merge_stack(stack, resolve) {
            self.git.merge_abort()?;
            return Err(why);
        }
        self.git.branch_at(branch, &self.git.head()?)?;
        self.git.checkout(branch)
    }

    /// Merge each parent. Conflict goes to `resolve` once, then stands.
    fn merge_stack(&self, stack: &[String], resolve: &Resolve) -> Result<()> {
        for parent in stack {
            let Err(conflict) = self.git.merge(parent) else {
                continue;
            };
            if !resolve(parent)? || self.git.merging()? {
                return Err(conflict);
            }
        }
        Ok(())
    }

    pub fn release(&self) -> Result<()> {
        self.git.detach()
    }

    pub fn commit(&self, message: &str, trailers: &[(&str, String)]) -> Result<String> {
        self.git.commit(message, trailers)
    }
}
