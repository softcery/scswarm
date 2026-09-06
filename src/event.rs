use crate::error::Error;

/// Sole writer of `serve` loop lines. Progress on stdout, faults on stderr.
/// journald stamps each line, so none carries a time field.
pub fn fetch(sha: &str) {
    println!("fetch main={}", short(sha));
}

/// Task and stage a line names.
#[derive(Clone, Copy)]
pub struct At<'a> {
    pub id: &'a str,
    pub stage: usize,
    pub total: usize,
}

pub fn assign(at: &At, slot: usize) {
    println!(
        "assign task={} slot={slot} stage={}/{}",
        at.id, at.stage, at.total
    );
}

pub fn turn(at: &At, number: usize) {
    println!(
        "turn task={} stage={}/{} turn={number}",
        at.id, at.stage, at.total
    );
}

pub fn retry(at: &At, number: usize, attempt: usize, exit: Option<i32>) {
    println!(
        "retry task={} stage={}/{} turn={number} attempt={attempt} exit={}",
        at.id,
        at.stage,
        at.total,
        exit.map_or_else(|| "none".to_string(), |code| code.to_string())
    );
}

pub fn commit(at: &At, sha: &str) {
    println!(
        "commit task={} stage={}/{} sha={}",
        at.id,
        at.stage,
        at.total,
        short(sha)
    );
}

pub fn land(id: &str) {
    println!("land task={id}");
}

pub fn fail(at: &At, why: &Error) {
    eprintln!(
        "fail task={} stage={}/{} cause={why}",
        at.id, at.stage, at.total
    );
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}
