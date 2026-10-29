use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::event;
use crate::git::Git;
use crate::host::Mount;
use crate::paths;
use crate::podman::{self, Container, Spec, Stamp};
use crate::step::Outcome;

const WORKTREE_LINK: &str = ".git";

/// One try at a conflicted merge.
pub type Resolve<'a> = dyn FnMut() -> Result<Outcome<()>> + 'a;

/// Worker slot: worktree outside any checkout, container over it. Takes rows of
/// any project; the worktree moves to the repo of each row.
pub struct Slot {
    git: Git,
    path: PathBuf,
    container: Container,
    index: usize,
}

/// Slot numbers with a slot dir or a container.
pub fn indices() -> Result<BTreeSet<usize>> {
    let mut held: BTreeSet<usize> = paths::slot_dirs()?.into_iter().collect();
    held.extend(podman::slots()?.into_keys());
    Ok(held)
}

/// Drop container, worktree and slot dir of one slot. Any may be absent.
pub fn remove(index: usize) -> Result<()> {
    podman::remove(index)?;
    Slot::open(index)?.drop_tree()?;
    let dir = paths::slot(index)?;
    match dir.exists() {
        true => fs::remove_dir_all(&dir)
            .map_err(|why| Error::Failed(format!("slot {}: {why}", dir.display()))),
        false => Ok(()),
    }
}

impl Slot {
    pub fn open(index: usize) -> Result<Self> {
        let path = paths::tree(index)?;
        Ok(Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(index),
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
        podman::stamp(self.index)
    }

    /// Worktree of `repo`, a foreign one dropped with its ignored files. Then the
    /// container: restarted if it carries `stamp`, else made again.
    pub fn prepare(
        &self,
        repo: &Git,
        base: &str,
        stamp: &Stamp,
        standing: Option<Stamp>,
        mounts: &[Mount],
        devices: &[String],
    ) -> Result<()> {
        if self.owner()? != Some(paths::real(repo.root())) {
            self.drop_tree()?;
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
        let ended = match resolve()? {
            Outcome::Spent(why) => Outcome::Spent(format!("{conflict}; `merge` steps: {why}")),
            Outcome::Stopped => Outcome::Stopped,
            Outcome::Done(()) if self.git.merging()? => Outcome::Spent(conflict.to_string()),
            Outcome::Done(()) => return Ok(Outcome::Done(())),
        };
        self.git.merge_abort()?;
        Ok(ended)
    }

    pub fn head(&self) -> Result<String> {
        self.git.head()
    }

    /// Free the branch for another slot. No worktree: nothing to free.
    pub fn release(&self) -> Result<()> {
        match self.owner()? {
            Some(_) => self.git.detach(),
            None => Ok(()),
        }
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        self.git.commit(message)
    }

    /// Repo that owns the worktree. `None`: no worktree, or its repo is gone.
    fn owner(&self) -> Result<Option<PathBuf>> {
        match self.path.join(WORKTREE_LINK).exists() {
            true => self.git.owner(),
            false => Ok(None),
        }
    }

    /// Worktree dir, then its registration in the owner. Owner gone: dir alone.
    fn drop_tree(&self) -> Result<()> {
        if let Some(owner) = self.owner()? {
            return Git::at(owner).worktree_remove(&self.path);
        }
        match self.path.exists() {
            true => fs::remove_dir_all(&self.path)
                .map_err(|why| Error::Failed(format!("worktree {}: {why}", self.path.display()))),
            false => Ok(()),
        }
    }
}
