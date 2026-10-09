use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::device;
use crate::error::Result;
use crate::git::{self, Git};
use crate::host::{Slots, Stamp};
use crate::paths::{self, Project};
use crate::podman::{self, Container, Spec};
use crate::step::Outcome;

const WORKTREE_LINK: &str = ".git";

/// One try at conflicted merge.
pub type Resolve<'a> = dyn Fn(&str) -> Result<Outcome<()>> + 'a;

/// Worker slot: worktree outside project checkout, container over it. Idle slot
/// holds no branch. Crash leaves one checked out.
pub struct Slot {
    git: Git,
    path: PathBuf,
    container: Container,
}

/// Worktree and container of one slot. Standing worktree stays, container is
/// replaced.
pub fn create(
    repo: &Git,
    project: &Project,
    index: usize,
    slots: &Slots,
    stamp: &Stamp,
) -> Result<()> {
    let slot = Slot::open(project, index)?;
    if !slot.path.join(WORKTREE_LINK).exists() {
        repo.worktree_add(&slot.path, git::TRUNK)?;
    }
    slot.container.create(&Spec {
        worktree: &slot.path,
        repo: repo.root(),
        mounts: &slots.mounts,
        devices: &device::nodes(&slots.devices)?,
        stamp,
    })?;
    slot.release()
}

/// Take over a whole slot: kill what a dead worker left, free its branch.
pub fn attach(project: &Project, index: usize) -> Result<()> {
    let slot = Slot::open(project, index)?;
    slot.container.restart()?;
    slot.release()
}

impl Slot {
    pub fn open(project: &Project, index: usize) -> Result<Self> {
        let path = paths::slot(project, index)?;
        Ok(Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(project, index),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Build `branch` from commit `base` plus merge of `stack`, named last: crash
    /// mid-merge leaves no branch. Standing branch is a cut-short session: rebuild.
    pub fn checkout(
        &self,
        branch: &str,
        base: &str,
        stack: &[String],
        resolve: &Resolve,
    ) -> Result<Outcome<()>> {
        if self.git.branch_exists(branch)? {
            self.git.branch_delete(branch)?;
        }
        self.git.detach_at(base)?;
        match self.merge_stack(stack, resolve) {
            Ok(Outcome::Done(())) => {}
            unmerged => {
                self.git.merge_abort()?;
                return unmerged;
            }
        }
        self.git.branch_at(branch, &self.git.head()?)?;
        self.git.checkout(branch)?;
        Ok(Outcome::Done(()))
    }

    /// Merge each parent. Conflict goes to `resolve` once; conflict left after it
    /// spends the row. Merge error with no merge in flight is git failing.
    fn merge_stack(&self, stack: &[String], resolve: &Resolve) -> Result<Outcome<()>> {
        for parent in stack {
            let Err(conflict) = self.git.merge(parent) else {
                continue;
            };
            if !self.git.merging()? {
                return Err(conflict);
            }
            if let Outcome::Spent(why) = resolve(parent)? {
                return Ok(Outcome::Spent(format!("{conflict}; `merge` steps: {why}")));
            }
            if self.git.merging()? {
                return Ok(Outcome::Spent(conflict.to_string()));
            }
        }
        Ok(Outcome::Done(()))
    }

    pub fn release(&self) -> Result<()> {
        self.git.detach()
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        self.git.commit(message)
    }
}

/// Every slot on the machine with the stamp its container carries. `None`: half
/// a slot, container or worktree absent. The two list apart: either outlives the other.
pub fn survey(project: &Project) -> Result<BTreeMap<usize, Option<Stamp>>> {
    let mut held: BTreeMap<usize, Option<Stamp>> = paths::slot_dirs(project)?
        .into_iter()
        .map(|index| (index, None))
        .collect();
    for (index, stamp) in podman::slots(project)? {
        let whole = paths::slot(project, index)?.join(WORKTREE_LINK).exists();
        held.insert(index, whole.then_some(stamp));
    }
    Ok(held)
}

/// Drop container and worktree of one slot. Either may be absent.
pub fn remove(repo: &Git, project: &Project, index: usize) -> Result<()> {
    podman::remove(project, index)?;
    repo.worktree_remove(&paths::slot(project, index)?)
}
