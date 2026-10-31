use std::collections::BTreeSet;
use std::fmt::Display;

use crate::error::Error;
use crate::paths::Project;

/// Sole writer of `serve` loop lines. Progress on stdout, faults on stderr.
/// journald stamps each line, so none carries a time field.
pub fn tip(project: &Project, track: &str, tip: &str, to: &str) {
    println!(
        "tip project={project} track={track} tip={} to={}",
        short(tip),
        short(to)
    );
}

pub fn open(project: &Project, track: &str, to: &str, sha: &str) {
    println!(
        "open project={project} track={track} to={to} sha={}",
        short(sha)
    );
}

pub fn mirror(project: &Project, refs: usize) {
    println!("mirror project={project} refs={refs}");
}

pub fn assign(key: impl Display, slot: usize) {
    println!("assign row={key} slot={slot}");
}

pub fn merge(key: impl Display, parent: &str) {
    println!("merge row={key} parent={parent}");
}

pub fn step(key: impl Display, number: usize) {
    println!("step row={key} step={number}");
}

pub fn attempt(key: impl Display, number: usize, attempt: usize, fault: &str) {
    println!("attempt row={key} step={number} try={attempt} cause={fault}");
}

pub fn commit(key: impl Display, sha: &str) {
    println!("commit row={key} sha={}", short(sha));
}

pub fn publish(key: impl Display, sha: &str) {
    println!("publish row={key} sha={}", short(sha));
}

pub fn publish_dropped(key: impl Display, sha: &str) {
    println!("publish-dropped row={key} sha={}", short(sha));
}

pub fn land(key: impl Display) {
    println!("land row={key}");
}

pub fn push(key: impl Display, to: &str, sha: &str) {
    println!("push row={key} to={to} sha={}", short(sha));
}

pub fn stop(key: impl Display) {
    println!("stop row={key}");
}

pub fn fail(key: impl Display, why: &str) {
    eprintln!("fail row={key} cause={why}");
}

pub fn attach(slot: usize) {
    println!("attach slot={slot}");
}

pub fn create(slot: usize) {
    println!("create slot={slot}");
}

pub fn remove(slot: usize) {
    println!("remove slot={slot}");
}

/// Causes of one fault kind last reported. A repeat stays quiet.
#[derive(Default)]
pub struct Latch(BTreeSet<String>);

impl Latch {
    pub fn report(&mut self, faults: &[Error], emit: impl Fn(&Error)) {
        let causes: BTreeSet<String> = faults.iter().map(Error::to_string).collect();
        for why in faults {
            if !self.0.contains(&why.to_string()) {
                emit(why);
            }
        }
        self.0 = causes;
    }
}

/// Causes reported since the last `clear`, however many cycles ago. A repeat stays quiet.
#[derive(Default)]
pub struct Seen(BTreeSet<String>);

impl Seen {
    pub fn once(&mut self, why: &Error, emit: impl Fn(&Error)) {
        if self.0.insert(why.to_string()) {
            emit(why);
        }
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Project or track passed over this cycle.
pub fn skip(project: &Project, why: &Error) {
    eprintln!("skip project={project} cause={why}");
}

/// Host file unreadable: loop runs on last good copy.
pub fn stale(why: &Error) {
    eprintln!("stale cause={why}");
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}
