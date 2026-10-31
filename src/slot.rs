use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::event;
use crate::git::{Git, WORKTREE_LINK};
use crate::host::Mount;
use crate::paths;
use crate::podman::{self, Container, Spec, Stamp};
use crate::step::Outcome;

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
        match self.owner()? {
            Some(_) => self.git.detach(),
            None => Ok(()),
        }
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        self.git.commit(message)
    }

    /// Text of `name` at the worktree root. The agent wrote the tree and the host reads
    /// the file, so a symlink counts as no file. Inner `Err`: what stands there instead.
    pub fn regular_text(&self, name: &str) -> Result<std::result::Result<String, &'static str>> {
        let path = self.path.join(name);
        let broke = |why: io::Error| Error::Failed(format!("worktree {}: {why}", path.display()));
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(why) if why.kind() == io::ErrorKind::NotFound => return Ok(Err("nothing")),
            Err(why) => return Err(broke(why)),
        };
        if meta.is_symlink() {
            return Ok(Err("a symlink"));
        }
        if !meta.is_file() {
            return Ok(Err("a directory or special file"));
        }
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Ok(text)),
            Err(why) if why.kind() == io::ErrorKind::InvalidData => Ok(Err("no UTF-8 text")),
            Err(why) if why.kind() == io::ErrorKind::PermissionDenied => {
                Ok(Err("a file the host cannot read"))
            }
            Err(why) => Err(broke(why)),
        }
    }

    /// Drop what stands at `name` in the worktree, of any kind. Absent: nothing to drop.
    pub fn drop_path(&self, name: &str) -> Result<()> {
        let path = self.path.join(name);
        let dropped = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(why) if why.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(why) => Err(why),
        };
        dropped.map_err(|why| Error::Failed(format!("worktree {}: {why}", path.display())))
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
