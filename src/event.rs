use crate::error::Error;

/// Sole writer of `serve` loop lines. Progress on stdout, faults on stderr.
/// journald stamps each line, so none carries a time field.
pub fn fetch(sha: &str) {
    println!("fetch trunk={}", short(sha));
}

pub fn assign(id: &str, slot: usize) {
    println!("assign task={id} slot={slot}");
}

pub fn merge(id: &str, parent: &str) {
    println!("merge task={id} parent={parent}");
}

pub fn turn(id: &str, number: usize) {
    println!("turn task={id} turn={number}");
}

pub fn retry(id: &str, number: usize, attempt: usize, exit: Option<i32>) {
    println!(
        "retry task={id} turn={number} attempt={attempt} exit={}",
        exit.map_or_else(|| "none".to_string(), |code| code.to_string())
    );
}

pub fn commit(id: &str, sha: &str) {
    println!("commit task={id} sha={}", short(sha));
}

/// Cycle passed over: git refused the trunk.
pub fn skip(why: &Error) {
    eprintln!("skip cause={why}");
}

/// Host file unreadable. The loop runs on the last good copy.
pub fn stale(why: &Error) {
    eprintln!("stale cause={why}");
}

pub fn land(id: &str) {
    println!("land task={id}");
}

pub fn fail(id: &str, why: &Error) {
    eprintln!("fail task={id} cause={why}");
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}
