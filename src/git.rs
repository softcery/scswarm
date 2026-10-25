use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{lock, paths, proc};

const BIN: &str = "git";

const ORIGIN: &str = "origin";
const ORIGIN_URL: &str = "remote.origin.url";
const ORIGIN_FETCH: &str = "remote.origin.fetch";
/// Remote branches land as remote-tracking refs, the layout of any clone.
const REMOTES: &str = "refs/remotes/origin/";
const BRANCHES: &str = "+refs/heads/*:refs/remotes/origin/*";
/// Land refs and fail refs. The bare repo holds them; the remote mirrors them.
pub const KEPT: &str = "refs/swarm/";
const KEPT_SPEC: &str = "refs/swarm/*:refs/swarm/*";
/// Directory of the bare repo flocked around each fetch and push.
const REF_STORE: &str = "refs";
/// `push --porcelain` refusal: line mark, then summaries.
const REFUSED: char = '!';
const BEHIND: &str = "[rejected]";
const DECLINED: &str = "[remote rejected]";

pub const HEADS: &str = "refs/heads/";
const HEAD: &str = "HEAD";
const MERGE_HEAD: &str = "MERGE_HEAD";
/// Every character `git check-ref-format` bars, control characters aside.
const BARRED: &str = " ~^:?*[\\";
/// What a name must avoid, for the caller that has to say so.
pub const REF_RULE: &str =
    "no space, `..`, `~^:?*[`, control character, leading `-`, or `.lock` end";
const WORKTREE_FIELD: &str = "worktree ";
const BRANCH_FIELD: &str = "branch ";

