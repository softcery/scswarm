use std::collections::BTreeMap;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};

pub const FILE: &str = "SWARM.md";

const COLUMNS: [&str; 4] = ["id", "after", "agent", "task"];

/// Project task list. Read only: no write path here.
#[derive(Debug)]
pub struct Board {
    rows: Vec<Row>,
}

#[derive(Debug, Clone)]
pub struct Row {
    /// Key of the row: branch, fail mark, trailer, transcript dir.
    pub id: String,
    /// Row indices this row waits for, resolved at parse time.
    pub after: Vec<usize>,
    pub agent: String,
    /// Free text handed to the agent. Empty allowed.
    pub task: String,
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
                "{FILE}: must hold a table with header `{}`",
                header()
            ))
        })?;
        if head.cells != COLUMNS {
            return Err(bad_line(
                head.line,
                &format!("header must read `{}`", header()),
            ));
        }

        let entries: Vec<Entry> = table.collect();
        let drafts = read_drafts(&entries, config)?;
        let index = index_ids(&drafts)?;
        let rows = link_rows(&drafts, &index)?;
        check_loops(&rows)?;
        Ok(Self { rows })
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
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

fn header() -> String {
    format!("| {} |", COLUMNS.join(" | "))
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

/// Cells of one row, named by `COLUMNS`, agent resolved.
struct Draft<'a> {
    line: usize,
    id: &'a str,
    after: &'a str,
    agent: &'a str,
    task: &'a str,
}

impl<'a> Draft<'a> {
    fn read(entry: &'a Entry, config: &'a Config) -> Result<Self> {
        let [id, after, agent, task] = <&[String; COLUMNS.len()]>::try_from(entry.cells.as_slice())
            .map_err(|_| {
                bad_line(
                    entry.line,
                    &format!(
                        "row must hold {} cells: {}",
                        COLUMNS.len(),
                        COLUMNS.join(", ")
                    ),
                )
            })?;
        Ok(Self {
            line: entry.line,
            id,
            after,
            agent: match agent.is_empty() {
                true => &config.default_agent,
                false => agent,
            },
            task,
        })
    }

    fn check(&self, config: &Config) -> Result<()> {
        if self.id.is_empty() {
            return Err(self.bad("field `id` must not be empty"));
        }
        if !git::valid_ref(self.id) {
            return Err(self.bad(&format!(
                "field `id` must hold a name git takes as a ref: {}",
                git::REF_RULE
            )));
        }
        if !config.knows_agent(self.agent) {
            return Err(self.bad(&format!(
                "field `agent` must name a key of `agents` in {}: {}",
                crate::config::FILE,
                config.agent_keys()
            )));
        }
        Ok(())
    }

    /// Resolve `after` against the board, own every cell.
    fn row(&self, index: &BTreeMap<&str, usize>) -> Result<Row> {
        let after = self
            .after
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| {
                index.get(id).copied().ok_or_else(|| {
                    self.bad(&format!(
                        "field `after` names id `{id}`, absent from the board"
                    ))
                })
            })
            .collect::<Result<Vec<usize>>>()?;
        Ok(Row {
            id: self.id.to_string(),
            after,
            agent: self.agent.to_string(),
            task: self.task.to_string(),
        })
    }

    fn bad(&self, what: &str) -> Error {
        bad_line(self.line, what)
    }
}

/// Cells to drafts, every draft checked on its own.
fn read_drafts<'a>(entries: &'a [Entry], config: &'a Config) -> Result<Vec<Draft<'a>>> {
    let drafts = entries
        .iter()
        .map(|entry| Draft::read(entry, config))
        .collect::<Result<Vec<Draft>>>()?;
    for draft in &drafts {
        draft.check(config)?;
    }
    Ok(drafts)
}

fn index_ids<'a>(drafts: &[Draft<'a>]) -> Result<BTreeMap<&'a str, usize>> {
    let mut index = BTreeMap::new();
    for (position, draft) in drafts.iter().enumerate() {
        if let Some(first) = index.insert(draft.id, position) {
            return Err(draft.bad(&format!(
                "id `{}` already used on line {}",
                draft.id, drafts[first].line
            )));
        }
    }
    Ok(index)
}

