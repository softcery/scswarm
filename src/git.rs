use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{paths, proc};

pub const MAIN: &str = "main";

const BIN: &str = "git";

const ORIGIN_URL: &str = "remote.origin.url";
const ORIGIN_FETCH: &str = "remote.origin.fetch";

const HEADS: &str = "refs/heads/";
const MERGE_HEAD: &str = "MERGE_HEAD";
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
    pub fn discover() -> Result<Self> {
        Self::discover_at(Path::new("."))
    }

    /// Repo that holds `dir`.
    pub fn discover_at(dir: &Path) -> Result<Self> {
        let root = run(dir, &["rev-parse", "--show-toplevel"])?;
        Ok(Self::at(PathBuf::from(root.trim())))
    }

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

    /// Point `origin` at the project checkout and take `main` from it alone.
    pub fn set_origin(&self, url: &Path) -> Result<()> {
        run(&self.root, &["config", ORIGIN_URL, &paths::arg(url)?])?;
        let refspec = format!("+{0}:{0}", head_ref(MAIN));
        run(&self.root, &["config", ORIGIN_FETCH, &refspec]).map(drop)
    }

    pub fn fetch(&self) -> Result<()> {
        run(&self.root, &["fetch", "--prune", "--quiet"]).map(drop)
    }

    /// File content at a revision. `None` when the revision holds no such path.
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
        let listing = run(&self.root, &["ls-tree", &head_ref(rev), "--", path])?;
        if listing.trim().is_empty() {
            return Ok(None);
        }
        run(&self.root, &["show", &format!("{rev}:{path}")]).map(Some)
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        let refname = head_ref(branch);
        probe(&self.root, &["show-ref", "--verify", "--quiet", &refname])
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
        let log = run(&self.root, &["log", &format, &head_ref(branch), "--"])?;
        Ok(log.lines().find_map(|line| parse_trailer(line, value)))
    }

    pub fn is_ancestor(&self, commit: &str, branch: &str) -> Result<bool> {
        let refname = head_ref(branch);
        probe(
            &self.root,
            &["merge-base", "--is-ancestor", commit, &refname],
        )
    }

    pub fn head(&self) -> Result<String> {
        run(&self.root, &["rev-parse", "HEAD"]).map(|sha| sha.trim().to_string())
    }

    pub fn branch_create(&self, branch: &str, base: &str) -> Result<()> {
        run(
            &self.root,
            &["branch", "--no-track", branch, &head_ref(base)],
        )
        .map(drop)
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

    /// Merge `branch` into the checked-out branch. Fast-forward when it can.
    pub fn merge(&self, branch: &str) -> Result<()> {
        run(&self.root, &["merge", "--no-edit", &head_ref(branch)]).map(drop)
    }

    /// Undo a conflicted merge. Does nothing when no merge is in flight.
    pub fn merge_abort(&self) -> Result<()> {
        if !probe(
            &self.root,
            &["rev-parse", "--verify", "--quiet", MERGE_HEAD],
        )? {
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
            &["worktree", "add", "--detach", &path, &head_ref(rev)],
        )
        .map(drop)
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

/// Full ref name: a branch name alone can also read as a path.
fn head_ref(branch: &str) -> String {
    format!("{HEADS}{branch}")
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