/// Sole caller of `git`.
pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Create the bare repo at `path`, parents and all.
    pub fn init_bare(path: &Path) -> Result<Self> {
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|why| Error::Failed(format!("directory {}: {why}", parent.display())))?;
        run(parent, &["init", "--bare", "--quiet", &paths::arg(path)?])?;
        Ok(Self::at(path.to_path_buf()))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Point `origin` at `url`, every branch to a remote-tracking ref. Idempotent.
    /// `--replace-all`: hand-edited key holds many values, plain write fails there.
    pub fn set_origin(&self, url: &str) -> Result<()> {
        self.set_config(ORIGIN_URL, url)?;
        self.set_config(ORIGIN_FETCH, BRANCHES)
    }

    fn set_config(&self, key: &str, value: &str) -> Result<()> {
        run(&self.root, &["config", "--replace-all", key, value]).map(drop)
    }

    /// Bare repo alone: every remote branch, pruned.
    pub fn fetch(&self) -> Result<()> {
        let _store = lock::hold(&self.root.join(REF_STORE))?;
        run(&self.root, &["fetch", "--prune", "--quiet", ORIGIN]).map(drop)
    }

    /// Bare repo alone: land refs and fail refs the remote holds.
    pub fn fetch_kept(&self) -> Result<()> {
        let _store = lock::hold(&self.root.join(REF_STORE))?;
        let spec = format!("+{KEPT_SPEC}");
        run(&self.root, &["fetch", "--quiet", ORIGIN, &spec]).map(drop)
    }

    /// Bare repo alone: remote holds exactly the land and fail refs held here.
    pub fn mirror_kept(&self) -> Result<()> {
        let _store = lock::hold(&self.root.join(REF_STORE))?;
        let args = ["push", "--quiet", "--force", "--prune", ORIGIN, KEPT_SPEC];
        run(&self.root, &args).map(drop)
    }

    /// Bare repo alone: push `commit` to remote `branch`, no force.
    pub fn push(&self, commit: &str, branch: &str) -> Result<Push> {
        let _store = lock::hold(&self.root.join(REF_STORE))?;
        let spec = format!("{commit}:{}", branch_ref(branch));
        let args = ["push", "--porcelain", ORIGIN, &spec];
        let out = proc::answer(BIN, &self.root, &args)?;
        if out.status.success() {
            return Ok(Push::Done);
        }
        judge(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| proc::refused(BIN, &args, &out))
    }

    /// Tip of each remote branch, by branch name, from the last fetch.
    pub fn branches(&self) -> Result<BTreeMap<String, String>> {
        Ok(self
            .refs(REMOTES)?
            .into_iter()
            .filter_map(|(name, sha)| Some((name.strip_prefix(REMOTES)?.to_string(), sha)))
            .collect())
    }

    /// Commit of each ref under `prefix`, by full ref name. One call.
    pub fn refs(&self, prefix: &str) -> Result<BTreeMap<String, String>> {
        let listing = run(
            &self.root,
            &["for-each-ref", "--format=%(objectname) %(refname)", prefix],
        )?;
        Ok(listing
            .lines()
            .filter_map(|line| {
                let (sha, name) = line.split_once(' ')?;
                Some((name.to_string(), sha.to_string()))
            })
            .collect())
    }

    /// Commit `rev` names, sha or full ref. `None` when it names none.
    pub fn commit_of(&self, rev: &str) -> Result<Option<String>> {
        let peeled = format!("{rev}^{{commit}}");
        let sha = proc::lookup(
            BIN,
            &self.root,
            &["rev-parse", "--verify", "--quiet", &peeled],
        )?;
        Ok(sha.map(|sha| sha.trim().to_string()))
    }

    /// True when `rev` holds `path`.
    pub fn has_path(&self, rev: &str, path: &str) -> Result<bool> {
        let listing = run(&self.root, &["ls-tree", rev, "--", path])?;
        Ok(!listing.trim().is_empty())
    }

    /// File content at a revision. `None` when the revision holds no such path.
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
        if !self.has_path(rev, path)? {
            return Ok(None);
        }
        run(&self.root, &["show", &format!("{rev}:{path}")]).map(Some)
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        probe(
            &self.root,
            &["show-ref", "--verify", "--quiet", &branch_ref(branch)],
        )
    }

    /// Commit sha `rev` points at. Errors when no such ref.
    pub fn tip(&self, rev: &str) -> Result<String> {
        run(&self.root, &["rev-parse", rev]).map(|sha| sha.trim().to_string())
    }

    pub fn ref_set(&self, name: &str, commit: &str) -> Result<()> {
        run(&self.root, &["update-ref", name, commit]).map(drop)
    }

    /// Drop `name`. Does nothing when no such ref.
    pub fn ref_delete(&self, name: &str) -> Result<()> {
        run(&self.root, &["update-ref", "-d", name]).map(drop)
    }

    pub fn is_ancestor(&self, commit: &str, rev: &str) -> Result<bool> {
        probe(&self.root, &["merge-base", "--is-ancestor", commit, rev])
    }

    pub fn head(&self) -> Result<String> {
        run(&self.root, &["rev-parse", HEAD]).map(|sha| sha.trim().to_string())
    }

    pub fn branch_delete(&self, branch: &str) -> Result<()> {
        run(&self.root, &["branch", "--delete", "--force", branch]).map(drop)
    }

    /// Drop every local change, then put `branch`, made or moved to `rev`, in the
    /// worktree.
    pub fn checkout_at(&self, branch: &str, rev: &str) -> Result<()> {
        run(
            &self.root,
            &["checkout", "--force", "--no-track", "-B", branch, rev],
        )?;
        run(&self.root, &["clean", "--force", "-d"]).map(drop)
    }

    /// Drop every local change, then put `rev` in the worktree on no branch.
    pub fn checkout_detached(&self, rev: &str) -> Result<()> {
        run(&self.root, &["checkout", "--force", "--detach", rev])?;
        run(&self.root, &["clean", "--force", "-d"]).map(drop)
    }

    /// Leave branch at its commit, so another worktree can take it.
    pub fn detach(&self) -> Result<()> {
        run(&self.root, &["checkout", "--detach"]).map(drop)
    }

    /// Merge `rev` into the worktree head, subject naming branch `name`. A merge
    /// commit even where a fast-forward fits: one commit per merged branch.
    pub fn merge(&self, rev: &str, name: &str) -> Result<()> {
        let subject = format!("Merge {name}");
        run(
            &self.root,
            &["merge", "--no-ff", "--no-edit", "-m", &subject, rev],
        )
        .map(drop)
    }

    /// True until a merge is committed or aborted.
    pub fn merging(&self) -> Result<bool> {
        probe(
            &self.root,
            &["rev-parse", "--verify", "--quiet", MERGE_HEAD],
        )
    }

    /// Undo a conflicted merge. Does nothing when no merge is in flight.
    pub fn merge_abort(&self) -> Result<()> {
        if !self.merging()? {
            return Ok(());
        }
        run(&self.root, &["merge", "--abort"]).map(drop)
    }

    /// Worktree path holding each checked-out branch. Detached worktrees hold none.
    pub fn checkouts(&self) -> Result<BTreeMap<String, PathBuf>> {
        let listing = run(&self.root, &["worktree", "list", "--porcelain"])?;
        Ok(parse_checkouts(&listing))
    }

    pub fn worktree_add(&self, path: &Path, rev: &str) -> Result<()> {
        run(&self.root, &["worktree", "prune"])?;
        let path = paths::arg(path)?;
        run(&self.root, &["worktree", "add", "--detach", &path, rev]).map(drop)
    }

    /// Drop worktree: directory, then registration. Prune reads registration, so
    /// directory goes first.
    pub fn worktree_remove(&self, path: &Path) -> Result<()> {
        if path.exists() {
            fs::remove_dir_all(path)
                .map_err(|why| Error::Failed(format!("worktree {}: {why}", path.display())))?;
        }
        run(&self.root, &["worktree", "prune"]).map(drop)
    }

    /// Add every change, commit with the agent's message. No diff: commit empty.
    pub fn commit(&self, message: &str) -> Result<String> {
        run(&self.root, &["add", "--all"])?;
        run(
            &self.root,
            &["commit", "--allow-empty", "--message", message],
        )?;
        self.head()
    }
}

