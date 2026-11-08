use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use clap::ValueEnum;
use serde::Deserialize;

use crate::credential::Credential;
use crate::error::Result;
use crate::git::{Git, Worktree};
use crate::host::Mount;
use crate::namespace::Namespace;
use crate::paths::Project;
use crate::podman::{self, Container};
use crate::proc::Stop;
use crate::step::Outcome;

/// Runner as the host file and `scswarm work` name it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Podman,
    Namespace,
}

/// How each slot runs its steps. The host file picks one per `serve` cycle, so
/// every slot of one swarm has one sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runner {
    /// One rootless container per slot.
    Podman,
    /// One uid per slot, each try in its own PID namespace.
    Namespace(Namespace),
}

/// What one row asks of its slot.
pub struct Request<'a> {
    pub repo: &'a Git,
    pub base: &'a str,
    pub project: &'a Project,
    pub image: Option<&'a str>,
    pub mounts: &'a [Mount],
    pub devices: &'a [String],
}

/// Row request, with the slot worktree that serves it.
pub struct Need<'a> {
    pub request: &'a Request<'a>,
    pub tree: &'a Path,
    /// Worktree made for this row: the slot had none, or held another project.
    pub fresh: bool,
}

/// Track run, skipped, or run with a field reported unread.
#[derive(Debug, PartialEq, Eq)]
pub enum Admit {
    Run,
    Skip(&'static str),
    Ignore(&'static str),
}

impl Runner {
    /// `uid`: first slot uid, `namespace` alone. Inner `Err`: field at fault.
    pub fn new(kind: Kind, uid: Option<u32>) -> std::result::Result<Self, String> {
        match (kind, uid) {
            (Kind::Podman, None) => Ok(Self::Podman),
            (Kind::Podman, Some(_)) => {
                Err("field `uid` must stay absent under runner `podman`".to_string())
            }
            (Kind::Namespace, None) => {
                Err("field `uid` must hold the first slot uid under runner `namespace`".to_string())
            }
            (Kind::Namespace, Some(0)) => {
                Err("field `uid` must be 1 or more: uid 0 is root".to_string())
            }
            (Kind::Namespace, Some(uid)) => Ok(Self::Namespace(Namespace::new(uid))),
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::Podman => Kind::Podman,
            Self::Namespace(_) => Kind::Namespace,
        }
    }

    /// First slot uid under `namespace`.
    pub fn uid(&self) -> Option<u32> {
        match self {
            Self::Podman => None,
            Self::Namespace(namespace) => Some(namespace.uid()),
        }
    }

    /// Runner of one row, in project group `gid`.
    pub fn serving(self, gid: Option<u32>) -> Self {
        match (self, gid) {
            (Self::Namespace(namespace), Some(gid)) => Self::Namespace(namespace.serving(gid)),
            (runner, _) => runner,
        }
    }

    /// Host runs each slot below `workers` and each group in `gids`.
    pub fn check(&self, workers: usize, gids: &[u32]) -> Result<()> {
        match self {
            Self::Podman => Ok(()),
            Self::Namespace(namespace) => namespace.check(workers, gids),
        }
    }

    pub fn admits(&self, image: Option<&str>) -> Admit {
        match (self, image) {
            (Self::Podman, None) => Admit::Skip(
                "`scswarm.yaml` field `image` must name a Containerfile path or an image: runner `podman` reads it",
            ),
            (Self::Namespace(_), Some(_)) => {
                Admit::Ignore("`scswarm.yaml` field `image` goes unread: runner `namespace` runs no image")
            }
            _ => Admit::Run,
        }
    }

    /// Slot ready for a row: leftovers gone, the row's view in place. Spent: the row's fault.
    /// `stop` cuts an image build or pull short.
    pub fn prepare(&self, slot: usize, need: &Need, stop: &Stop) -> Result<Outcome<()>> {
        match self {
            Self::Podman => podman::prepare(slot, need, stop),
            Self::Namespace(namespace) => namespace.prepare(slot, need).map(Outcome::Done),
        }
    }

    pub fn exec(
        &self,
        slot: usize,
        dir: &Path,
        credentials: &[Credential],
        argv: &[String],
    ) -> Result<Command> {
        match self {
            Self::Podman => Container::slot(slot).exec(dir, credentials, argv),
            Self::Namespace(namespace) => namespace.exec(slot, dir, credentials, argv),
        }
    }

    pub fn reset(&self, slot: usize) -> Result<()> {
        match self {
            Self::Podman => Container::slot(slot).restart(),
            Self::Namespace(namespace) => namespace.reset(slot),
        }
    }

    /// Files host git wrote, given to the slot.
    pub fn handoff(&self, slot: usize, tree: &Worktree) -> Result<()> {
        match self {
            Self::Podman => Ok(()),
            Self::Namespace(namespace) => namespace.handoff(slot, tree),
        }
    }

    /// Slots with runner state outside slot dirs.
    pub fn held(&self) -> Result<BTreeSet<usize>> {
        match self {
            Self::Podman => Ok(podman::slots()?.into_keys().collect()),
            Self::Namespace(_) => Ok(BTreeSet::new()),
        }
    }

    pub fn remove(&self, slot: usize) -> Result<()> {
        match self {
            Self::Podman => podman::remove(slot),
            Self::Namespace(namespace) => namespace.reset(slot),
        }
    }
}
