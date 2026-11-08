use std::fs;
use std::io;
use std::os::fd::AsFd;

use clap::{Args, ValueEnum};

use crate::board::{self, Row, Track};
use crate::config::Config;
use crate::credential::{self, Binding};
use crate::error::{Error, Result};
use crate::event;
use crate::git::{self, Git, Push};
use crate::host::Mount;
use crate::lock;
use crate::paths::{self, Project};
use crate::proc::Stop;
use crate::runner::{Kind, Request, Runner};
use crate::script::{Played, Session};
use crate::slot::Slot;
use crate::state::{Address, Key};
use crate::step::Outcome;
use crate::stop::{self, Watch};
use crate::transcript::{Entry, Transcript};

pub const COMMAND: &str = "work";
const SLOT: &str = "slot";
const TRACK: &str = "track";
const TRACK_BRANCH: &str = "track-branch";
const ROW: &str = "row";
const TRACK_TIP: &str = "track-tip";
const BASE: &str = "base";
const ENV: &str = "env";
const MOUNT: &str = "mount";
const DEVICE: &str = "device";
const RUNNER: &str = "runner";
const UID: &str = "uid";
const GID: &str = "gid";

/// One row attempt on one slot. Argv of `scswarm work`, row lock on stdin: old
/// `serve` spawns new binary after upgrade, so change needs release note.
#[derive(Args, Debug, PartialEq, Eq)]
pub struct Job {
    pub project: Project,
    #[arg(long = SLOT)]
    pub slot: usize,
    #[arg(long = TRACK, value_name = "NAME")]
    pub track: String,
    /// Branch of the track on the remote: stop check reads it, a published file lands on it.
    #[arg(long = TRACK_BRANCH, value_name = "BRANCH")]
    pub track_branch: String,
    #[arg(long = ROW, value_name = "ID")]
    pub id: String,
    /// Track branch commit: the track file, and the tip a failure marks.
    #[arg(long = TRACK_TIP, value_name = "SHA")]
    pub track_tip: String,
    /// `to` tip: config, image, branch base.
    #[arg(long = BASE, value_name = "SHA")]
    pub base: String,
    /// Container variable and credential name each exec gets. The worker reads
    /// the values at row start.
    #[arg(long = ENV, value_name = "VAR=FILE", value_parser = binding)]
    pub env: Vec<Binding>,
    #[arg(long = MOUNT, value_name = "HOST:GUEST", value_parser = mount)]
    pub mounts: Vec<Mount>,
    /// Host device nodes, resolved.
    #[arg(long = DEVICE, value_name = "PATH")]
    pub devices: Vec<String>,
    #[arg(long = RUNNER, value_name = "RUNNER")]
    pub runner: Kind,
    /// First slot uid, runner `namespace` alone.
    #[arg(long = UID, value_name = "UID")]
    pub uid: Option<u32>,
    /// Project group, runner `namespace` alone.
    #[arg(long = GID, value_name = "GID")]
    pub gid: Option<u32>,
}

/// Row lock from stdin, slot lock once its last holder ends. `Done`: row pushed.
/// `Spent`: fail refs written. `Stopped`: on `scswarm stop`, no mark. `Err`:
/// machine fault, no mark.
pub fn work(repo: &Git, job: &Job) -> Result<Outcome<()>> {
    let address = job.address();
    let _row = lock::adopt(io::stdin().as_fd(), &paths::transcripts(&address)?)?;
    let dir = paths::slot(job.slot)?;
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
    stop::settle(repo, &address.key, &job.track_branch)?;
    let watch = Watch::new(repo, &address.key, &job.track_branch);
    Attempt {
        slot: &Slot::open(job.slot, &job.runner()?)?,
        stop: &|| watch.stopped(),
        watch: &watch,
        repo,
        track: &track,
        config: &config,
        row,
        job,
        address: &address,
        against: job.base.clone(),
    }
    .run()
}

impl Job {
    pub fn address(&self) -> Address {
        Address {
            project: self.project.clone(),
            key: Key::new(&self.track, &self.id),
        }
    }

