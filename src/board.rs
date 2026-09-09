use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};

pub const FILE: &str = "SWARM.md";

const COLUMNS: [&str; 3] = ["task", "after", "agent"];

/// Project task list. Read only: no write path here.
#[derive(Debug)]
pub struct Board {
    rows: Vec<Row>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub task: String,
    /// Row indices this row waits for, resolved at parse time.
    pub after: Vec<usize>,
    pub agent: String,
}

impl Board {
    pub fn from_trunk(git: &Git, config: &Config) -> Result<Self> {
        let text = git
            .show(git::TRUNK, FILE)?
            .ok_or_else(|| Error::Invalid(format!("{FILE}: absent from the trunk branch")))?;
        Self::parse(&text, config)
    }

    pub fn parse(text: &str, config: &Config) -> Result<Self> {
        let mut table = table_lines(text);
        let head = table.next().ok_or_else(|| {
            Error::Invalid(format!(
                "{FILE}: must hold a table with header `| task | after | agent |`"
            ))
        })?;
        if head.cells != COLUMNS {
            return Err(bad_line(
                head.line,
                "header must read `| task | after | agent |`",
            ));
        }

        let entries: Vec<Entry> = table.collect();
        let rows = read_rows(&entries, config)?;
        check_rows(&entries, &rows, config)?;
        let index = index_ids(&entries, &rows)?;
        Ok(Self {
            rows: link_rows(&entries, rows, &index)?,
        })
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

impl Row {
    /// Id = file stem when task text is path, else text.
    pub fn id(&self) -> &str {
        let task = self.task.as_str();
        let path_like =
            !task.contains(char::is_whitespace) && (task.contains('/') || task.contains('.'));
        if !path_like {
            return task;
        }
        Path::new(task)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or(task)
    }
}

/// One table line: 1-based number, trimmed cells.
struct Entry {
    line: usize,
    cells: Vec<String>,
}

/// Table lines, header first, separators dropped.
fn table_lines(text: &str) -> impl Iterator<Item = Entry> + '_ {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with('|'))
        .map(|(index, line)| Entry {
            line: index + 1,
            cells: cells(line),
        })
        .filter(|entry| !is_separator(&entry.cells))
}

fn cells(line: &str) -> Vec<String> {
    let row = line.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    row.split('|').map(|cell| cell.trim().to_string()).collect()
}

fn is_separator(cells: &[String]) -> bool {
    cells
        .iter()
        .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':'))
}

fn read_rows(entries: &[Entry], config: &Config) -> Result<Vec<Row>> {
    entries
        .iter()
        .map(|entry| {
            let [task, _, agent] = <&[String; 3]>::try_from(entry.cells.as_slice())
                .map_err(|_| bad_line(entry.line, "row must hold 3 cells: task, after, agent"))?;
            let agent = if agent.is_empty() {
                &config.default_agent
            } else {
                agent
            };
            Ok(Row {
                task: task.clone(),
                after: Vec::new(),
                agent: agent.clone(),
            })
        })
        .collect()
}

fn check_rows(entries: &[Entry], rows: &[Row], config: &Config) -> Result<()> {
    for (row, entry) in rows.iter().zip(entries) {
        if row.task.is_empty() {
            return Err(bad_line(entry.line, "field `task` must not be empty"));
        }
        if !config.knows_agent(&row.agent) {
            return Err(bad_line(
                entry.line,
                &format!(
                    "field `agent` must name a key of `agents` in {}: {}",
                    crate::config::FILE,
                    config.agent_keys()
                ),
            ));
        }
    }
    Ok(())
}

fn index_ids(entries: &[Entry], rows: &[Row]) -> Result<BTreeMap<String, usize>> {
    let mut index = BTreeMap::new();
    for (position, row) in rows.iter().enumerate() {
        if let Some(first) = index.insert(row.id().to_string(), position) {
            let first_line = entries[first].line;
            return Err(bad_line(
                entries[position].line,
                &format!("id `{}` already used on line {first_line}", row.id()),
            ));
        }
    }
    Ok(index)
}

fn link_rows(
    entries: &[Entry],
    rows: Vec<Row>,
    index: &BTreeMap<String, usize>,
) -> Result<Vec<Row>> {
    rows.into_iter()
        .zip(entries)
        .map(|(row, entry)| {
            let after = entry.cells[1]
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(|id| {
                    index.get(id).copied().ok_or_else(|| {
                        bad_line(
                            entry.line,
                            &format!("field `after` names id `{id}`, absent from the board"),
                        )
                    })
                })
                .collect::<Result<Vec<usize>>>()?;
            Ok(Row { after, ..row })
        })
        .collect()
}

fn bad_line(line: usize, what: &str) -> Error {
    Error::Invalid(format!("{FILE} line {line}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
(
  image: "Containerfile.swarm",
  default_agent: "claude",
  agents: {
    "claude": (exec: "claude -p"),
    "codex": (exec: "codex exec"),
  },
  stages: [(turns: ["/execute {task}", "{commit}"])],
)
"#;

    const EXAMPLE: &str = "\
| task | after | agent |
| --- | --- | --- |
| docs/tasks/next/parse-10-repair.md | | |
| docs/tasks/next/trace-06-decode.md | | |
| docs/tasks/next/parse-15-postures.md | parse-10-repair, trace-06-decode | codex |
";

    fn config() -> Config {
        Config::parse(CONFIG).expect("test config must parse")
    }

    #[test]
    fn parses_contract_example() {
        let board = Board::parse(EXAMPLE, &config()).expect("example must parse");
        let rows = board.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].id(), "trace-06-decode");
        assert_eq!(rows[1].task, "docs/tasks/next/trace-06-decode.md");
        assert_eq!(rows[1].agent, "claude");
        assert_eq!(rows[2].after, [0, 1]);
        assert_eq!(rows[2].agent, "codex");
    }

    #[test]
    fn keeps_empty_cells() {
        let text = "| task | after | agent |\n|docs/tasks/next/a-one.md||codex|\n";
        let board = Board::parse(text, &config()).expect("tight cells must parse");
        assert_eq!(board.rows()[0].agent, "codex");
    }

    #[test]
    fn names_the_bad_row() {
        let text = EXAMPLE.replace("parse-10-repair,", "parse-99-missing,");
        let why = Board::parse(&text, &config())
            .expect_err("unknown id must fail")
            .to_string();
        assert!(why.contains("line 5"), "{why}");
        assert!(why.contains("`after`"), "{why}");
    }
}
