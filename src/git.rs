use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::{host, lock, paths, proc};

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
/// Tree entry mode of a regular file.
const REGULAR_FILE: &str = "100644";
/// Entry at a worktree root that ties it to its repo.
pub const WORKTREE_LINK: &str = ".git";
/// Admin dir of each worktree, under the bare repo, and its file naming the worktree.
const ADMINS: &str = "worktrees";
const ADMIN_LINK: &str = "gitdir";
const GIT_DIR: &str = "--git-dir=";
const WORK_TREE: &str = "--work-tree=";
const SHARED_REPOSITORY: &str = "core.sharedRepository";
/// Owner reads and writes, group reads, others nothing.
const SHARED_MODE: &str = "0640";
const WORKTREE_FIELD: &str = "worktree ";
const BRANCH_FIELD: &str = "branch ";

/// Sole caller of `git`, on a bare repo or a checkout git finds from its own root.
pub struct Git {
    root: PathBuf,
}

/// Slot worktree. Each call names its admin dir: git never reads the `.git` the
/// row can rewrite there. Only `Git::worktree` makes one.
pub struct Worktree {
    root: PathBuf,
    admin: PathBuf,
}

impl Git {
    /// Create the bare repo at `path`, parents and all.
    pub fn init_bare(path: &Path) -> Result<Self> {
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|why| Error::Failed(format!("directory {}: {why}", parent.display())))?;
        proc::run(
            BIN,
            parent,
            &["init", "--bare", "--quiet", &paths::arg(path)?],
        )?;
        Ok(Self::at(path.to_path_buf()))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Worktree at `tree` this bare repo registers, found from its own admin files.
    /// `None`: no registration names `tree`.
    pub fn worktree(&self, tree: &Path) -> Result<Option<Worktree>> {
        let admins = self.root.join(ADMINS);
        let listing = match fs::read_dir(&admins) {
            Ok(listing) => listing,
            Err(why) if why.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(why) => {
                return Err(Error::Failed(format!(
                    "directory {}: {why}",
                    admins.display()
                )))
            }
        };
        let tree = paths::real(tree);
        for entry in listing {
            let admin = entry
                .map_err(|why| Error::Failed(format!("directory {}: {why}", admins.display())))?
                .path();
            let Ok(link) = fs::read_to_string(admin.join(ADMIN_LINK)) else {
                continue;
            };
            let link = Path::new(link.trim());
            let names = link.file_name().is_some_and(|name| name == WORKTREE_LINK)
                && link.parent().map(paths::real).as_deref() == Some(tree.as_path());
            if names {
                return Ok(Some(Worktree { root: tree, admin }));
            }
        }
        Ok(None)
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

    /// URL `origin` points at. `None` when unset.
    pub fn origin(&self) -> Result<Option<String>> {
        let url = self.lookup(&["config", "--get", ORIGIN_URL])?;
        Ok(url.map(|url| url.trim().to_string()))
    }

    fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.run(&["config", "--replace-all", key, value]).map(drop)
    }

    /// Bare repo alone: every remote branch, pruned.
    pub fn fetch(&self) -> Result<()> {
        let _store = self.hold_refs()?;
        self.remote(&["fetch", "--prune", "--quiet", ORIGIN])
            .map(drop)
    }

    /// Bare repo alone: land refs and fail refs the remote holds.
    pub fn fetch_kept(&self) -> Result<()> {
        let _store = self.hold_refs()?;
        let spec = format!("+{KEPT_SPEC}");
        self.remote(&["fetch", "--quiet", ORIGIN, &spec]).map(drop)
    }

    /// Bare repo alone: remote holds exactly the land and fail refs held here.
    pub fn mirror_kept(&self) -> Result<()> {
        let _store = self.hold_refs()?;
        let args = ["push", "--quiet", "--force", "--prune", ORIGIN, KEPT_SPEC];
        self.remote(&args).map(drop)
    }

    /// Bare repo alone: push `commit` to remote `branch`, no force.
    pub fn push(&self, commit: &str, branch: &str) -> Result<Push> {
        self.push_with(commit, branch, &[])
    }

    /// Bare repo alone: push `commit`, a child of `expected`, to remote `branch` while
    /// the remote holds `expected`. Moved or gone: `Behind`, and the remote stays.
    pub fn push_if(&self, commit: &str, branch: &str, expected: &str) -> Result<Push> {
        let lease = format!("--force-with-lease={}:{expected}", branch_ref(branch));
        self.push_with(commit, branch, &[&lease])
    }

    fn push_with(&self, commit: &str, branch: &str, flags: &[&str]) -> Result<Push> {
        let _store = self.hold_refs()?;
        let spec = format!("{commit}:{}", branch_ref(branch));
        let mut args = vec!["push", "--porcelain"];
        args.extend_from_slice(flags);
        args.extend_from_slice(&[ORIGIN, &spec]);
        let args = bounded(&args);
        let out = self.answer(&args)?;
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
        let listing = self.run(&["for-each-ref", "--format=%(objectname) %(refname)", prefix])?;
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
        let sha = self.lookup(&["rev-parse", "--verify", "--quiet", &peeled])?;
        Ok(sha.map(|sha| sha.trim().to_string()))
    }

