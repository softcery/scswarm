use std::collections::BTreeMap;
use std::env;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::host::{Mount, Stamp};
use crate::paths::{self, Project};
use crate::proc;

const BIN: &str = "podman";
const NAME: &str = "scswarm";
/// Tag podman adds to a name that carries none. It reports the full form back.
const LATEST: &str = "latest";
/// Prefix some podman replies put on an image ID.
const DIGEST: &str = "sha256:";
const READ: &str = "ro";
const WRITE: &str = "rw";
/// SELinux hosts deny a container every host label. `z` relabels the bind shared.
const SHARED: &str = "z";
/// Container sleeps; work arrives through `exec`.
const SLEEP: &str = "--entrypoint=sleep";
const FOREVER: &str = "infinity";
/// Reaping PID 1. `sleep` reaps none, so forks stay zombies.
const INIT: &str = "--init";
/// Host file field the container bakes. `serve` reads it back and compares.
const MOUNTS_LABEL: &str = "scswarm.mounts";
const DEVICES_LABEL: &str = "scswarm.devices";

/// What a build or pull gave: image ID, or the last line podman printed on failure.
pub type Made = std::result::Result<String, String>;

/// One long-lived container per slot. Sole owner of podman arguments.
pub struct Container {
    name: String,
}

/// Binds of slot container, plus stamp it carries.
pub struct Spec<'a> {
    pub worktree: &'a Path,
    /// Bare repo: worktree gitdir and common dir sit under it.
    pub repo: &'a Path,
    pub mounts: &'a [Mount],
    pub devices: &'a [String],
    pub stamp: &'a Stamp,
}

impl Container {
    pub fn slot(project: &Project, index: usize) -> Self {
        Self {
            name: name(project, index),
        }
    }

    /// Drop container of this name, create and start from `spec`.
    pub fn create(&self, spec: &Spec) -> Result<()> {
        drop_container(&self.name)?;
        run(&self.run_args(spec)?)
    }

    /// Kill every process, start again; stopped container starts too. Same bake:
    /// no process left by one step reaches next step.
    pub fn restart(&self) -> Result<()> {
        run(&[
            "restart".to_string(),
            "--time=0".to_string(),
            self.name.clone(),
        ])
    }

    /// Worktree rw, bare repo ro, mounts rw, devices by `--device`: a device node takes no relabel.
    fn run_args(&self, spec: &Spec) -> Result<Vec<String>> {
        let worktree = paths::arg(spec.worktree)?;
        let repo = paths::arg(spec.repo)?;
        let mut args = vec![
            "run".to_string(),
            "--detach".to_string(),
            INIT.to_string(),
            format!("--name={}", self.name),
            "--userns=keep-id".to_string(),
            format!("--workdir={worktree}"),
            format!("--label={MOUNTS_LABEL}={}", spec.stamp.mounts),
            format!("--label={DEVICES_LABEL}={}", spec.stamp.devices),
            bind(&worktree, &worktree, WRITE),
            bind(&repo, &repo, READ),
        ];
        for mount in spec.mounts {
            args.push(bind(&mount.host, &mount.guest, WRITE));
        }
        for device in spec.devices {
            args.push(format!("--device={device}"));
        }
        args.push(SLEEP.to_string());
        args.push(spec.stamp.image.clone());
        args.push(FOREVER.to_string());
        Ok(args)
    }

    /// Command running `argv` in container, at host path `dir`, with host
    /// variables `env`.
    pub fn exec(&self, dir: &Path, env: &[String], argv: &[String]) -> Result<Command> {
        let mut command = Command::new(BIN);
        command
            .arg("exec")
            .arg(format!("--workdir={}", paths::arg(dir)?));
        for name in env {
            present(name)?;
            command.arg(format!("--env={name}"));
        }
        command.arg(&self.name).args(argv);
        Ok(command)
    }
}

/// Slot number and stamp per project container, running or not.
pub fn slots(project: &Project) -> Result<BTreeMap<usize, Stamp>> {
    let listing = proc::run(
        BIN,
        here(),
        &[
            "ps".to_string(),
            "--all".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
    )?;
    let held: Vec<Listed> = serde_json::from_str(&listing)
        .map_err(|why| Error::Failed(format!("podman ps: reply must hold json: {why}")))?;
    let prefix = format!("{NAME}-{project}-");
    Ok(held.iter().filter_map(|one| one.slot(&prefix)).collect())
}

/// Stamp the container of one slot carries. `None` when absent.
pub fn stamp(project: &Project, index: usize) -> Result<Option<Stamp>> {
    Ok(slots(project)?.remove(&index))
}

/// Drop the container of one slot. Does nothing when absent.
pub fn remove(project: &Project, index: usize) -> Result<()> {
    drop_container(&name(project, index))
}

pub fn image_exists(image: &str) -> Result<bool> {
    probe(&["image".to_string(), "exists".to_string(), image.to_string()])
}

/// ID of a local image. `None` when absent.
pub fn image_id(image: &str) -> Result<Option<String>> {
    if !image_exists(image)? {
        return Ok(None);
    }
    let id = proc::run(
        BIN,
        here(),
        &[
            "image".to_string(),
            "inspect".to_string(),
            "--format={{.Id}}".to_string(),
            image.to_string(),
        ],
    )?;
    Ok(Some(bare_id(id.trim())))
}

/// Fetch an image from its registry.
pub fn pull(image: &str) -> Result<Made> {
    made(&["pull".to_string(), "--quiet".to_string(), image.to_string()])
}

/// Build a Containerfile. The image takes no tag: a slot runs it by ID.
pub fn build(file: &Path, context: &Path) -> Result<Made> {
    made(&[
        "build".to_string(),
        "--quiet".to_string(),
        format!("--file={}", paths::arg(file)?),
        paths::arg(context)?,
    ])
}

/// Image name in the form podman reports: an untagged name reads as `:latest`.
pub fn image_ref(name: &str) -> String {
    let tagged = name
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains(':'));
    match tagged {
        true => name.to_string(),
        false => format!("{name}:{LATEST}"),
    }
}

