use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths::{self, Project};
use crate::turn::Entry;

const PROMPT: &str = ">";
const REPLY: &str = "<";
const FAULT: &str = "!";

/// Print the transcript of one task: the named stage, or every stage in order.
pub fn log(project: &Project, id: &str, stage: Option<usize>) -> Result<()> {
    let files = match stage {
        Some(number) => vec![(number, paths::transcript(project, id, number)?)],
        None => stages(&paths::transcripts(project, id)?)?,
    };
    for (number, path) in files {
        print!("{}", render(number, &read(&path)?));
    }
    Ok(())
}

/// Transcript files of one task, by stage number.
fn stages(dir: &Path) -> Result<Vec<(usize, PathBuf)>> {
    let listing = fs::read_dir(dir).map_err(|why| unreadable(dir, &why))?;
    let mut found: Vec<(usize, PathBuf)> = listing
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            Some((paths::transcript_stage(&path)?, path))
        })
        .collect();
    found.sort_by_key(|(number, _)| *number);
    Ok(found)
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|why| unreadable(path, &why))
}

fn render(stage: usize, transcript: &str) -> String {
    transcript.lines().map(|line| entry(stage, line)).collect()
}

/// Header, prompt, then reply or failure. A line that parses as no entry stays raw.
fn entry(stage: usize, line: &str) -> String {
    let Ok(turn) = serde_json::from_str::<Entry>(line) else {
        return format!("{line}\n");
    };
    let head = match turn.exit {
        Some(code) => format!("stage {stage} turn {} exit {code}", turn.turn),
        None => format!("stage {stage} turn {}", turn.turn),
    };
    let mut lines = vec![head, format!("{PROMPT} {}", turn.prompt.trim())];
    let marked = [
        (REPLY, &turn.result),
        (REPLY, &turn.stdout),
        (FAULT, &turn.error),
    ];
    for (mark, field) in marked {
        if let Some(text) = field.as_deref().map(str::trim).filter(|it| !it.is_empty()) {
            lines.push(format!("{mark} {text}"));
        }
    }
    lines.join("\n") + "\n"
}

/// Absent transcript names a task or stage that never ran: bad input. Any other
/// fault is the filesystem refusing.
fn unreadable(path: &Path, why: &io::Error) -> Error {
    let what = format!("transcript {}: {why}", path.display());
    match why.kind() {
        io::ErrorKind::NotFound => Error::Invalid(what),
        _ => Error::Failed(what),
    }
}
