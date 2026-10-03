use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

impl Slot {
    /// Worktree and container of one slot. Standing worktree stays, container is
    /// replaced.
    pub fn create(
        repo: &Git,
        project: &Project,
        index: usize,
        slots: &Slots,
        stamp: &Stamp,
    ) -> Result<Self> {
        let path = paths::slot(project, index)?;
        if !path.join(WORKTREE_LINK).exists() {
            repo.worktree_add(&path, git::TRUNK)?;
        }
        let slot = Self::bind(project, index, path);
        slot.container.create(&Spec {
            worktree: &slot.path,
            repo: repo.root(),
            mounts: &slots.mounts,
            stamp,
        })?;
        slot.release()?;
        Ok(slot)
    }

    /// Bind to a slot `survey` found whole. Restart kills what a killed `serve`
    /// left running inside.
    pub fn attach(project: &Project, index: usize) -> Result<Self> {
        let slot = Self::bind(project, index, paths::slot(project, index)?);
        slot.container.restart()?;
        slot.release()?;
        Ok(slot)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Build `branch` from `base` plus merge of `stack`, named last: crash mid-merge
    /// leaves no branch. Branch standing holds a cut-short session: drop, build again.
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

    fn bind(project: &Project, index: usize, path: PathBuf) -> Self {
        Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(project, index),
        }
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