/// One container of `podman ps --format json`. Names carry the slot number.
#[derive(Deserialize)]
struct Listed {
    #[serde(rename = "Names")]
    names: Vec<String>,
    #[serde(rename = "ImageID")]
    image: String,
    #[serde(rename = "Labels")]
    labels: Option<BTreeMap<String, String>>,
}

impl Listed {
    /// Slot number and stamp of a project container. Match is text: podman reads
    /// `--filter name=` as regex, so project `a.b` would match `axb` too.
    fn slot(&self, prefix: &str) -> Option<(usize, Stamp)> {
        let index = self
            .names
            .iter()
            .find_map(|name| name.strip_prefix(prefix)?.parse().ok())?;
        let label = |key: &str| {
            self.labels
                .as_ref()
                .and_then(|held| held.get(key))
                .cloned()
                .unwrap_or_default()
        };
        let stamp = Stamp {
            image: bare_id(&self.image),
            mounts: label(MOUNTS_LABEL),
            devices: label(DEVICES_LABEL),
        };
        Some((index, stamp))
    }
}

/// Build or pull: `--quiet` prints the image ID last on stdout.
fn made(args: &[String]) -> Result<Made> {
    let out = proc::answer(BIN, here(), args)?;
    if !out.status.success() {
        return Ok(Err(proc::last_line(&out)));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(Ok(bare_id(
        stdout.lines().last().unwrap_or_default().trim(),
    )))
}

/// Image ID with no digest prefix. Both sides of a stamp pass here.
fn bare_id(id: &str) -> String {
    id.strip_prefix(DIGEST).unwrap_or(id).to_string()
}

fn name(project: &Project, index: usize) -> String {
    format!("{NAME}-{project}-{index}")
}

fn drop_container(name: &str) -> Result<()> {
    run(&[
        "rm".to_string(),
        "--force".to_string(),
        "--ignore".to_string(),
        name.to_string(),
    ])
}

/// Value stays on host: podman reads variable from own environment.
fn present(name: &str) -> Result<()> {
    if env::var_os(name).is_none() {
        return Err(Error::Failed(format!(
            "environment variable {name}, named in the host file field `env`, must hold a value"
        )));
    }
    Ok(())
}

/// podman takes paths as arguments; working dir holds no meaning.
fn here() -> &'static Path {
    Path::new(".")
}

fn bind(host: &str, guest: &str, mode: &str) -> String {
    format!("--volume={host}:{guest}:{mode},{SHARED}")
}

fn run(args: &[String]) -> Result<()> {
    proc::run(BIN, here(), args).map(drop)
}

fn probe(args: &[String]) -> Result<bool> {
    proc::probe(BIN, here(), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_untagged_name_as_latest() {
        assert_eq!(image_ref("localhost/hex"), "localhost/hex:latest");
        assert_eq!(image_ref("docker.io/a/b:1.2"), "docker.io/a/b:1.2");
        assert_eq!(image_ref("reg.example:5000/b"), "reg.example:5000/b:latest");
    }

    #[test]
    fn reaps_with_init_and_binds_worktree_rw_repo_ro() {
        let project = Project::try_from("alpha").expect("name must parse");
        let container = Container::slot(&project, 2);
        let mounts = [Mount {
            host: "/records".to_string(),
            guest: "/mnt".to_string(),
        }];
        let stamp = Stamp {
            image: "swarm:latest".to_string(),
            mounts: "[\"/records:/mnt\"]".to_string(),
            devices: "[\"/dev/dri\"]".to_string(),
        };
        let args = container
            .run_args(&Spec {
                worktree: Path::new("/slot/2"),
                repo: Path::new("/repo.git"),
                mounts: &mounts,
                devices: &["/dev/dri".to_string()],
                stamp: &stamp,
            })
            .expect("utf-8 paths must build arguments");
        let holds = |arg: &str| args.iter().any(|held| held == arg);
        assert!(holds("--name=scswarm-alpha-2"), "{args:?}");
        assert!(holds(INIT), "{args:?}");
        assert!(holds("--volume=/slot/2:/slot/2:rw,z"), "{args:?}");
        assert!(holds("--volume=/repo.git:/repo.git:ro,z"), "{args:?}");
        assert!(holds("--volume=/records:/mnt:rw,z"), "{args:?}");
        assert!(holds("--device=/dev/dri"), "{args:?}");
        assert!(
            holds("--label=scswarm.mounts=[\"/records:/mnt\"]"),
            "{args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some(FOREVER));
    }
}
