use std::fs;
use std::io;
use std::os::fd::AsFd;

use clap::Args;

use crate::board::{self, Row, Track};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::event;
use crate::git::{self, Git, Push};
use crate::host::{Mount, Stamp};
use crate::lock;
use crate::paths::{self, Project};
use crate::podman::{self, Made};
use crate::script::Session;
use crate::slot::Slot;
use crate::state::Key;
use crate::step::Outcome;
use crate::transcript::Transcript;

pub const COMMAND: &str = "work";
const SLOT: &str = "slot";
const TRACK: &str = "track";
const ROW: &str = "row";
const TRACK_TIP: &str = "track-tip";
const BASE: &str = "base";
const ENV: &str = "env";
const MOUNT: &str = "mount";
const DEVICE: &str = "device";

/// One row attempt on one slot. Argv of `scswarm work`, row lock on stdin: old
/// `serve` spawns new binary across `cargo install`, so a change needs a release note.
#[derive(Args, Debug, PartialEq, Eq)]
pub struct Job {
    #[arg(long = SLOT)]
    pub slot: usize,
    #[arg(long = TRACK, value_name = "NAME")]
    pub track: String,
    #[arg(long = ROW, value_name = "ID")]
    pub id: String,
    /// Track branch commit: the track file, and the tip a failure marks.
    #[arg(long = TRACK_TIP, value_name = "SHA")]
    pub track_tip: String,
    /// `to` tip: config, image, branch base.
    #[arg(long = BASE, value_name = "SHA")]
    pub base: String,
    /// Host variables each exec gets, values inherited.
    #[arg(long = ENV, value_name = "NAME")]
    pub env: Vec<String>,
    #[arg(long = MOUNT, value_name = "HOST:GUEST", value_parser = mount)]
    pub mounts: Vec<Mount>,
    /// Host device nodes, resolved.
    #[arg(long = DEVICE, value_name = "PATH")]
    pub devices: Vec<String>,
}

/// Row lock from stdin, slot lock once its last holder ends. `Ok`: row pushed or
/// fail refs written. `Err`: machine fault, no mark.
pub fn work(repo: &Git, project: &Project, job: &Job) -> Result<()> {
    let key = job.key();
    let _row = lock::adopt(io::stdin().as_fd(), &paths::transcripts(project, &key)?)?;
    let dir = paths::slot(project, job.slot)?;
    fs::create_dir_all(&dir)
        .map_err(|why| Error::Failed(format!("directory {}: {why}", dir.display())))?;
    let _slot = lock::hold(&dir)?;
    let track = Track::at(repo, &job.track, &job.track_tip)?;
    let config = Config::at(repo, &job.base)?;
    let board = track.board(&config)?;
    let row = board.row(&job.id).ok_or_else(|| {
        Error::Invalid(format!(
            "argument `--{ROW}` must name a row of {} at `--{TRACK_TIP}`",
            board::FILE
        ))
    })?;
    Attempt {
        slot: &Slot::open(project, job.slot)?,
        repo,
        project,
        track: &track,
        config: &config,
        row,
        job,
        key: &key,
        against: job.base.clone(),
    }
    .run()
}

impl Job {
    pub fn key(&self) -> Key {
        Key::new(&self.track, &self.id)
    }

    pub fn argv(&self, project: &Project) -> Vec<String> {
        let mut argv = vec![COMMAND.to_string(), project.to_string()];
        let mut flag = |name: &str, value: String| argv.extend([format!("--{name}"), value]);
        flag(SLOT, self.slot.to_string());
        flag(TRACK, self.track.clone());
        flag(ROW, self.id.clone());
        flag(TRACK_TIP, self.track_tip.clone());
        flag(BASE, self.base.clone());
        self.env.iter().for_each(|name| flag(ENV, name.clone()));
        self.mounts
            .iter()
            .for_each(|bind| flag(MOUNT, bind.clone().into()));
        self.devices
            .iter()
            .for_each(|node| flag(DEVICE, node.clone()));
        argv
    }
}

/// Everything one attempt reads, plus the `to` tip it last ran against.
struct Attempt<'a> {
    slot: &'a Slot,
    repo: &'a Git,
    project: &'a Project,
    track: &'a Track,
    config: &'a Config,
    row: &'a Row,
    job: &'a Job,
    key: &'a Key,
    against: String,
}

