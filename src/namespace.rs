use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{chown, lchown, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::credential::Credential;
use crate::error::{Error, Result};
use crate::event;
use crate::git::{Git, Worktree};
use crate::paths;
use crate::proc;
use crate::runner::Need;

const SETPRIV: &str = "setpriv";
/// Own user namespace mapping the slot uid alone: the row gains no capability.
/// Own PID namespace: its init dies with `unshare`, and every process of the try with it.
const UNSHARE: [&str; 6] = [
    "unshare",
    "--user",
    "--map-current-user",
    "--pid",
    "--fork",
    "--kill-child",
];
/// Signal every process the slot uid owns but the caller.
const SWEEP: [&str; 4] = ["kill", "-KILL", "--", "-1"];
const PROBE: &str = "true";
const PATH: &str = "PATH";
const PATH_FALLBACK: &str = "/usr/local/bin:/usr/bin:/bin";
const HOME: &str = "HOME";
const TMPDIR: &str = "TMPDIR";
/// Git refuses a worktree whose admin dir another uid owns, unless told it is safe.
const GIT_CONFIG: [(&str, &str); 2] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "safe.directory"),
];
const GIT_CONFIG_VALUE: &str = "GIT_CONFIG_VALUE_0";
/// Owner and process info of the caller.
const SELF: &str = "/proc/self";
/// Slot uids pass through, list nothing.
const TRAVERSE: u32 = 0o711;
const PRIVATE: u32 = 0o700;
/// Bare repo dirs: project group reads, its new entries take the group. Others nothing.
const GROUP_DIR: u32 = 0o2750;
/// Bare repo root while its group changes: the done mark is `GROUP_DIR`.
const GROUP_PENDING: u32 = 0o2700;
const MODE_BITS: u32 = 0o7777;

/// Slot `n` runs as uid `uid + n`, in the group of its row's project. Sole owner of
/// `setpriv` and `unshare` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    uid: u32,
    /// Project group of the row. `serve` holds none: it runs no row.
    gid: Option<u32>,
}

impl Namespace {
    pub fn new(uid: u32) -> Self {
        Self { uid, gid: None }
    }

    /// First slot uid.
    pub fn uid(&self) -> u32 {
        self.uid
    }

    pub fn serving(self, gid: u32) -> Self {
        Self {
            gid: Some(gid),
            ..self
        }
    }

    /// Linux alone. No slot uid and no project group is the caller's own. Each slot
    /// uid below `workers` runs a try, slot 0 in each project group.
    pub fn check(&self, workers: usize, gids: &[u32]) -> Result<()> {
        if !cfg!(target_os = "linux") {
            return Err(Error::Invalid(
                "host file field `runner` must name `podman` off Linux: `namespace` needs Linux namespaces"
                    .to_string(),
            ));
        }
        let caller = fs::metadata(SELF).map_err(|why| failed(Path::new(SELF), &why))?;
        let uids = (0..workers)
            .map(|slot| self.slot_uid(slot))
            .collect::<Result<Vec<u32>>>()?;
        if uids.contains(&caller.uid()) {
            return Err(Error::Invalid(format!(
                "host file field `uid` plus `workers` must leave out uid {} of `serve`",
                caller.uid()
            )));
        }
        if gids.contains(&caller.gid()) {
            return Err(Error::Invalid(format!(
                "host file field `projects.<name>.gid` must differ from gid {} of `serve`",
                caller.gid()
            )));
        }
        let own = uids.iter().enumerate().map(|(slot, &uid)| (slot, uid));
        for (slot, gid) in own.chain(gids.iter().map(|&gid| (0, gid))) {
            let out = self.run(slot, gid, Path::new("/"), &[PROBE])?;
            if !out.status.success() {
                return Err(Error::Invalid(self.refused(slot, "must run a try", &out)));
            }
        }
        Ok(())
    }

