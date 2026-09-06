use crate::error::Error;

/// Sole writer of `serve` loop operator lines. One function per event, fixed field order.
/// journald stamps every line, so none carries a timestamp field.
pub fn fetch(sha: &str) {
    println!("fetch main={}", short(sha));
}

pub fn assign(id: &str, slot: usize, stage: usize, total: usize) {
    println!("assign task={id} slot={slot} stage={stage}/{total}");
}

pub fn turn(id: &str, stage: usize, total: usize, number: usize) {
    println!("turn task={id} stage={stage}/{total} turn={number}");
}

pub fn retry(
    id: &str,
    stage: usize,
    total: usize,
    number: usize,
    attempt: usize,
    exit: Option<i32>,
) {
    println!(
        "retry task={id} stage={stage}/{total} turn={number} attempt={attempt} exit={}",
        exit.map_or_else(|| "none".to_string(), |code| code.to_string())
    );
}

pub fn commit(id: &str, stage: usize, total: usize, sha: &str) {
    println!("commit task={id} stage={stage}/{total} sha={}", short(sha));
}

pub fn land(id: &str) {
    println!("land task={id}");
}

pub fn fail(id: &str, stage: usize, total: usize, why: &Error) {
    println!("fail task={id} stage={stage}/{total} cause={why}");
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}
