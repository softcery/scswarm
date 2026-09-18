use std::fs;
use std::io;
use std::path::Path;

use crate::error::{Error, Result};
use crate::paths::{self, Project, MERGE_LOG, RUN_LOG};
use crate::turn::Entry;

const PROMPT: &str = ">";
const REPLY: &str = "<";
const FAULT: &str = "!";

/// Print the transcripts of one task: the merge session first, then the row.
pub fn log(project: &Project, id: &str) -> Result<()> {
    let mut printed = false;
    for name in [MERGE_LOG, RUN_LOG] {
        let Some(transcript) = read(&paths::transcript(project, id, name)?)? else {
            continue;
        };
        printed = true;
        print!("{}", render(name, &transcript));
    }
    if !printed {
        let dir = paths::transcripts(project, id)?;
        return Err(Error::Invalid(format!(
            "id `{id}` must name a task that ran a session: {} holds no transcript",
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
    transcript.lines().map(|line| entry(name, line)).collect()
}

/// Header, prompt, then reply or failure. A line that parses as no entry stays raw.
fn entry(name: &str, line: &str) -> String {
    let Ok(turn) = serde_json::from_str::<Entry>(line) else {
        return format!("{line}\n");
    };
    let head = match turn.exit {
        Some(code) => format!("{name} turn {} exit {code}", turn.turn),
        None => format!("{name} turn {}", turn.turn),
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