    /// Leftovers killed, slot dirs made with their modes, bare repo given to the
    /// project group, worktree root to the slot, then a try that reads the base.
    pub fn prepare(&self, slot: usize, need: &Need) -> Result<()> {
        let (uid, gid) = (self.slot_uid(slot)?, self.gid()?);
        self.reset(slot)?;
        for dir in [paths::swarm()?, paths::slots()?, paths::slot(slot)?] {
            mode(&dir, TRAVERSE)?;
        }
        let logs = paths::logs()?;
        make(&logs)?;
        mode(&logs, PRIVATE)?;
        regroup(need.request.repo, gid)?;
        let home = paths::slot_home(slot)?;
        let made = need.fresh || !home.exists();
        for dir in [home, paths::slot_tmp(slot)?] {
            if need.fresh && dir.exists() {
                fs::remove_dir_all(&dir).map_err(|why| failed(&dir, &why))?;
            }
            make(&dir)?;
            own(&dir, uid, gid)?;
            mode(&dir, PRIVATE)?;
        }
        own(need.tree, uid, gid)?;
        mode(need.tree, PRIVATE)?;
        let repo = format!("--git-dir={}", paths::arg(need.request.repo.root())?);
        let base = format!("{}^{{commit}}", need.request.base);
        let out = self.run(
            slot,
            gid,
            need.tree,
            &["git", &repo, "cat-file", "-e", &base],
        )?;
        if !out.status.success() {
            let what = format!(
                "in gid {gid} must read the base in bare repo {}, each dir above it granting others `x`",
                need.request.repo.root().display()
            );
            return Err(Error::Failed(self.refused(slot, &what, &out)));
        }
        if made {
            event::create(slot);
        }
        Ok(())
    }

    /// `argv` as the slot uid in the project group, in fresh namespaces, at `dir`.
    /// Env cleared: `PATH`, slot home and temp dir, git safe dir, then the credentials.
    pub fn exec(
        &self,
        slot: usize,
        dir: &Path,
        credentials: &[Credential],
        argv: &[String],
    ) -> Result<Command> {
        let mut command = self.command(slot, self.gid()?, dir, argv)?;
        for credential in credentials {
            command.env(&credential.name, &credential.value);
        }
        Ok(command)
    }

    /// Kill every process of the slot uid: a worker that died hard leaves its try's.
    pub fn reset(&self, slot: usize) -> Result<()> {
        let uid = self.slot_uid(slot)?.to_string();
        let mut command = Command::new(SETPRIV);
        command
            .args(identity(&uid, &uid))
            .args(SWEEP)
            .current_dir("/")
            .env_clear()
            .env(PATH, path());
        let out = output(&mut command)?;
        match out.status.code() {
            Some(0 | 1) => Ok(()),
            _ => Err(Error::Failed(self.refused(slot, "must take a kill", &out))),
        }
    }

