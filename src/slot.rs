use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use crate::credential::Credential;
use crate::error::{Error, Result};
use crate::git::{Git, Worktree};
use crate::paths;
use crate::proc::Stop;
use crate::runner::{Need, Request, Runner};
use crate::step::Outcome;

/// One try at a conflicted merge.
pub type Resolve<'a> = dyn FnMut() -> Result<Outcome<()>> + 'a;

/// Worker slot: worktree outside any checkout, runner over it. Takes rows of any
/// project; the worktree moves to the repo of each row.
pub struct Slot {
    path: PathBuf,
    runner: Runner,
    index: usize,
}

/// Slot numbers with a slot dir or runner state.
pub fn indices(runner: &Runner) -> Result<BTreeSet<usize>> {
    let mut held: BTreeSet<usize> = paths::slot_dirs()?.into_iter().collect();
    held.extend(runner.held()?);
    Ok(held)
}

/// Drop runner state, worktree and slot dir of one slot. Any may be absent.
pub fn remove(index: usize, runner: &Runner) -> Result<()> {
    runner.remove(index)?;
    Slot::open(index, runner)?.drop_tree()?;
    let dir = paths::slot(index)?;
    match dir.exists() {
        true => fs::remove_dir_all(&dir)
            .map_err(|why| Error::Failed(format!("slot {}: {why}", dir.display()))),
        false => Ok(()),
    }
}

impl Slot {
    pub fn open(index: usize, runner: &Runner) -> Result<Self> {
        Ok(Self {
            path: paths::tree(index)?,
            runner: runner.clone(),
            index,
        })
    }

    /// Worktree of `repo`, a foreign one dropped with its ignored files. Then the
    /// runner readies the slot. Spent: the row's fault. Stopped: on `stop`.
    pub fn prepare(&self, request: &Request, stop: &Stop) -> Result<Outcome<()>> {
        let repo = request.repo;
        let fresh = !self.path.is_dir() || repo.worktree(&self.path)?.is_none();
        if fresh {
            self.drop_tree()?;
            repo.worktree_add(&self.path, request.base)?;
        }
        self.runner.prepare(
            self.index,
            &Need {
                request,
                tree: &self.path,
                fresh,
            },
            stop,
        )
    }

    /// Command of one try in the worktree.
    pub fn exec(&self, credentials: &[Credential], argv: &[String]) -> Result<Command> {
        self.runner.exec(self.index, &self.path, credentials, argv)
    }

    /// Kill every process of the slot.
    pub fn reset(&self) -> Result<()> {
        self.runner.reset(self.index)
    }

    pub fn checkout(&self, branch: &str, rev: &str) -> Result<()> {
        let tree = self.tree()?;
        tree.checkout_at(branch, rev)?;
        self.runner.handoff(self.index, &tree)
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
        let tree = self.tree()?;
        tree.checkout_detached(onto)?;
        let Err(conflict) = tree.merge(rev, name) else {
            return Ok(Outcome::Done(()));
        };
        if !tree.merging()? {
            return Err(conflict);
        }
        self.runner.handoff(self.index, &tree)?;
        let ended = match resolve()? {
            Outcome::Spent(why) => Outcome::Spent(format!("{conflict}; `merge` steps: {why}")),
            Outcome::Stopped => Outcome::Stopped,
            Outcome::Done(()) if tree.merging()? => Outcome::Spent(conflict.to_string()),
            Outcome::Done(()) => return Ok(Outcome::Done(())),
        };
        tree.merge_abort()?;
        Ok(ended)
    }

    pub fn head(&self) -> Result<String> {
        self.tree()?.head()
    }

    /// Free the branch for another slot. No worktree: nothing to free.
    pub fn release(&self) -> Result<()> {
        match self.owner()? {
            Some((_, tree)) => tree.detach(),
            None => Ok(()),
        }
    }

    pub fn commit(&self, message: &str) -> Result<String> {
        self.tree()?.commit(message)
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

    /// Worktree git of the standing worktree. Errors when none stands.
    fn tree(&self) -> Result<Worktree> {
        self.owner()?.map(|(_, tree)| tree).ok_or_else(|| {
            Error::Failed(format!(
                "slot {}: worktree {} must stand",
                self.index,
                self.path.display()
            ))
        })
    }

    /// Bare repo that registers the worktree, and the worktree git. Read from the
    /// bare repos alone: the row owns the worktree and its `.git`.
    fn owner(&self) -> Result<Option<(Git, Worktree)>> {
        if !self.path.is_dir() {
            return Ok(None);
        }
        for root in paths::repos()? {
            let repo = Git::at(root);
            if let Some(tree) = repo.worktree(&self.path)? {
                return Ok(Some((repo, tree)));
            }
        }
        Ok(None)
    }

    /// Worktree dir, then its registration in the owner. Owner gone: dir alone.
    fn drop_tree(&self) -> Result<()> {
        if let Some((owner, _)) = self.owner()? {
            return owner.worktree_remove(&self.path);
        }
        match self.path.exists() {
            true => fs::remove_dir_all(&self.path)
                .map_err(|why| Error::Failed(format!("worktree {}: {why}", self.path.display()))),
            false => Ok(()),
        }
    }
}
