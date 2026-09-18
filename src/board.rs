use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::yaml;

pub const FILE: &str = "SWARM.yaml";

/// Project task list. Read only: no write path here.
#[derive(Debug)]
pub struct Board {
    rows: Vec<Row>,
}

/// One row is one agent session: own turns, one commit, one branch.
#[derive(Debug, Clone)]
pub struct Row {
    /// Key of the row: branch, land ref, fail mark, transcript dir.
    pub id: String,
    /// Row indices this row waits for, resolved at parse time.
    pub after: Vec<usize>,
    pub agent: String,
    /// Prompts of the session, sent in order.
    pub turns: Vec<String>,
}

impl Board {
    pub fn from_trunk(git: &Git, config: &Config) -> Result<Self> {
        let text = git
            .show(git::TRUNK, FILE)?
            .ok_or_else(|| Error::Invalid(format!("{FILE}: absent from the trunk branch")))?;
        Self::parse(&text, config)
    }

    pub fn parse(text: &str, config: &Config) -> Result<Self> {
        let written: Vec<Written> = yaml::parse(FILE, text)?;
        for (at, row) in written.iter().enumerate() {
            row.check(at, config)?;
        }
        let index = index_ids(&written)?;
        let rows = written
            .iter()
            .enumerate()
            .map(|(at, row)| row.row(at, &index, config))
            .collect::<Result<Vec<Row>>>()?;
        check_loops(&rows)?;
        Ok(Self { rows })
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// Row as written. Absent `id` still names its place, so both fields default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    #[serde(default)]
    id: String,
    #[serde(default)]
    after: Vec<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    turns: Vec<String>,
}

impl Written {
    fn check(&self, at: usize, config: &Config) -> Result<()> {
        if self.id.is_empty() {
            return Err(self.bad(at, "field `id` must not be empty"));
        }
        if !git::valid_ref(&self.id) {
            return Err(self.bad(
                at,
                &format!(
                    "field `id` must hold a name git takes as a ref: {}",
                    git::REF_RULE
                ),
            ));
        }
        if !config.knows_agent(self.agent(config)) {
            return Err(self.bad(
                at,
                &format!(
                    "field `agent` must name a key of `agents` in {}: {}",
                    crate::config::FILE,
                    config.agent_keys()
                ),
            ));
        }
        self.check_turns(at)
    }

    fn check_turns(&self, at: usize) -> Result<()> {
        if self.turns.is_empty() {
            return Err(self.bad(at, "field `turns` must hold one turn or more"));
        }
        match self.turns.iter().position(|turn| turn.trim().is_empty()) {
            Some(turn) => Err(self.bad(
                at,
                &format!(
                    "field `turns` turn {} must hold the text sent to the agent",
                    turn + 1
                ),
            )),
            None => Ok(()),
        }
    }

    fn agent<'a>(&'a self, config: &'a Config) -> &'a str {
        self.agent.as_deref().unwrap_or(&config.default_agent)
    }

    /// Resolve `after` against the board, own every field.
    fn row(&self, at: usize, index: &BTreeMap<&str, usize>, config: &Config) -> Result<Row> {
        let after = self
            .after
            .iter()
            .map(|id| {
                index.get(id.as_str()).copied().ok_or_else(|| {
                    self.bad(
                        at,
                        &format!("field `after` names id `{id}`, absent from the board"),
                    )
                })
            })
            .collect::<Result<Vec<usize>>>()?;
        Ok(Row {
            id: self.id.clone(),
            after,
            agent: self.agent(config).to_string(),
            turns: self.turns.clone(),
        })
    }

    fn bad(&self, at: usize, what: &str) -> Error {
        let row = match self.id.is_empty() {
            true => format!("row {}", at + 1),
            false => format!("row `{}`", self.id),
        };
        Error::Invalid(format!("{FILE} {row}: {what}"))
    }
}

fn index_ids(written: &[Written]) -> Result<BTreeMap<&str, usize>> {
    let mut index = BTreeMap::new();
    for (at, row) in written.iter().enumerate() {
        if let Some(first) = index.insert(row.id.as_str(), at) {
            return Err(row.bad(
                at,
                &format!(
                    "field `id` must be unique on the board: row {} holds it too",
                    first + 1
                ),
            ));
        }
    }
    Ok(index)
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

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
image: Containerfile.swarm
default_agent: claude
agents:
  claude:
    exec: claude -p
  codex:
    exec: codex exec
commit: Message only.
";

    const EXAMPLE: &str = r#"
- id: parse-10-repair
  turns:
    - |
      Repair the parser.
      Spec in docs/spec/parser.md.
    - &check "Correct, clean, beautiful?"
- id: parse-10-review
  after: [parse-10-repair]
  agent: codex
  turns:
    - "Audit the parser repair. Write the plan, close nothing."
    - *check
"#;

    fn config() -> Config {
        Config::parse(CONFIG).expect("test config must parse")
    }

    #[test]
    fn parses_contract_example() {
        let board = Board::parse(EXAMPLE, &config()).expect("example must parse");
        let rows = board.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].turns[0],
            "Repair the parser.\nSpec in docs/spec/parser.md.\n"
        );
        assert_eq!(rows[0].agent, "claude");
        assert_eq!(rows[1].after, [0]);
        assert_eq!(rows[1].agent, "codex");
        assert_eq!(rows[1].turns[1], "Correct, clean, beautiful?");
    }

    #[test]
    fn names_a_row_with_no_turns() {
        let text = "- id: parse-10-repair\n  turns: []\n";
        let why = Board::parse(text, &config())
            .expect_err("an empty turn list must fail")
            .to_string();
        assert!(why.contains("row `parse-10-repair`"), "{why}");
        assert!(why.contains("`turns`"), "{why}");
    }

    #[test]
    fn names_the_place_of_a_row_with_no_id() {
        let text = EXAMPLE.replace("- id: parse-10-review", "- id: \"\"");
        let why = Board::parse(&text, &config())
            .expect_err("an empty id must fail")
            .to_string();
        assert!(why.contains("row 2"), "{why}");
        assert!(why.contains("`id`"), "{why}");
    }

    #[test]
    fn names_the_ids_of_a_loop() {
        let text = EXAMPLE.replace(
            "- id: parse-10-repair",
            "- id: parse-10-repair\n  after: [parse-10-review]",
        );
        let why = Board::parse(&text, &config())
            .expect_err("a loop in `after` must fail")
            .to_string();
        assert!(
            why.contains("parse-10-repair -> parse-10-review -> parse-10-repair"),
            "{why}"
        );
    }

    #[test]
    fn names_the_row_that_waits_on_no_row() {
        let text = EXAMPLE.replace("after: [parse-10-repair]", "after: [parse-99-missing]");
        let why = Board::parse(&text, &config())
            .expect_err("unknown id must fail")
            .to_string();
        assert!(why.contains("row `parse-10-review`"), "{why}");
        assert!(why.contains("`after`"), "{why}");
    }
}