    pub fn argv(&self) -> Vec<String> {
        let mut argv = vec![COMMAND.to_string(), self.project.to_string()];
        let mut flag = |name: &str, value: String| argv.extend([format!("--{name}"), value]);
        flag(SLOT, self.slot.to_string());
        flag(TRACK, self.track.clone());
        flag(TRACK_BRANCH, self.track_branch.clone());
        flag(ROW, self.id.clone());
        flag(TRACK_TIP, self.track_tip.clone());
        flag(BASE, self.base.clone());
        self.env
            .iter()
            .for_each(|binding| flag(ENV, binding.clone().into()));
        self.mounts
            .iter()
            .for_each(|bind| flag(MOUNT, bind.clone().into()));
        self.devices
            .iter()
            .for_each(|node| flag(DEVICE, node.clone()));
        if let Some(kind) = self.runner.to_possible_value() {
            flag(RUNNER, kind.get_name().to_string());
        }
        self.uid.iter().for_each(|uid| flag(UID, uid.to_string()));
        self.gid.iter().for_each(|gid| flag(GID, gid.to_string()));
        argv
    }

    /// Runner of this row: `serve` checked the host file it came from.
    pub fn runner(&self) -> Result<Runner> {
        Runner::new(self.runner, self.uid)
            .map(|runner| runner.serving(self.gid))
            .map_err(|why| Error::Failed(format!("arguments `--{RUNNER}`, `--{UID}`: {why}")))
    }
}

/// Everything one attempt reads, plus the `to` tip it last ran against.
struct Attempt<'a> {
    slot: &'a Slot,
    stop: &'a Stop<'a>,
    watch: &'a Watch<'a>,
    repo: &'a Git,
    track: &'a Track,
    config: &'a Config,
    row: &'a Row,
    job: &'a Job,
    address: &'a Address,
    against: String,
}

