use std::fs;
use std::io;
use std::path::Path;

use crate::error::{Error, Result};
use crate::paths::{self, MERGE_LOG, RUN_LOG};
use crate::state::Address;
use crate::transcript::Entry;

const PROMPT: &str = ">";
const RUN: &str = "$";
const COMMIT: &str = "@";
const REPLY: &str = "<";
const STDERR: &str = "2>";
const FAULT: &str = "!";

/// Print the transcripts of one row: the row session first, then the merge into `to`.
pub fn log(row: &Address) -> Result<()> {
    let mut printed = false;
    for name in [RUN_LOG, MERGE_LOG] {
        let Some(transcript) = read(&paths::transcript(row, name)?)? else {
            continue;
        };
        printed = true;
        print!("{}", render(name, &transcript));
    }
    if !printed {
        let dir = paths::transcripts(row)?;
        return Err(Error::Invalid(format!(
            "row `{}` must name a row that ran a session: {} holds no transcript",
            row.key,
            dir.display()
        )));
    }
    Ok(())
}

/// `None` when no such session ran. Any other fault is the filesystem refusing.
fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(why) if why.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(why) => Err(Error::Failed(format!(
            "transcript {}: {why}",
            path.display()
        ))),
    }
}

fn render(name: &str, transcript: &str) -> String {
    transcript.lines().map(|line| block(name, line)).collect()
}

/// Header, prompt, command or commit, then reply, output and faults. A line that
/// parses as no entry stays raw.
fn block(name: &str, line: &str) -> String {
    let Ok(entry) = serde_json::from_str::<Entry>(line) else {
        return format!("{line}\n");
    };
    let mut head = format!("{name} step {}", entry.step);
    if let Some(code) = entry.exit {
        head.push_str(&format!(" exit {code}"));
    }
    if let Some(commit) = &entry.commit {
        head.push_str(&format!(" commit {commit}"));
    }
    let mut lines = vec![head];
    let marked = [
        (PROMPT, &entry.prompt),
        (RUN, &entry.run),
        (COMMIT, &entry.message),
        (REPLY, &entry.result),
        (REPLY, &entry.stdout),
        (STDERR, &entry.stderr),
        (FAULT, &entry.fault),
    ];
    for (mark, field) in marked {
        if let Some(text) = field.as_deref().map(str::trim).filter(|it| !it.is_empty()) {
            lines.push(format!("{mark} {text}"));
        }
    }
    lines.join("\n") + "\n"
}
