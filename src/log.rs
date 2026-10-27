use crate::error::{Error, Result};
use crate::paths::{self, MERGE_LOG, RUN_LOG};
use crate::state::Address;
use crate::transcript::{Entry, Kind, Sent, Transcript, Try};

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
        let Some(transcript) = Transcript::at(paths::transcript(row, name)?).read()? else {
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

fn render(name: &str, transcript: &str) -> String {
    transcript.lines().map(|line| block(name, line)).collect()
}

/// Header, then one marked line per text the entry holds. A line that parses as
/// no entry stays raw.
fn block(name: &str, line: &str) -> String {
    let Ok(entry) = serde_json::from_str::<Entry>(line) else {
        return format!("{line}\n");
    };
    match entry.kind {
        Kind::Start { base } => format!("{name} start base {base}\n"),
        Kind::Commit {
            step,
            commit,
            message,
        } => marked(
            format!("{name} step {step} commit {commit}"),
            [(COMMIT, Some(message))],
        ),
        Kind::Try(record) => attempt(name, record),
    }
}

/// Step, exit and context in the header; sent text, reply, output, fault below.
fn attempt(name: &str, record: Try) -> String {
    let mut head = format!("{name} step {}", record.step);
    if let Some(code) = record.exit {
        head.push_str(&format!(" exit {code}"));
    }
    if let Some(context) = record.context {
        head.push_str(&format!(" context {}/{}", context.used, context.window));
    }
    let sent = match record.sent {
        Sent::Prompt(text) => (PROMPT, Some(text)),
        Sent::Run(text) => (RUN, Some(text)),
    };
    marked(
        head,
        [
            sent,
            (REPLY, record.result),
            (REPLY, record.stdout),
            (STDERR, record.stderr),
            (FAULT, record.fault),
        ],
    )
}

fn marked<const N: usize>(head: String, texts: [(&str, Option<String>); N]) -> String {
    let mut lines = vec![head];
    for (mark, text) in texts {
        if let Some(text) = text.as_deref().map(str::trim).filter(|it| !it.is_empty()) {
            lines.push(format!("{mark} {text}"));
        }
    }
    lines.join("\n") + "\n"
}
