use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::event;
use crate::git::Git;
use crate::host::{Mount, Stamp};
use crate::paths::{self, Project};
use crate::podman::{self, Container, Spec};
use crate::step::Outcome;

const WORKTREE_LINK: &str = ".git";

/// One try at a conflicted merge.
pub type Resolve<'a> = dyn FnMut() -> Result<Outcome<()>> + 'a;

/// Worker slot: worktree outside any checkout, container over it.
pub struct Slot {
    git: Git,
    path: PathBuf,
    container: Container,
    project: Project,
    index: usize,
}

/// Slot numbers with a worktree directory or a container.
pub fn indices(project: &Project) -> Result<BTreeSet<usize>> {
    let mut held: BTreeSet<usize> = paths::slot_dirs(project)?.into_iter().collect();
    held.extend(podman::slots(project)?.into_keys());
    Ok(held)
}

/// Drop container and worktree of one slot. Either may be absent.
pub fn remove(repo: &Git, project: &Project, index: usize) -> Result<()> {
    podman::remove(project, index)?;
    repo.worktree_remove(&paths::slot(project, index)?)
}

impl Slot {
    pub fn open(project: &Project, index: usize) -> Result<Self> {
        let path = paths::slot(project, index)?;
        Ok(Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(project, index),
            project: project.clone(),
            index,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Stamp of the container standing on this slot. `None` when absent.
    pub fn stamp(&self) -> Result<Option<Stamp>> {
        podman::stamp(&self.project, self.index)
    }

    /// Worktree, then a container carrying `stamp`: the standing one restarted, so
    /// no process a dead worker left reaches this row, else made again.
    pub fn prepare(
        &self,
        repo: &Git,
        base: &str,
        stamp: &Stamp,
        standing: Option<Stamp>,
        mounts: &[Mount],
        devices: &[String],
    ) -> Result<()> {
        if !self.path.join(WORKTREE_LINK).exists() {
            repo.worktree_add(&self.path, base)?;
        }
        if standing.as_ref() == Some(stamp) {
            return self.container.restart();
        }
        self.container.create(&Spec {
            worktree: &self.path,
            repo: repo.root(),
            mounts,
            devices,
            stamp,
        })?;
        event::create(self.index);
        Ok(())
    }

    pub fn checkout(&self, branch: &str, rev: &str) -> Result<()> {
        self.git.checkout_at(branch, rev)
    }

    /// Merge `rev`, tip of `name`, into `onto` on no branch: `onto` first parent.
    /// Conflict goes to `resolve` once; no merge in flight is git failing.
    pub fn merge_onto(
        &self,
        onto: &str,
        rev: &str,
        name: &str,
        resolve: &mut Resolve,
    ) -> Result<Outcome<()>> {
        self.git.checkout_detached(onto)?;
        let Err(conflict) = self.git.merge(rev, name) else {
            return Ok(Outcome::Done(()));
        };
        if !self.git.merging()? {
            return Err(conflict);
        }
        let spent = match resolve()? {
            Outcome::Spent(why) => format!("{conflict}; `merge` steps: {why}"),
            Outcome::Done(()) if self.git.merging()? => conflict.to_string(),
            Outcome::Done(()) => return Ok(Outcome::Done(())),
        };
        self.git.merge_abort()?;
        Ok(Outcome::Spent(spent))
    }

    pub fn head(&self) -> Result<String> {
        self.git.head()
    }

    /// Free the branch for another slot. No worktree: nothing to free.
    pub fn release(&self) -> Result<()> {
        match self.path.join(WORKTREE_LINK).exists() {
            true => self.git.detach(),
            false => Ok(()),
        }
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        self.git.commit(message)
    }
}
