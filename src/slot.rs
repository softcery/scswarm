use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
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
    /// Worktree and container of one slot. `init` alone creates.
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
        let slot = Self::bind(project, index, path, &slots.env);
        slot.container.create(&Spec {
            worktree: &slot.path,
            repo: repo.root(),
            mounts: &slots.mounts,
            stamp,
        })?;
        slot.release()?;
        Ok(slot)
    }

    /// Bind to worktree and container `init` made; absent slot is drift. Restart
    /// kills what a killed `serve` left running inside.
    pub fn attach(project: &Project, index: usize, env: &[String]) -> Result<Self> {
        let path = paths::slot(project, index)?;
        if !path.join(WORKTREE_LINK).exists() {
            return Err(missing(project, &format!("worktree {}", path.display())));
        }
        let slot = Self::bind(project, index, path, env);
        if !slot.container.exists()? {
            return Err(missing(
                project,
                &format!("container {}", slot.container.name()),
            ));
        }
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

    fn bind(project: &Project, index: usize, path: PathBuf, env: &[String]) -> Self {
        Self {
            git: Git::at(path.clone()),
            path,
            container: Container::slot(project, index, env),
        }
    }
}

/// Slot set against host file: numbers, image, fields each container baked. Runs
/// before any worker opens.
pub fn verify(project: &Project, slots: &Slots, stamp: &Stamp) -> Result<()> {
    let made = podman::slots(project)?;
    let held: BTreeSet<usize> = made.keys().copied().collect();
    if held != (0..slots.workers).collect() {
        return Err(drifted(project, "field `workers`"));
    }
    if let Some(field) = made.values().find_map(|made| stamp.drift(made)) {
        return Err(drifted(project, field));
    }
    if !podman::image_exists(&stamp.image)? {
        return Err(missing(project, &format!("image {}", stamp.image)));
    }
    Ok(())
}

/// Drop every slot at or above `keep`. `init` alone destroys. Containers and
/// directories list apart: either outlives the other.
pub fn prune(repo: &Git, project: &Project, keep: usize) -> Result<()> {
    let mut held: BTreeSet<usize> = podman::slots(project)?.into_keys().collect();
    held.extend(paths::slot_dirs(project)?);
    for index in held.into_iter().filter(|&index| index >= keep) {
        podman::remove(project, index)?;
        repo.worktree_remove(&paths::slot(project, index)?)?;
        println!("dropped slot {index}");
    }
    Ok(())
}

/// Machine no longer holds what host file declares. `init` writes that set, so stop.
pub fn drifted(project: &Project, what: &str) -> Error {
    Error::Failed(format!(
        "slot set differs from the host file at {what}: run `scswarm init {project}`"
    ))
}

fn missing(project: &Project, what: &str) -> Error {
    Error::Failed(format!("{what} is absent: run `scswarm init {project}`"))
}