/// How a push ended short of a git failure.
#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    Done,
    /// Remote holds a commit the push lacks.
    Behind,
    /// Remote turned the ref down, for example branch protection.
    Declined(String),
}

/// `None`: a refusal that is git failing.
fn judge(porcelain: &str) -> Option<Push> {
    let summary = porcelain
        .lines()
        .filter(|line| line.starts_with(REFUSED))
        .find_map(|line| line.split('\t').nth(2))?;
    if summary.starts_with(BEHIND) {
        return Some(Push::Behind);
    }
    summary
        .starts_with(DECLINED)
        .then(|| Push::Declined(summary.to_string()))
}

/// Full ref of branch `name`.
pub fn branch_ref(name: &str) -> String {
    format!("{HEADS}{name}")
}

/// Remote-tracking ref of remote branch `name`.
pub fn remote_branch(name: &str) -> String {
    format!("{REMOTES}{name}")
}

/// Name git takes as ref: `git check-ref-format`, plus no leading `-`, not `HEAD`.
pub fn valid_ref(name: &str) -> bool {
    let barred = |letter: char| letter.is_ascii_control() || BARRED.contains(letter);
    let part = |part: &str| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock");
    !name.is_empty()
        && name != HEAD
        && name != "@"
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.chars().any(barred)
        && name.split('/').all(part)
}

/// Porcelain worktree list: `worktree <path>` opens a block, `branch <ref>` names its branch.
fn parse_checkouts(listing: &str) -> BTreeMap<String, PathBuf> {
    let mut checkouts = BTreeMap::new();
    let mut path = None;
    for line in listing.lines() {
        if let Some(rest) = line.strip_prefix(WORKTREE_FIELD) {
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix(BRANCH_FIELD) {
            let branch = rest.strip_prefix(HEADS).unwrap_or(rest);
            if let Some(path) = path.clone() {
                checkouts.insert(branch.to_string(), path);
            }
        }
    }
    checkouts
}

fn run(dir: &Path, args: &[&str]) -> Result<String> {
    proc::run(BIN, dir, args)
}

fn probe(dir: &Path, args: &[&str]) -> Result<bool> {
    proc::probe(BIN, dir, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_behind_from_declined() {
        let refused = |summary: &str| format!("To r.git\n!\tHEAD:refs/heads/a\t{summary}\nDone\n");
        assert_eq!(
            judge(&refused("[rejected] (fetch first)")),
            Some(Push::Behind)
        );
        assert_eq!(
            judge(&refused("[remote rejected] (protected branch)")),
            Some(Push::Declined(
                "[remote rejected] (protected branch)".to_string()
            ))
        );
        assert_eq!(judge(&refused("[remote failure] (hung up)")), None);
    }

    /// Each name checked against `git check-ref-format --branch` before it landed here.
    #[test]
    fn takes_the_names_git_takes() {
        for name in ["one", "swarm/one", "feature/a-b.c", "refs/swarm/land/a/b"] {
            assert!(valid_ref(name), "{name}");
        }
        for name in [
            "",
            "HEAD",
            "@",
            "-x",
            "a..b",
            "x^",
            "ho*t",
            "a\\b",
            "feat.lock",
            "my task",
            "a//b",
            "a/",
            "a.",
            "a/.b",
            "a@{0}",
        ] {
            assert!(!valid_ref(name), "{name}");
        }
    }
}