    /// Tracked paths, their dirs and the worktree root, owned by the slot uid.
    pub fn handoff(&self, slot: usize, tree: &Worktree) -> Result<()> {
        let (uid, gid) = (self.slot_uid(slot)?, self.gid()?);
        let mut held: BTreeSet<PathBuf> = BTreeSet::new();
        for file in tree.tracked()? {
            held.extend(file.ancestors().map(Path::to_path_buf));
        }
        for path in held {
            let path = tree.root().join(path);
            match lchown(&path, Some(uid), Some(gid)) {
                Err(why) if why.kind() != io::ErrorKind::NotFound => {
                    return Err(failed(&path, &why));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn command(
        &self,
        slot: usize,
        gid: u32,
        dir: &Path,
        argv: &[impl AsRef<str>],
    ) -> Result<Command> {
        let (uid, gid) = (self.slot_uid(slot)?.to_string(), gid.to_string());
        let mut command = Command::new(SETPRIV);
        command
            .args(identity(&uid, &gid))
            .arg("--no-new-privs")
            .args(UNSHARE)
            .args(argv.iter().map(AsRef::as_ref))
            .current_dir(dir)
            .env_clear()
            .env(PATH, path())
            .env(HOME, paths::slot_home(slot)?)
            .env(TMPDIR, paths::slot_tmp(slot)?)
            .envs(GIT_CONFIG)
            .env(GIT_CONFIG_VALUE, paths::tree(slot)?);
        Ok(command)
    }

    fn run(&self, slot: usize, gid: u32, dir: &Path, argv: &[&str]) -> Result<Output> {
        output(&mut self.command(slot, gid, dir, argv)?)
    }

    fn slot_uid(&self, slot: usize) -> Result<u32> {
        u32::try_from(slot)
            .ok()
            .and_then(|slot| self.uid.checked_add(slot))
            .ok_or_else(|| {
                Error::Failed(format!(
                    "slot {slot}: uid must fit 32 bits from first uid {}",
                    self.uid
                ))
            })
    }

    fn gid(&self) -> Result<u32> {
        self.gid.ok_or_else(|| {
            Error::Failed("runner `namespace`: `scswarm work` must take `--gid`".to_string())
        })
    }

    /// Slot, uid, what failed, last line printed.
    fn refused(&self, slot: usize, what: &str, out: &Output) -> String {
        let uid = self
            .slot_uid(slot)
            .map_or_else(|_| "?".to_string(), |uid| uid.to_string());
        let said = proc::last_line(out);
        let why = match said.is_empty() {
            true => out.status.to_string(),
            false => said,
        };
        format!("runner `namespace` slot {slot}: uid {uid} {what}: {why}")
    }
}

/// Bare repo readable by group `gid` alone, under the ref store lock: no fetch
/// writes mid-walk. Root mode marks it done; a walk cut short runs again.
fn regroup(repo: &Git, gid: u32) -> Result<()> {
    let root = repo.root();
    let meta = fs::metadata(root).map_err(|why| failed(root, &why))?;
    if meta.gid() == gid && meta.mode() & MODE_BITS == GROUP_DIR {
        return Ok(());
    }
    let _store = repo.hold_refs()?;
    repo.share()?;
    lchown(root, None, Some(gid)).map_err(|why| failed(root, &why))?;
    mode(root, GROUP_PENDING)?;
    regroup_entries(root, gid)?;
    mode(root, GROUP_DIR)
}

/// Each entry under `dir` to group `gid`: dirs `GROUP_DIR` before their entries,
/// files readable by the group where the owner reads. Entries git drops mid-walk skip.
fn regroup_entries(dir: &Path, gid: u32) -> Result<()> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(why) if why.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(why) => return Err(failed(dir, &why)),
    };
    for entry in listing {
        let path = entry.map_err(|why| failed(dir, &why))?.path();
        let gone = |why: &io::Error| why.kind() == io::ErrorKind::NotFound;
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(why) if gone(&why) => continue,
            Err(why) => return Err(failed(&path, &why)),
        };
        match lchown(&path, None, Some(gid)) {
            Err(why) if gone(&why) => continue,
            Err(why) => return Err(failed(&path, &why)),
            Ok(()) => {}
        }
        let bits = match meta.file_type() {
            kind if kind.is_symlink() => continue,
            kind if kind.is_dir() => GROUP_DIR,
            _ => (meta.mode() & 0o700) | ((meta.mode() & 0o500) >> 3),
        };
        match fs::set_permissions(&path, fs::Permissions::from_mode(bits)) {
            Err(why) if gone(&why) => continue,
            Err(why) => return Err(failed(&path, &why)),
            Ok(()) => {}
        }
        if meta.is_dir() {
            regroup_entries(&path, gid)?;
        }
    }
    Ok(())
}

/// `setpriv` arguments that switch to `uid` and `gid` and drop every other group.
fn identity<'a>(uid: &'a str, gid: &'a str) -> [&'a str; 5] {
    ["--reuid", uid, "--regid", gid, "--clear-groups"]
}

fn path() -> OsString {
    env::var_os(PATH).unwrap_or_else(|| PATH_FALLBACK.into())
}

fn output(command: &mut Command) -> Result<Output> {
    command.output().map_err(|why| match why.kind() {
        io::ErrorKind::NotFound => Error::Failed(format!("{SETPRIV} must sit on PATH")),
        _ => Error::Failed(format!("{SETPRIV}: {why}")),
    })
}

fn make(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|why| failed(dir, &why))
}

fn own(path: &Path, uid: u32, gid: u32) -> Result<()> {
    chown(path, Some(uid), Some(gid)).map_err(|why| failed(path, &why))
}

fn mode(path: &Path, bits: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(bits)).map_err(|why| failed(path, &why))
}

fn failed(path: &Path, why: &io::Error) -> Error {
    Error::Failed(format!("{}: {why}", path.display()))
}
