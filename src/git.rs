use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{paths, proc};

/// Trunk tip, fetched from the branch the host file declares. The tool holds this
/// ref, never a branch name.
pub const TRUNK: &str = "refs/swarm/trunk";

const BIN: &str = "git";

const ORIGIN_URL: &str = "remote.origin.url";
const ORIGIN_FETCH: &str = "remote.origin.fetch";

const REFS: &str = "refs/";
const HEADS: &str = "refs/heads/";
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

/// Commit picked by one trailer, value of another.
pub struct Trailer {
    pub commit: String,
    pub value: String,
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

    /// Point `origin` at checkout, map `trunk` to `TRUNK`. Idempotent.
    /// `--replace-all`: hand-edited key can hold many values, plain write fails on those.
    pub fn set_origin(&self, url: &Path, trunk: &str) -> Result<()> {
        self.set_config(ORIGIN_URL, &paths::arg(url)?)?;
        self.set_config(ORIGIN_FETCH, &format!("+{}:{TRUNK}", refname(trunk)))
    }

    fn set_config(&self, key: &str, value: &str) -> Result<()> {
        run(&self.root, &["config", "--replace-all", key, value]).map(drop)
    }

    pub fn fetch(&self) -> Result<()> {
        run(&self.root, &["fetch", "--prune", "--quiet"]).map(drop)
    }

    /// True when the revision holds `path`.
    pub fn has_path(&self, rev: &str, path: &str) -> Result<bool> {
        let listing = run(&self.root, &["ls-tree", &refname(rev), "--", path])?;
        Ok(!listing.trim().is_empty())
    }

    /// File content at a revision. `None` when the revision holds no such path.
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
        if !self.has_path(rev, path)? {
            return Ok(None);
        }
        run(&self.root, &["show", &format!("{}:{path}", refname(rev))]).map(Some)
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        let full = refname(branch);
        probe(&self.root, &["show-ref", "--verify", "--quiet", &full])
    }

    /// Commit sha `rev` points at. Errors when no such ref.
    pub fn tip(&self, rev: &str) -> Result<String> {
        run(&self.root, &["rev-parse", &refname(rev)]).map(|sha| sha.trim().to_string())
    }

    /// Commit `name` points at. `None` when no such ref.
    pub fn ref_commit(&self, name: &str) -> Result<Option<String>> {
        if !probe(&self.root, &["show-ref", "--verify", "--quiet", name])? {
            return Ok(None);
        }
        run(&self.root, &["rev-parse", name]).map(|sha| Some(sha.trim().to_string()))
    }

    pub fn ref_set(&self, name: &str, commit: &str) -> Result<()> {
        run(&self.root, &["update-ref", name, commit]).map(drop)
    }

    /// Drop `name`. Does nothing when no such ref.
    pub fn ref_delete(&self, name: &str) -> Result<()> {
        run(&self.root, &["update-ref", "-d", name]).map(drop)
    }

    /// Newest commit of `branch` with trailer `key` = `value`, plus its `want` trailer.
    pub fn trailer_commit(
        &self,
        branch: &str,
        key: &str,
        value: &str,
        want: &str,
    ) -> Result<Option<Trailer>> {
        let format = format!(
            "--format=%H%x1f{}%x1f{}",
            trailer_format(key),
            trailer_format(want)
        );
        let log = run(&self.root, &["log", &format, &refname(branch), "--"])?;
        Ok(log.lines().find_map(|line| parse_trailer(line, value)))
    }

    pub fn is_ancestor(&self, commit: &str, rev: &str) -> Result<bool> {
        let ancestor = refname(rev);
        probe(
            &self.root,
            &["merge-base", "--is-ancestor", commit, &ancestor],
        )
    }

    pub fn head(&self) -> Result<String> {
        run(&self.root, &["rev-parse", HEAD]).map(|sha| sha.trim().to_string())
    }

    /// Name `commit` as `branch`.
    pub fn branch_at(&self, branch: &str, commit: &str) -> Result<()> {
        run(&self.root, &["branch", "--no-track", branch, commit]).map(drop)
    }

    pub fn branch_delete(&self, branch: &str) -> Result<()> {
        run(&self.root, &["branch", "--delete", "--force", branch]).map(drop)
    }

    /// Drop every local change, then put `branch` in the worktree.
    pub fn checkout(&self, branch: &str) -> Result<()> {
        run(&self.root, &["checkout", "--force", branch])?;
        run(&self.root, &["clean", "--force", "-d"]).map(drop)
    }

    /// Leave the branch behind at the same commit, so another worktree can take it.
    pub fn detach(&self) -> Result<()> {
        run(&self.root, &["checkout", "--detach"]).map(drop)
    }

    /// Drop every local change, then put `rev` in the worktree under no branch.
    pub fn detach_at(&self, rev: &str) -> Result<()> {
        run(
            &self.root,
            &["checkout", "--force", "--detach", &refname(rev)],
        )?;
        run(&self.root, &["clean", "--force", "-d"]).map(drop)
    }

    /// Merge `branch` into the worktree head. Fast-forward when it can.
    /// Subject set here: a detached head writes `into HEAD` on its own.
    pub fn merge(&self, branch: &str) -> Result<()> {
        let subject = format!("Merge {branch}");
        run(
            &self.root,
            &["merge", "--no-edit", "-m", &subject, &refname(branch)],
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
        run(
            &self.root,
            &["worktree", "add", "--detach", &path, &refname(rev)],
        )
        .map(drop)
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

    /// Stage every change and commit it: message from the agent, trailers from the host.
    pub fn commit(&self, message: &str, trailers: &[(&str, String)]) -> Result<String> {
        run(&self.root, &["add", "--all"])?;
        let mut args = vec![
            "commit".to_string(),
            "--allow-empty".to_string(),
            "--message".to_string(),
            message.to_string(),
        ];
        for (key, value) in trailers {
            args.push("--trailer".to_string());
            args.push(format!("{key}={value}"));
        }
        run(&self.root, &borrow(&args))?;
        self.head()
    }
}

/// Name git takes as ref. Rules of `git check-ref-format`, plus branch rules: no
/// leading `-`, not `HEAD`. Board and host text pass here before becoming a ref.
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

/// Full ref name: a branch name alone can also read as a path. A name that
/// already starts with `refs/` stands, the rule git itself resolves by.
fn refname(rev: &str) -> String {
    match rev.starts_with(REFS) {
        true => rev.to_string(),
        false => format!("{HEADS}{rev}"),
    }
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

fn trailer_format(key: &str) -> String {
    format!("%(trailers:key={key},valueonly,separator=%x1e)")
}

fn parse_trailer(line: &str, value: &str) -> Option<Trailer> {
    let mut fields = line.split('\x1f');
    let commit = fields.next()?;
    let picked = fields.next()?.trim();
    let wanted = fields.next()?.trim();
    (picked == value && !wanted.is_empty()).then(|| Trailer {
        commit: commit.to_string(),
        value: wanted.to_string(),
    })
}

fn borrow(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
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

    /// Each name checked against `git check-ref-format --branch` before it landed here.
    #[test]
    fn takes_the_names_git_takes() {
        for name in ["one", "swarm/one", "feature/a-b.c", "refs/swarm/trunk"] {
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
