use std::env;
use std::path::Path;
use std::process::Command;

use crate::config::{self, Mount};
use crate::error::{Error, Result};
use crate::{paths, proc};

const BIN: &str = "podman";
const NAME: &str = "sercswarm";
/// Host image store. Keeps built image apart from remote image of same name.
const LOCAL: &str = "localhost";
const READ: &str = "ro";
const WRITE: &str = "rw";
/// SELinux hosts deny a container every host label. `z` relabels the bind shared.
const SHARED: &str = "z";
/// Container sleeps; work arrives through `exec`.
const SLEEP: &str = "--entrypoint=sleep";
const FOREVER: &str = "infinity";

/// One long-lived container per slot. Sole owner of podman arguments.
pub struct Container {
    name: String,
    env: Vec<String>,
}

/// Image and binds of a slot container.
pub struct Spec<'a> {
    pub image: &'a str,
    pub worktree: &'a Path,
    /// Bare repo: worktree gitdir and common dir sit under it.
    pub repo: &'a Path,
    pub mounts: &'a [Mount],
}

impl Container {
    pub fn slot(project: &str, index: usize, env: &[String]) -> Self {
        Self {
            name: format!("{NAME}-{}-{index}", slug(project)),
            env: env.to_vec(),
        }
    }

    /// Drop container of this name, create and start from `spec`.
    pub fn create(&self, spec: &Spec) -> Result<()> {
        run(&[
            "rm".to_string(),
            "--force".to_string(),
            "--ignore".to_string(),
            self.name.clone(),
        ])?;
        run(&self.run_args(spec)?)
    }

    pub fn start(&self) -> Result<()> {
        let exists = probe(&[
            "container".to_string(),
            "exists".to_string(),
            self.name.clone(),
        ])?;
        if !exists {
            return Err(Error::Failed(format!(
                "container {} is absent: run `sercswarm init`",
                self.name
            )));
        }
        run(&["start".to_string(), self.name.clone()])
    }

    /// Worktree rw, bare repo ro, project mounts rw. Every path is the host path.
    fn run_args(&self, spec: &Spec) -> Result<Vec<String>> {
        let worktree = paths::arg(spec.worktree)?;
        let repo = paths::arg(spec.repo)?;
        let mut args = vec![
            "run".to_string(),
            "--detach".to_string(),
            format!("--name={}", self.name),
            "--userns=keep-id".to_string(),
            format!("--workdir={worktree}"),
            bind(&worktree, &worktree, WRITE),
            bind(&repo, &repo, READ),
        ];
        for mount in spec.mounts {
            args.push(bind(&paths::expand(&mount.host)?, &mount.guest, WRITE));
        }
        args.push(SLEEP.to_string());
        args.push(spec.image.to_string());
        args.push(FOREVER.to_string());
        Ok(args)
    }

    /// Command running `argv` in container, at host path `dir`.
    pub fn exec(&self, dir: &Path, argv: &[String]) -> Result<Command> {
        let mut command = Command::new(BIN);
        command
            .arg("exec")
            .arg(format!("--workdir={}", paths::arg(dir)?));
        for name in &self.env {
            present(name)?;
            command.arg(format!("--env={name}"));
        }
        command.arg(&self.name).args(argv);
        Ok(command)
    }
}

/// Build project Containerfile. Output streams to operator.
pub fn build(tag: &str, file: &Path, context: &Path) -> Result<()> {
    let args = [
        "build".to_string(),
        format!("--tag={tag}"),
        format!("--file={}", paths::arg(file)?),
        paths::arg(context)?,
    ];
    proc::stream(BIN, here(), &args)
}

/// Image tag for project that ships a Containerfile.
pub fn tag(project: &str) -> String {
    format!("{LOCAL}/{NAME}/{}", slug(project))
}

/// Value stays on host: podman reads variable from own environment.
fn present(name: &str) -> Result<()> {
    if env::var_os(name).is_none() {
        return Err(Error::Failed(format!(
            "environment variable {name}, named in {} field `env`, must hold a value",
            config::FILE
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

fn slug(project: &str) -> String {
    project.to_lowercase()
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
    fn binds_worktree_rw_and_repo_ro() {
        let container = Container::slot("Alpha", 2, &[]);
        let mounts = [Mount {
            host: "/records".to_string(),
            guest: "/mnt".to_string(),
        }];
        let args = container
            .run_args(&Spec {
                image: "swarm:latest",
                worktree: Path::new("/slot/2"),
                repo: Path::new("/repo.git"),
                mounts: &mounts,
            })
            .expect("utf-8 paths must build arguments");
        let holds = |arg: &str| args.iter().any(|held| held == arg);
        assert!(holds("--name=sercswarm-alpha-2"), "{args:?}");
        assert!(holds("--volume=/slot/2:/slot/2:rw,z"), "{args:?}");
        assert!(holds("--volume=/repo.git:/repo.git:ro,z"), "{args:?}");
        assert!(holds("--volume=/records:/mnt:rw,z"), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some(FOREVER));
    }
}
