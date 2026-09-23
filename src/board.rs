use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::step::{self, Steps};
use crate::yaml;

pub const FILE: &str = "SWARM.yaml";

/// Project task list. Read only: no write path here.
#[derive(Debug)]
pub struct Board {
    rows: Vec<Row>,
}

/// One row is one session: own steps, one branch, one land ref.
#[derive(Debug, Clone)]
pub struct Row {
    /// Key of the row: branch, land ref, fail mark, transcript dir.
    pub id: String,
    /// Row indices this row waits for, resolved at parse time.
    pub after: Vec<usize>,
    pub agent: String,
    pub steps: Steps,
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
    steps: Vec<step::Written>,
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
        Ok(())
    }

    fn agent<'a>(&'a self, config: &'a Config) -> &'a str {
        self.agent.as_deref().unwrap_or(&config.default_agent)
    }

    /// Resolve `after` against the board, settle steps against `defaults`.
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
        let steps = step::settle("steps", &self.steps, &config.defaults)
            .map_err(|what| self.bad(at, &what))?;
        Ok(Row {
            id: self.id.clone(),
            after,
            agent: self.agent(config).to_string(),
            steps,
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
    use crate::step::Step;

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
- id: rag-04-retriever
  steps:
    - |
      Swap the retriever to hybrid BM25 plus dense.
      Spec in docs/spec/retrieval.md.
    - &check "Correct, clean, beautiful?"
    - commit:
    - run: make bench > bench/out.txt
      timeout: 1800
    - commit: "bench: hybrid retriever"
- id: rag-04-review
  after: [rag-04-retriever]
  agent: codex
  steps:
    - "Read bench/out.txt. Audit the retriever swap against it. Close every gap."
    - *check
    - commit:
"#;

    fn config() -> Config {
        Config::parse(CONFIG).expect("test config must parse")
    }

    fn prompt(step: &Step) -> &str {
        match step {
            Step::Prompt { text, .. } => text,
            other => panic!("step must be a prompt: {other:?}"),
        }
    }

    #[test]
    fn parses_contract_example() {
        let board = Board::parse(EXAMPLE, &config()).expect("example must parse");
        let rows = board.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            prompt(&rows[0].steps[0]),
            "Swap the retriever to hybrid BM25 plus dense.\nSpec in docs/spec/retrieval.md.\n"
        );
        assert!(matches!(rows[0].steps[3], Step::Run { .. }));
        assert_eq!(rows[0].agent, "claude");
        assert_eq!(rows[1].after, [0]);
        assert_eq!(rows[1].agent, "codex");
        assert_eq!(prompt(&rows[1].steps[1]), "Correct, clean, beautiful?");
    }

    #[test]
    fn names_the_row_and_step_of_a_bad_field() {
        let text = EXAMPLE.replace("timeout: 1800", "timeout: 0");
        let why = Board::parse(&text, &config())
            .expect_err("a zero timeout must fail")
            .to_string();
        assert!(why.contains("row `rag-04-retriever`"), "{why}");
        assert!(why.contains("step 4: field `timeout`"), "{why}");
    }

    #[test]
    fn names_the_place_of_a_row_with_no_id() {
        let text = EXAMPLE.replace("- id: rag-04-review", "- id: \"\"");
        let why = Board::parse(&text, &config())
            .expect_err("an empty id must fail")
            .to_string();
        assert!(why.contains("row 2"), "{why}");
        assert!(why.contains("`id`"), "{why}");
    }

    #[test]
    fn names_the_ids_of_a_loop() {
        let text = EXAMPLE.replace(
            "- id: rag-04-retriever",
            "- id: rag-04-retriever\n  after: [rag-04-review]",
        );
        let why = Board::parse(&text, &config())
            .expect_err("a loop in `after` must fail")
            .to_string();
        assert!(
            why.contains("rag-04-retriever -> rag-04-review -> rag-04-retriever"),
            "{why}"
        );
    }

    #[test]
    fn names_the_row_that_waits_on_no_row() {
        let text = EXAMPLE.replace("after: [rag-04-retriever]", "after: [rag-99-missing]");
        let why = Board::parse(&text, &config())
            .expect_err("unknown id must fail")
            .to_string();
        assert!(why.contains("row `rag-04-review`"), "{why}");
        assert!(why.contains("`after`"), "{why}");
    }
}