impl Attempt<'_> {
    /// Release before the outcome: no fault leaves the branch held. A fault of the
    /// attempt outranks one of the release. Then the fail refs of a spent attempt.
    fn run(mut self) -> Result<Outcome<()>> {
        let outcome = self.attempt();
        let released = self.slot.release();
        let outcome =
            outcome.map_err(|why| Error::Failed(format!("row {}: {why}", self.address.key)))?;
        released?;
        match outcome {
            Outcome::Done(()) => Ok(Outcome::Done(())),
            Outcome::Spent(why) => {
                self.fail(&why)?;
                Ok(Outcome::Spent(why))
            }
            Outcome::Stopped => {
                event::stop(self.address);
                Ok(Outcome::Stopped)
            }
        }
    }

    /// Credentials, slot, the row session unless a land ref stands, then the merge
    /// into `to`.
    fn attempt(&mut self) -> Result<Outcome<()>> {
        let (slot, job, address) = (self.slot, self.job, self.address);
        let key = &address.key;
        let credentials = credential::load(&job.env)?;
        let request = Request {
            repo: self.repo,
            base: &job.base,
            project: &address.project,
            image: self.config.image.as_deref(),
            mounts: &job.mounts,
            devices: &job.devices,
        };
        match slot.prepare(&request, self.stop)? {
            Outcome::Done(()) => {}
            Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
            Outcome::Stopped => return Ok(Outcome::Stopped),
        }
        let mut session = Session::open(
            slot,
            &credentials,
            self.config,
            self.track,
            self.row,
            address,
            self.stop,
        )?;
        let land = match self.repo.commit_of(&key.land_ref())? {
            Some(land) => land,
            None => {
                slot.checkout(&key.branch(), &job.base)?;
                match session.play(&self.transcript(paths::RUN_LOG)?)? {
                    Outcome::Done(played) => self.land(played)?,
                    Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
                    Outcome::Stopped => return Ok(Outcome::Stopped),
                }
            }
        };
        match self.publish()? {
            Outcome::Done(()) => {}
            Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
            Outcome::Stopped => return Ok(Outcome::Stopped),
        }
        self.deliver(&mut session, &land)
    }

    /// Land ref and any track commit in one transaction: a kill leaves both or neither,
    /// so the track moves for a landed row alone, and a landed row never replays.
    fn land(&self, played: Played) -> Result<String> {
        let (repo, address) = (self.repo, self.address);
        let key = &address.key;
        let mut refs = vec![(key.land_ref(), played.commit.clone())];
        if let Some(next) = played.next {
            let parent = &self.job.track_tip;
            let commit = repo.commit_file(parent, board::FILE, next.text(), &self.message())?;
            refs.push((key.publish_ref(), commit));
        }
        repo.refs_set(&refs)?;
        event::land(address);
        Ok(played.commit)
    }

    /// Push the track commit of the row, if one waits, over the tip it grew on. A
    /// track moved since then keeps its content, and the row still lands. Stop check
    /// first: this push moves the tip a stop ref holds.
    fn publish(&self) -> Result<Outcome<()>> {
        let (repo, address, branch) = (self.repo, self.address, &self.job.track_branch);
        let name = address.key.publish_ref();
        let Some(commit) = repo.commit_of(&name)? else {
            return Ok(Outcome::Done(()));
        };
        if self.watch.read()? {
            return Ok(Outcome::Stopped);
        }
        let parent = repo.tip(&format!("{commit}^"))?;
        let mut entry = Entry::new(self.row.steps.len());
        let spent = match repo.push_if(&commit, branch, &parent)? {
            Push::Done => {
                event::publish(address, &commit);
                entry.commit = Some(commit);
                entry.message = Some(self.message());
                None
            }
            Push::Behind => {
                event::publish_dropped(address, &commit);
                entry.fault = Some(format!(
                    "track branch `{branch}` moved during the row: its content stays, this file drops"
                ));
                None
            }
            Push::Declined(why) => {
                let why = format!("push to `{branch}` must land: {why}");
                entry.fault = Some(why.clone());
                Some(why)
            }
        };
        self.transcript(paths::RUN_LOG)?.append(&entry)?;
        if let Some(why) = spent {
            return Ok(Outcome::Spent(why));
        }
        repo.ref_delete(&name)?;
        Ok(Outcome::Done(()))
    }

    /// Message of the track commit.
    fn message(&self) -> String {
        format!("publish {}", self.address)
    }

    /// Merge the row into the `to` tip, push to `to` with no force, again onto the
    /// new tip after a fetch when behind. Stop check before each push. Landed:
    /// fail and stop refs go, `serve` mirrors after.
    fn deliver(&mut self, session: &mut Session, land: &str) -> Result<Outcome<()>> {
        let (slot, repo, address, to) = (self.slot, self.repo, self.address, &self.track.to);
        let key = &address.key;
        let tracking = git::remote_branch(to);
        let branch = key.branch();
        let transcript = self.transcript(paths::MERGE_LOG)?;
        let mut resolve = || session.resolve(to, &transcript);
        let head = loop {
            self.against = repo.tip(&tracking)?;
            match slot.merge_onto(&self.against, land, &branch, &mut resolve)? {
                Outcome::Done(()) => {}
                Outcome::Spent(why) => return Ok(Outcome::Spent(why)),
                Outcome::Stopped => return Ok(Outcome::Stopped),
            }
            if self.watch.read()? {
                return Ok(Outcome::Stopped);
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
        let [on_track, on_to] = key.fail_refs();
        for name in [on_track, on_to, key.stop_ref()] {
            repo.ref_delete(&name)?;
        }
        event::push(address, to, &head);
        Ok(Outcome::Done(()))
    }

    /// Track tip and `to` tip of this attempt.
    fn fail(&self, why: &str) -> Result<()> {
        event::fail(self.address, why);
        let [on_track, on_to] = self.address.key.fail_refs();
        self.repo.ref_set(&on_track, &self.job.track_tip)?;
        self.repo.ref_set(&on_to, &self.against)
    }

    fn transcript(&self, name: &str) -> Result<Transcript> {
        Ok(Transcript::at(paths::transcript(self.address, name)?))
    }
}

fn mount(text: &str) -> std::result::Result<Mount, String> {
    Mount::try_from(text.to_string())
}

fn binding(text: &str) -> std::result::Result<Binding, String> {
    Binding::try_from(text.to_string())
}
