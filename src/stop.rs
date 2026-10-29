use std::cell::Cell;
use std::time::Instant;

use crate::board;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::host;
use crate::lock;
use crate::paths;
use crate::state::{self, Address, Key, State};

/// Hold `address` at its track tip: here, then on the remote by mirror. A worker
/// running it ends at its next check; with none, the land ref goes here.
pub fn stop(repo: &Git, tracks: &str, address: &Address) -> Result<()> {
    let key = &address.key;
    let held = |key: &Key| {
        lock::held(&paths::transcripts(&Address {
            project: address.project.clone(),
            key: key.clone(),
        })?)
    };
    let track = board::scan(repo, tracks)?
        .tracks
        .into_iter()
        .find(|track| track.name == key.track)
        .ok_or_else(|| missing(key))?;
    let snapshot = state::snapshot(repo, track, &held)?;
    let index = snapshot
        .board
        .rows()
        .iter()
        .position(|row| row.id == key.id)
        .ok_or_else(|| missing(key))?;
    if let state @ (State::Pushed | State::Merged) = snapshot.states[index] {
        return Err(Error::Invalid(format!(
            "row `{key}` must not be pushed or merged to stop: it reads {state}; \
             `scswarm reset` it, then `git revert -m 1 <merge commit>` on `{}`",
            snapshot.track.to
        )));
    }
    repo.ref_set(&key.stop_ref(), &snapshot.track.tip)?;
    println!("stopped {key} at {}", snapshot.track.tip);
    if !held(key)? && drop_land(repo, key)? {
        println!("deleted {}", key.land_ref());
    }
    repo.mirror_kept()
}

/// Stop check of one running row: its stop ref against the current track tip,
/// read once per `host::STOP_POLL`. Once true, stays true.
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

/// Stopped row's undelivered work must not merge later. `true`: land ref stood.
pub fn drop_land(repo: &Git, key: &Key) -> Result<bool> {
    let land = key.land_ref();
    let stood = repo.commit_of(&land)?.is_some();
    if stood {
        repo.ref_delete(&land)?;
    }
    Ok(stood)
}

fn missing(key: &Key) -> Error {
    Error::Invalid(format!(
        "row `{key}` must name a row of a track branch: no track `{}` holds id `{}`",
        key.track, key.id
    ))
}