    /// True when `rev` holds `path`.
    pub fn has_path(&self, rev: &str, path: &str) -> Result<bool> {
        let listing = self.run(&["ls-tree", rev, "--", path])?;
        Ok(!listing.trim().is_empty())
    }

    /// File content at a revision. `None` when the revision holds no such path.
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
        if !self.has_path(rev, path)? {
            return Ok(None);
        }
        self.run(&["show", &format!("{rev}:{path}")]).map(Some)
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        self.probe(&["show-ref", "--verify", "--quiet", &branch_ref(branch)])
    }

    /// Commit sha `rev` points at. Errors when no such ref.
    pub fn tip(&self, rev: &str) -> Result<String> {
        self.run(&["rev-parse", rev])
            .map(|sha| sha.trim().to_string())
    }

    /// Bare repo alone: commit on `parent` whose tree holds `file` alone, with `text`
    /// as content. Moves no ref.
    pub fn commit_file(
        &self,
        parent: &str,
        file: &str,
        text: &str,
        message: &str,
    ) -> Result<String> {
        let blob = self.input(&["hash-object", "-w", "--stdin"], text)?;
        let entry = format!("{REGULAR_FILE} blob {}\t{file}\n", blob.trim());
        let tree = self.input(&["mktree"], &entry)?;
        let args = ["commit-tree", tree.trim(), "-p", parent, "-m", message];
        self.run(&args).map(|sha| sha.trim().to_string())
    }

    pub fn ref_set(&self, name: &str, commit: &str) -> Result<()> {
        self.run(&["update-ref", name, commit]).map(drop)
    }

    /// Set each `(name, commit)` in one transaction: all or none.
    pub fn refs_set(&self, refs: &[(String, String)]) -> Result<()> {
        let input: String = refs
            .iter()
            .map(|(name, commit)| format!("update {name} {commit}\n"))
            .collect();
        self.input(&["update-ref", "--stdin"], &input).map(drop)
    }

    /// Drop `name`. Does nothing when no such ref.
    pub fn ref_delete(&self, name: &str) -> Result<()> {
        self.run(&["update-ref", "-d", name]).map(drop)
    }

    pub fn is_ancestor(&self, commit: &str, rev: &str) -> Result<bool> {
        self.probe(&["merge-base", "--is-ancestor", commit, rev])
    }

    pub fn branch_delete(&self, branch: &str) -> Result<()> {
        self.run(&["branch", "--delete", "--force", branch])
            .map(drop)
    }

    /// Worktree path holding each checked-out branch. Detached worktrees hold none.
    pub fn checkouts(&self) -> Result<BTreeMap<String, PathBuf>> {
        let listing = self.run(&["worktree", "list", "--porcelain"])?;
        Ok(parse_checkouts(&listing))
    }

    pub fn worktree_add(&self, path: &Path, rev: &str) -> Result<()> {
        self.run(&["worktree", "prune"])?;
        let path = paths::arg(path)?;
        self.run(&["worktree", "add", "--detach", &path, rev])
            .map(drop)
    }

    /// Drop worktree: directory, then registration. Prune reads registration, so
    /// directory goes first.
    pub fn worktree_remove(&self, path: &Path) -> Result<()> {
        if path.exists() {
            fs::remove_dir_all(path)
                .map_err(|why| Error::Failed(format!("worktree {}: {why}", path.display())))?;
        }
        self.run(&["worktree", "prune"]).map(drop)
    }

    /// Files git writes from now on take `SHARED_MODE` whatever the umask; dirs
    /// pass their group on.
    pub fn share(&self) -> Result<()> {
        self.set_config(SHARED_REPOSITORY, SHARED_MODE)
    }

    /// Ref store lock: each fetch and push holds it.
    pub fn hold_refs(&self) -> Result<lock::Hold> {
        lock::hold(&self.root.join(REF_STORE))
    }
}

impl Worktree {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head(&self) -> Result<String> {
        self.run(&["rev-parse", HEAD])
            .map(|sha| sha.trim().to_string())
    }

    /// Drop every local change, then put `branch`, made or moved to `rev`, in the
    /// worktree.
    pub fn checkout_at(&self, branch: &str, rev: &str) -> Result<()> {
        self.run(&["checkout", "--force", "--no-track", "-B", branch, rev])?;
        self.run(&["clean", "--force", "-d"]).map(drop)
    }

    /// Drop every local change, then put `rev` in the worktree on no branch.
    pub fn checkout_detached(&self, rev: &str) -> Result<()> {
        self.run(&["checkout", "--force", "--detach", rev])?;
        self.run(&["clean", "--force", "-d"]).map(drop)
    }

    /// Leave branch at its commit, so another worktree can take it.
    pub fn detach(&self) -> Result<()> {
        self.run(&["checkout", "--detach"]).map(drop)
    }