fn link_rows(drafts: &[Draft<'_>], index: &BTreeMap<&str, usize>) -> Result<Vec<Row>> {
    drafts.iter().map(|draft| draft.row(index)).collect()
}

/// Row on the walk path twice = loop. Such a row never settles, so every row
/// behind it reads `blocked` for good.
fn check_loops(rows: &[Row]) -> Result<()> {
    let mut marks = vec![Mark::Fresh; rows.len()];
    let mut path = Vec::new();
    for start in 0..rows.len() {
        walk(rows, start, &mut marks, &mut path)?;
    }
    Ok(())
}

/// Depth-first walk of `after`.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Fresh,
    OnPath,
    Walked,
}

fn walk(rows: &[Row], at: usize, marks: &mut [Mark], path: &mut Vec<usize>) -> Result<()> {
    match marks[at] {
        Mark::Walked => return Ok(()),
        Mark::OnPath => return Err(loop_error(rows, path, at)),
        Mark::Fresh => {}
    }
    marks[at] = Mark::OnPath;
    path.push(at);
    for &parent in &rows[at].after {
        walk(rows, parent, marks, path)?;
    }
    path.pop();
    marks[at] = Mark::Walked;
    Ok(())
}

/// Ids from where the loop opens, closing id last.
fn loop_error(rows: &[Row], path: &[usize], closes: usize) -> Error {
    let open = path
        .iter()
        .position(|&at| at == closes)
        .expect("a row marked `OnPath` must sit on the path");
    let ids: Vec<&str> = path[open..]
        .iter()
        .chain([&closes])
        .map(|&at| rows[at].id.as_str())
        .collect();
    Error::Invalid(format!(
        "{FILE}: field `after` must name no loop: {}",
        ids.join(" -> ")
    ))
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
  stages: [(turns: ["/execute {id}", "{commit}"])],
)
"#;

    const EXAMPLE: &str = "\
| id | after | agent | task |
| --- | --- | --- | --- |
| parse-10-repair | | | Repair the parser. Spec in docs/spec/parser.md. |
| trace-06-decode | | | |
| parse-15-postures | parse-10-repair, trace-06-decode | codex | Postures. |
";

    fn config() -> Config {
        Config::parse(CONFIG).expect("test config must parse")
    }

    #[test]
    fn parses_contract_example() {
        let board = Board::parse(EXAMPLE, &config()).expect("example must parse");
        let rows = board.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].task, "Repair the parser. Spec in docs/spec/parser.md.");
        assert_eq!(rows[1].id, "trace-06-decode");
        assert_eq!(rows[1].task, "");
        assert_eq!(rows[1].agent, "claude");
        assert_eq!(rows[2].after, [0, 1]);
        assert_eq!(rows[2].agent, "codex");
    }

    #[test]
    fn names_a_row_of_wrong_width() {
        let text = "| id | after | agent | task |\n|a-one|||codex||\n";
        let why = Board::parse(text, &config())
            .expect_err("five cells must fail")
            .to_string();
        assert!(why.contains("4 cells"), "{why}");
    }

    #[test]
    fn reads_tight_cells() {
        let text = "| id | after | agent | task |\n|a-one||codex||\n";
        let board = Board::parse(text, &config()).expect("tight cells must parse");
        assert_eq!(board.rows()[0].agent, "codex");
    }

    #[test]
    fn names_a_row_whose_id_is_no_ref() {
        let text = EXAMPLE.replace("| parse-10-repair |", "| parse 10 repair |");
        let why = Board::parse(&text, &config())
            .expect_err("an id with a space must fail")
            .to_string();
        assert!(why.contains("line 3"), "{why}");
        assert!(why.contains("takes as a ref"), "{why}");
    }

    #[test]
    fn names_a_row_with_no_id() {
        let text = EXAMPLE.replace("| trace-06-decode | |", "|  | |");
        let why = Board::parse(&text, &config())
            .expect_err("an empty id must fail")
            .to_string();
        assert!(why.contains("line 4"), "{why}");
        assert!(why.contains("`id`"), "{why}");
    }

    #[test]
    fn names_the_ids_of_a_loop() {
        let text = EXAMPLE.replace(
            "| parse-10-repair | |",
            "| parse-10-repair | parse-15-postures |",
        );
        let why = Board::parse(&text, &config())
            .expect_err("a loop in `after` must fail")
            .to_string();
        assert!(
            why.contains("parse-10-repair -> parse-15-postures -> parse-10-repair"),
            "{why}"
        );
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
