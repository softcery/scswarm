use std::collections::BTreeSet;
use std::fmt::Display;

use crate::error::Error;

/// Sole writer of `serve` loop lines. Progress on stdout, faults on stderr.
/// journald stamps each line, so none carries a time field.
pub fn fetch(track: &str, tip: &str, to: &str) {
    println!("fetch track={track} tip={} to={}", short(tip), short(to));
}

pub fn open(track: &str, to: &str, sha: &str) {
    println!("open track={track} to={to} sha={}", short(sha));
}

pub fn mirror(refs: usize) {
    println!("mirror refs={refs}");
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

pub fn land(key: impl Display) {
    println!("land row={key}");
}

pub fn push(key: impl Display, to: &str, sha: &str) {
    println!("push row={key} to={to} sha={}", short(sha));
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
    pub fn report(&mut self, faults: &[Error], emit: fn(&Error)) {
        let causes: BTreeSet<String> = faults.iter().map(Error::to_string).collect();
        for why in faults {
            if !self.0.contains(&why.to_string()) {
                emit(why);
            }
        }
        self.0 = causes;
    }
}

/// Cycle or track passed over.
pub fn skip(why: &Error) {
    eprintln!("skip cause={why}");
}

/// Host file unreadable: loop runs on last good copy.
pub fn stale(why: &Error) {
    eprintln!("stale cause={why}");
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}