    /// Merge `rev` into the worktree head, subject naming branch `name`. A merge
    /// commit even where a fast-forward fits: one commit per merged branch.
    pub fn merge(&self, rev: &str, name: &str) -> Result<()> {
        let subject = format!("Merge {name}");
        self.run(&["merge", "--no-ff", "--no-edit", "-m", &subject, rev])
            .map(drop)
    }

    /// True until a merge is committed or aborted.
    pub fn merging(&self) -> Result<bool> {
        self.probe(&["rev-parse", "--verify", "--quiet", MERGE_HEAD])
    }

    /// Undo a conflicted merge. Does nothing when no merge is in flight.
    pub fn merge_abort(&self) -> Result<()> {
        if !self.merging()? {
            return Ok(());
        }
        self.run(&["merge", "--abort"]).map(drop)
    }

    /// Paths the index tracks, relative to the worktree root. Unmerged paths once.
    pub fn tracked(&self) -> Result<BTreeSet<PathBuf>> {
        let listing = self.run(&["ls-files", "-z"])?;
        Ok(listing
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect())
    }

    /// Add every change, commit with the agent's message. No diff: commit empty.
    pub fn commit(&self, message: &str) -> Result<String> {
        self.run(&["add", "--all"])?;
        self.run(&["commit", "--allow-empty", "--message", message])?;
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

impl Git {
    fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<String> {
        proc::run(BIN, &self.root, args)
    }

    fn input(&self, args: &[&str], input: &str) -> Result<String> {
        proc::run_with_input(BIN, &self.root, args, input)
    }

    fn lookup(&self, args: &[&str]) -> Result<Option<String>> {
        proc::lookup(BIN, &self.root, args)
    }

    fn probe(&self, args: &[&str]) -> Result<bool> {
        proc::probe(BIN, &self.root, args)
    }

    fn answer<S: AsRef<OsStr>>(&self, args: &[S]) -> Result<std::process::Output> {
        proc::answer(BIN, &self.root, args)
    }

    fn remote(&self, args: &[&str]) -> Result<String> {
        self.run(&bounded(args))
    }
}

impl Worktree {
    /// `args` after the admin dir and the worktree.
    fn scoped(&self, args: &[&str]) -> Vec<OsString> {
        [flag(GIT_DIR, &self.admin), flag(WORK_TREE, &self.root)]
            .into_iter()
            .chain(args.iter().map(OsString::from))
            .collect()
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        proc::run(BIN, &self.root, &self.scoped(args))
    }

    fn probe(&self, args: &[&str]) -> Result<bool> {
        proc::probe(BIN, &self.root, &self.scoped(args))
    }
}

fn flag(name: &str, path: &Path) -> OsString {
    let mut flag = OsString::from(name);
    flag.push(path);
    flag
}

/// `args` after the transport bounds of `host`.
fn bounded(args: &[&str]) -> Vec<String> {
    let ssh = format!(
        "core.sshCommand=ssh -o ConnectTimeout={} -o ServerAliveInterval={} -o ServerAliveCountMax={}",
        host::SSH_CONNECT,
        host::SSH_ALIVE,
        host::SSH_ALIVE_MISSED
    );
    let speed = format!("http.lowSpeedLimit={}", host::LOW_SPEED);
    let time = format!("http.lowSpeedTime={}", host::LOW_SPEED_TIME);
    ["-c", &ssh, "-c", &speed, "-c", &time]
        .into_iter()
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A row owns the worktree and can point its `.git` at a repo with its own hooks.
    #[test]
    fn host_commit_ignores_a_row_written_git_file() {
        let root = std::env::temp_dir().join(format!("scswarm-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let repo = Git::init_bare(&root.join("repo.git")).expect("bare repo must init");
        for (key, value) in [("user.name", "test"), ("user.email", "test@localhost")] {
            repo.set_config(key, value).expect("identity must set");
        }
        let empty = repo.input(&["mktree"], "").expect("empty tree must write");
        let seed = repo
            .run(&["commit-tree", empty.trim(), "-m", "seed"])
            .expect("seed must commit");
        let path = root.join("tree");
        repo.worktree_add(&path, seed.trim())
            .expect("worktree must add");
        let fake = Git::init_bare(&root.join("fake.git")).expect("row repo must init");
        let hooks = root.join("hooks");
        let marker = root.join("ran");
        fs::create_dir_all(&hooks).expect("hook dir must make");
        let hook = hooks.join("pre-commit");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).expect("hook");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("hook mode");
        fake.set_config("core.bare", "false")
            .expect("row repo must take a worktree");
        fake.set_config("core.hooksPath", &hooks.display().to_string())
            .expect("row hook must set");
        let link = format!("gitdir: {}\n", root.join("fake.git").display());
        fs::write(path.join(WORKTREE_LINK), link).expect("row writes .git");
        fs::write(path.join("work.txt"), "row\n").expect("row writes work");
        let tree = repo
            .worktree(&path)
            .expect("admin dirs must read")
            .expect("bare repo must register the tree");
        let commit = tree.commit("row").expect("host commit must land");
        assert!(!marker.exists(), "host commit must run no row hook");
        assert!(repo.commit_of(&commit).expect("lookup").is_some());
        fs::remove_dir_all(&root).expect("scratch must drop");
    }

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
