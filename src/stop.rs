use std::cell::Cell;
use std::time::Instant;

use crate::board;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::host;
use crate::state::{self, Key};

/// Hold `key` at its track tip: here, then on the remote by mirror. A worker
/// running it ends at its next check.
pub fn stop(repo: &Git, tracks: &str, key: &Key) -> Result<()> {
    let track = board::track(repo, tracks, &key.track)?.ok_or_else(|| {
        Error::Invalid(format!(
            "row `{key}` must name a track branch: no branch `{}`",
            board::branch(tracks, &key.track)
        ))
    })?;
    if !track.has_row(&key.id) {
        return Err(Error::Invalid(format!(
            "row `{key}` must name a row of its track: {} holds no id `{}`",
            board::FILE,
            key.id
        )));
    }
    if let Some(state) = state::landed(repo, &track, key)? {
        return Err(Error::Invalid(format!(
            "row `{key}` must not be pushed or merged to stop: it reads {state}; \
             `scswarm reset` it, then `git revert -m 1 <merge commit>` on `{}`",
            track.to
        )));
    }
    repo.ref_set(&key.stop_ref(), &track.tip)?;
    println!("stopped {key} at {}", track.tip);
    repo.mirror_kept()
}

/// Worker start, under the row lock. A stop ref at an older track tip: the row
/// stopped, then its track moved. Its old work must not merge or publish: drop the
/// land and publish refs, then the stop ref, unless a new `stop` moved it meanwhile.
pub fn settle(repo: &Git, key: &Key, branch: &str) -> Result<()> {
    let stop_ref = key.stop_ref();
    let Some(stop) = repo.commit_of(&stop_ref)? else {
        return Ok(());
    };
    let tip = repo.commit_of(&git::remote_branch(branch))?;
    if state::stops(Some(&stop), tip.as_deref()) {
        return Ok(());
    }
    repo.ref_delete(&key.land_ref())?;
    repo.ref_delete(&key.publish_ref())?;
    repo.ref_delete_at(&stop_ref, &stop)
}

/// Stop check of one running row: `state::stops` on its stop ref and the current
/// track tip, read once per `host::STOP_POLL`. Once true, stays true.
pub struct Watch<'a> {
    repo: &'a Git,
    stop_ref: String,
    /// Remote-tracking ref of the track branch.
    track: String,
    next: Cell<Instant>,
    seen: Cell<bool>,
}

impl<'a> Watch<'a> {
    pub fn new(repo: &'a Git, key: &Key, branch: &str) -> Self {
        Self {
            repo,
            stop_ref: key.stop_ref(),
            track: git::remote_branch(branch),
            next: Cell::new(Instant::now()),
            seen: Cell::new(false),
        }
    }

    pub fn stopped(&self) -> Result<bool> {
        let now = Instant::now();
        if self.seen.get() || now < self.next.get() {
            return Ok(self.seen.get());
        }
        self.next.set(now + host::STOP_POLL);
        let stop = self.repo.commit_of(&self.stop_ref)?;
        let tip = self.repo.commit_of(&self.track)?;
        self.seen.set(state::stops(stop.as_deref(), tip.as_deref()));
        Ok(self.seen.get())
    }
}