impl Attempt<'_> {
    /// Release before the outcome: no fault leaves the branch held. A fault of the
    /// attempt outranks one of the release. Then the fail refs of a spent attempt.
    fn run(mut self) -> Result<()> {
        let outcome = self.attempt();
        let released = self.slot.release();
        let outcome = outcome.map_err(|why| Error::Failed(format!("row {}: {why}", self.key)))?;
        released?;
        match outcome {
            Outcome::Done(()) => Ok(()),
            Outcome::Spent(why) => self.fail(&why),
        }
    }

    /// Image, container, the row session unless a land ref stands, then the merge
    /// into `to`. Podman answers before the build, so a failed build is the row's.
    fn attempt(&mut self) -> Result<Outcome<()>> {
        let (slot, job, key) = (self.slot, self.job, self.key);
        let standing = slot.stamp()?;
        let image = match self.image()? {
            Ok(image) => image,
            Err(why) => return Ok(Outcome::Spent(format!("image must build: {why}"))),
        };
        let stamp = Stamp::new(&image, &job.mounts, &job.devices)?;
        slot.prepare(
            self.repo,
            &job.base,
            &stamp,
            standing,
            &job.mounts,
            &job.devices,
        )?;
        let mut session = Session::open(slot, &job.env, self.config, self.row, key)?;
        let land = match self.repo.commit_of(&key.land_ref())? {
            Some(land) => land,
            None => {
                slot.checkout(&key.branch(), &job.base)?;
                match session.play(&self.transcript(paths::RUN_LOG)?)? {
                    Outcome::Done(land) => {
                        event::land(key);
                        self.repo.ref_set(&key.land_ref(), &land)?;
                        land
                    }
                    Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
                }
            }
        };
        self.deliver(&mut session, &land)
    }

    /// Merge the row into the `to` tip, push to `to` with no force, again onto the
    /// new tip after a fetch when behind. `serve` pushes the refs after.
    fn deliver(&mut self, session: &mut Session, land: &str) -> Result<Outcome<()>> {
        let (slot, repo, key, to) = (self.slot, self.repo, self.key, &self.track.to);
        let tracking = git::remote_branch(to);
        let branch = key.branch();
        let transcript = self.transcript(paths::MERGE_LOG)?;
        let mut resolve = || session.resolve(to, &transcript);
        let head = loop {
            self.against = repo.tip(&tracking)?;
            if let Outcome::Spent(why) =
                slot.merge_onto(&self.against, land, &branch, &mut resolve)?
            {
                return Ok(Outcome::Spent(why));
            }
            let head = slot.head()?;
            match repo.push(&head, to)? {
                Push::Done => break head,
                Push::Declined(why) => {
                    return Ok(Outcome::Spent(format!("push to `{to}` must land: {why}")));
                }
                Push::Behind => repo.fetch()?,
            }
            if repo.tip(&tracking)? == self.against {
                return Err(Error::Failed(format!(
                    "push to `{to}` must land: the remote refused it, a fetch brought no new commit"
                )));
            }
        };
        for name in &key.fail_refs() {
            repo.ref_delete(name)?;
        }
        event::push(key, to, &head);
        Ok(Outcome::Done(()))
    }

    /// Track tip and `to` tip of this attempt.
    fn fail(&self, why: &str) -> Result<()> {
        event::fail(self.key, why);
        let [on_track, on_to] = self.key.fail_refs();
        self.repo.ref_set(&on_track, &self.job.track_tip)?;
        self.repo.ref_set(&on_to, &self.against)
    }

    /// Containerfile at the base builds from a throwaway worktree of it, free of
    /// untracked files. Anything else names an image.
    fn image(&self) -> Result<Made> {
        let (repo, job, image) = (self.repo, self.job, &self.config.image);
        if !repo.has_path(&job.base, image)? {
            let name = podman::image_ref(image);
            return match podman::image_id(&name)? {
                Some(id) => Ok(Ok(id)),
                None => podman::pull(&name),
            };
        }
        let tree = paths::build(self.project, job.slot)?;
        repo.worktree_remove(&tree)?;
        repo.worktree_add(&tree, &job.base)?;
        let built = podman::build(&tree.join(image), &tree);
        repo.worktree_remove(&tree)?;
        built
    }

    fn transcript(&self, name: &str) -> Result<Transcript> {
        Ok(Transcript::at(paths::transcript(
            self.project,
            self.key,
            name,
        )?))
    }
}

fn mount(text: &str) -> std::result::Result<Mount, String> {
    Mount::try_from(text.to_string())
}
