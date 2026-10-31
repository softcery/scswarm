use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::step::{self, Steps};
use crate::yaml;

pub const FILE: &str = "SWARM.yaml";
/// Length range of a commit sha git resolves.
const SHA: std::ops::RangeInclusive<usize> = 4..=40;

/// Tracks that pass their checks, and the fault of each that fails.
pub struct Scan {
    pub tracks: Vec<Track>,
    pub faults: Vec<Error>,
}

/// One track branch: what to run, from where, into which branch. Read only.
#[derive(Debug)]
pub struct Track {
    /// Branch name minus the host file `tracks` prefix. Keys every row.
    pub name: String,
    /// Track branch commit the file comes from.
    pub tip: String,
    /// Branch the work belongs to.
    pub from: String,
    /// Commit `to` starts at. `None`: tip of `from` when `to` is made.
    pub at: Option<String>,
    /// Output branch: rows start at its tip and merge back into it.
    pub to: String,
    pub priority: i32,
    drafts: Vec<Draft>,
    /// What errors call the file this came from.
    label: String,
}

/// Rows of one track, settled against `scswarm.yaml` at the `to` tip.
#[derive(Debug)]
pub struct Board {
    rows: Vec<Row>,
}

/// One row is one session: own steps, one branch, one land ref.
#[derive(Debug)]
pub struct Row {
    pub id: String,
    /// Row indices this row waits for, resolved at parse time.
    pub after: Vec<usize>,
    pub agent: String,
    /// File at the worktree root that becomes the next track file.
    pub publish: Option<String>,
    pub steps: Steps,
}

/// Text that passed `Track::check_next`: all that may reach a track branch.
pub struct NextFile(String);

/// Read every track branch under `prefix`. A failed check skips that track alone.
pub fn scan(git: &Git, prefix: &str) -> Result<Scan> {
    let branches = git.branches()?;
    let mut scan = Scan {
        tracks: Vec::new(),
        faults: Vec::new(),
    };
    for (branch, tip) in &branches {
        let Some(name) = branch.strip_prefix(prefix) else {
            continue;
        };
        match read(git, prefix, name, tip) {
            Ok(track) => scan.tracks.push(track),
            Err(why @ Error::Invalid(_)) => scan.faults.push(why),
            Err(why) => return Err(why),
        }
    }
    scan.part_shared_to();
    Ok(scan)
}

/// One track by name, checked alone. `None`: no such branch.
pub fn track(git: &Git, prefix: &str, name: &str) -> Result<Option<Track>> {
    match git.commit_of(&git::remote_branch(&branch(prefix, name)))? {
        Some(tip) => read(git, prefix, name, &tip).map(Some),
        None => Ok(None),
    }
}

/// Branch of track `name` under `prefix`.
pub fn branch(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}")
}

fn read(git: &Git, prefix: &str, name: &str, tip: &str) -> Result<Track> {
    let track = Track::at(git, name, tip)?;
    track.check_prefix(prefix)?;
    Ok(track)
}

impl Scan {
    /// Two tracks on one `to` would push over each other: both skip.
    fn part_shared_to(&mut self) {
        let mut names: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for track in &self.tracks {
            names.entry(&track.to).or_default().push(&track.name);
        }
        let shared: BTreeMap<String, String> = names
            .into_iter()
            .filter(|(_, names)| names.len() > 1)
            .map(|(to, names)| (to.to_string(), names.join(", ")))
            .collect();
        let (kept, parted): (Vec<Track>, Vec<Track>) = std::mem::take(&mut self.tracks)
            .into_iter()
            .partition(|track| !shared.contains_key(&track.to));
        self.tracks = kept;
        for track in parted {
            self.faults.push(track.invalid(&format!(
                "field `to` must name a branch no other track names: tracks {} name it",
                shared[&track.to]
            )));
        }
    }
}

/// Track file as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    from: String,
    #[serde(default)]
    at: Option<String>,
    to: String,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    rows: Vec<Draft>,
}

/// Row as written. Absent `id` still names its place, so both fields default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    #[serde(default)]
    id: String,
    #[serde(default)]
    after: Vec<String>,
    #[serde(default)]
    agent: Option<String>,
    /// `Some(None)`: key present with no value.
    #[serde(default, deserialize_with = "step::present")]
    publish: Option<Option<String>>,
    #[serde(default)]
    steps: Vec<step::Written>,
}

impl Track {
    /// Track file at commit `tip` of the branch of track `name`.
    pub fn at(git: &Git, name: &str, tip: &str) -> Result<Self> {
        let text = git
            .show(tip, FILE)?
            .ok_or_else(|| Error::Invalid(format!("{}: absent at `{tip}`", label(name))))?;
        Self::parse(name, tip, &text)
    }

    /// Checks what the file alone states.
    pub fn parse(name: &str, tip: &str, text: &str) -> Result<Self> {
        if !one_name(name) {
            let got = match name.contains('/') {
                true => "a name with `/`",
                false => "a name git refuses",
            };
            return Err(Error::Invalid(format!(
                "track branch: name past the `tracks` prefix must hold no `/` and pass as a ref: {}; got {got}",
                git::REF_RULE
            )));
        }
        Self::read(name, tip, text, label(name))
    }

    fn read(name: &str, tip: &str, text: &str, label: String) -> Result<Self> {
        let written: Written = yaml::parse(&label, text)?;
        let track = Self {
            name: name.to_string(),
            tip: tip.to_string(),
            from: written.from,
            at: written.at,
            to: written.to,
            priority: written.priority,
            drafts: written.rows,
            label,
        };
        track.check_file()?;
        Ok(track)
    }

    /// Settle every row against the project file at the base of the rows.
    pub fn board(&self, config: &Config) -> Result<Board> {
        let index = self.index()?;
        let rows = self
            .drafts
            .iter()
            .enumerate()
            .map(|(at, draft)| self.row(at, draft, &index, config))
            .collect::<Result<Vec<Row>>>()?;
        Ok(Board { rows })
    }

    /// Row `id` stands in the file. No config read.
    pub fn has_row(&self, id: &str) -> bool {
        self.drafts.iter().any(|draft| draft.id == id)
    }

    /// Check `text` of worktree file `file` as the next file of this track: same `from`
    /// and `to`, row `id` kept with no `after`, every row settled against each config.
    pub fn check_next(
        &self,
        file: &str,
        text: String,
        id: &str,
        configs: &[&Config],
    ) -> Result<NextFile> {
        let label = format!("track `{}` next file `{file}`", self.name);
        let next = Self::read(&self.name, &self.tip, &text, label)?;
        for (field, running, written) in
            [("from", &self.from, &next.from), ("to", &self.to, &next.to)]
        {
            if running != written {
                return Err(next.invalid(&format!(
                    "field `{field}` must name `{running}`, the branch of the running track"
                )));
            }
        }
        let carried = next.drafts.iter().find(|draft| draft.id == id);
        if !carried.is_some_and(|draft| draft.after.is_empty()) {
            return Err(next.invalid(&format!(
                "field `rows` must hold row `{id}` with no `after`: a retry of this row reads this file"
            )));
        }
        for config in configs {
            next.board(config)?;
        }
        Ok(NextFile(text))
    }

    /// Commit `to` starts at: `at` on `from`, else the `from` tip. Read until `to`
    /// stands; then `from` and `at` start nothing.
    pub fn start(&self, git: &Git) -> Result<String> {
        let from = git
            .commit_of(&git::remote_branch(&self.from))?
            .ok_or_else(|| self.invalid("field `from` must name a branch of the remote"))?;
        let Some(at) = &self.at else {
            return Ok(from);
        };
        let at = git
            .commit_of(at)?
            .ok_or_else(|| self.invalid("field `at` must name a commit of the remote"))?;
        match git.is_ancestor(&at, &from)? {
            true => Ok(at),
            false => Err(self.invalid("field `at` must name a commit on `from`")),
        }
    }

    fn check_file(&self) -> Result<()> {
        for (field, value) in [("from", &self.from), ("to", &self.to)] {
            if !git::valid_ref(value) {
                return Err(self.invalid(&format!(
                    "field `{field}` must name a branch git takes: {}",
                    git::REF_RULE
                )));
            }
        }
        if self.to == self.from {
            return Err(self.invalid(
                "field `to` must name a branch other than `from`: rows push to `to`, humans merge it into `from`",
            ));
        }
        let hex = |at: &str| SHA.contains(&at.len()) && at.chars().all(|c| c.is_ascii_hexdigit());
        if self.at.as_deref().is_some_and(|at| !hex(at)) {
            return Err(self.invalid(&format!(
                "field `at` must hold a commit sha of {} to {} hex digits, or stay absent",
                SHA.start(),
                SHA.end()
            )));
        }
        for (at, draft) in self.drafts.iter().enumerate() {
            if draft.id.is_empty() {
                return Err(self.bad(at, draft, "field `id` must not be empty"));
            }
            if !one_name(&draft.id) {
                return Err(self.bad(
                    at,
                    draft,
                    &format!(
                        "field `id` must hold no `/` and pass as a ref: {}",
                        git::REF_RULE
                    ),
                ));
            }
            if draft.publish.as_ref().is_some_and(|file| !root_file(file)) {
                return Err(self.bad(
                    at,
                    draft,
                    &format!(
                        "field `publish` must name a file at the worktree root: no `/`, not `.`, `..` or `{}`",
                        git::WORKTREE_LINK
                    ),
                ));
            }
        }
        let index = self.index()?;
        let after = self
            .drafts
            .iter()
            .enumerate()
            .map(|(at, draft)| self.after(at, draft, &index))
            .collect::<Result<Vec<Vec<usize>>>>()?;
        self.check_loops(&after)
    }

    /// `to` outside the track branches.
    fn check_prefix(&self, prefix: &str) -> Result<()> {
        match self.to.starts_with(prefix) {
            true => Err(self.invalid(&format!(
                "field `to` must name a branch outside the `tracks` prefix `{prefix}`"
            ))),
            false => Ok(()),
        }
    }

    fn index(&self) -> Result<BTreeMap<&str, usize>> {
        let mut index = BTreeMap::new();
        for (at, draft) in self.drafts.iter().enumerate() {
            if let Some(first) = index.insert(draft.id.as_str(), at) {
                return Err(self.bad(
                    at,
                    draft,
                    &format!(
                        "field `id` must be unique in the track: row {} holds it too",
                        first + 1
                    ),
                ));
            }
        }
        Ok(index)
    }

    fn after(&self, at: usize, draft: &Draft, index: &BTreeMap<&str, usize>) -> Result<Vec<usize>> {
        draft
            .after
            .iter()
            .map(|id| {
                index.get(id.as_str()).copied().ok_or_else(|| {
                    self.bad(
                        at,
                        draft,
                        &format!("field `after` names id `{id}`, absent from the track"),
                    )
                })
            })
            .collect()
    }

    /// Resolve `after` against the track, settle steps against `defaults`.
    fn row(
        &self,
        at: usize,
        draft: &Draft,
        index: &BTreeMap<&str, usize>,
        config: &Config,
    ) -> Result<Row> {
        let agent = draft.agent.as_deref().unwrap_or(&config.default_agent);
        if !config.knows_agent(agent) {
            return Err(self.bad(
                at,
                draft,
                &format!(
                    "field `agent` must name a key of `agents` in {}: {}",
                    crate::config::FILE,
                    config.agent_keys()
                ),
            ));
        }
        let steps = step::settle("steps", &draft.steps, &config.defaults)
            .map_err(|what| self.bad(at, draft, &what))?;
        Ok(Row {
            id: draft.id.clone(),
            after: self.after(at, draft, index)?,
            agent: agent.to_string(),
            publish: draft.publish.clone().flatten(),
            steps,
        })
    }

    /// Row on the walk path twice = loop. Such a row never settles, so every row
    /// behind it reads `waiting` for good.
    fn check_loops(&self, after: &[Vec<usize>]) -> Result<()> {
        let mut marks = vec![Mark::Fresh; after.len()];
        let mut path = Vec::new();
        for start in 0..after.len() {
            if let Some(ids) = walk(after, start, &mut marks, &mut path) {
                let ids: Vec<&str> = ids.iter().map(|&at| self.drafts[at].id.as_str()).collect();
                return Err(self.invalid(&format!(
                    "field `after` must name no loop: {}",
                    ids.join(" -> ")
                )));
            }
        }
        Ok(())
    }

    /// Row named by `id`, or by its place when `id` cannot name it.
    fn bad(&self, at: usize, draft: &Draft, what: &str) -> Error {
        let row = match one_name(&draft.id) {
            true => format!("row `{}`", draft.id),
            false => format!("row {}", at + 1),
        };
        Error::Invalid(format!("{} {row}: {what}", self.label))
    }

    /// Fault of this track, named by track and file.
    pub fn invalid(&self, what: &str) -> Error {
        Error::Invalid(format!("{}: {what}", self.label))
    }
}

impl NextFile {
    pub fn text(&self) -> &str {
        &self.0
    }
}

impl Board {
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|row| row.id == id)
    }

    /// `id`, then every row that waits on it, straight or through others.
    pub fn dependants(&self, id: &str) -> Vec<&str> {
        let Some(root) = self.rows.iter().position(|row| row.id == id) else {
            return Vec::new();
        };
        let mut marked = vec![false; self.rows.len()];
        marked[root] = true;
        let mut grown = true;
        while grown {
            grown = false;
            for (index, row) in self.rows.iter().enumerate() {
                if !marked[index] && row.after.iter().any(|&parent| marked[parent]) {
                    marked[index] = true;
                    grown = true;
                }
            }
        }
        self.rows
            .iter()
            .zip(marked)
            .filter(|(_, hit)| *hit)
            .map(|(row, _)| row.id.as_str())
            .collect()
    }
}

/// Depth-first walk of `after`.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Fresh,
    OnPath,
    Walked,
}

/// Row indices of the first loop found, from where it opens, closing row last.
fn walk(
    after: &[Vec<usize>],
    at: usize,
    marks: &mut [Mark],
    path: &mut Vec<usize>,
) -> Option<Vec<usize>> {
    match marks[at] {
        Mark::Walked => return None,
        Mark::OnPath => {
            let open = path.iter().position(|&on| on == at)?;
            return Some(path[open..].iter().copied().chain([at]).collect());
        }
        Mark::Fresh => {}
    }
    marks[at] = Mark::OnPath;
    path.push(at);
    for &parent in &after[at] {
        if let Some(ids) = walk(after, parent, marks, path) {
            return Some(ids);
        }
    }
    path.pop();
    marks[at] = Mark::Walked;
    None
}

/// Track name or row id: one ref path part, so `<track>/<id>` splits one way.
pub fn one_name(name: &str) -> bool {
    !name.contains('/') && git::valid_ref(name)
}

/// One name at the worktree root, so no symlink directory reaches a host file, and
/// no worktree link: the host deletes this file before the last commit.
fn root_file(file: &Option<String>) -> bool {
    file.as_deref().is_some_and(|name| {
        !name.trim().is_empty()
            && !name.contains('/')
            && !matches!(name, "." | "..")
            && !name.eq_ignore_ascii_case(git::WORKTREE_LINK)
    })
}

/// File name in errors: track name and file.
fn label(name: &str) -> String {
    format!("track `{name}` {FILE}")
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
from: main
to: feat/laser
priority: 10
rows:
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

    fn track(text: &str) -> Result<Track> {
        Track::parse("laser", &"0".repeat(40), text)
    }

    fn board(text: &str) -> Result<Board> {
        let config = Config::parse(CONFIG).expect("test config must parse");
        track(text)?.board(&config)
    }

    fn prompt(step: &Step) -> &str {
        match step {
            Step::Prompt { text, .. } => text,
            other => panic!("step must be a prompt: {other:?}"),
        }
    }

    #[test]
    fn parses_contract_example() {
        let parsed = track(EXAMPLE).expect("example must parse");
        assert_eq!(
            (parsed.from.as_str(), parsed.to.as_str()),
            ("main", "feat/laser")
        );
        assert_eq!(parsed.priority, 10);
        let board = board(EXAMPLE).expect("example must settle");
        let rows = board.rows();
        assert_eq!(
            prompt(&rows[0].steps[0]),
            "Swap the retriever to hybrid BM25 plus dense.\nSpec in docs/spec/retrieval.md.\n"
        );
        assert!(matches!(rows[0].steps[3], Step::Run { .. }));
        assert_eq!(rows[1].after, [0]);
        assert_eq!(rows[1].agent, "codex");
        assert_eq!(
            board.dependants("rag-04-retriever"),
            ["rag-04-retriever", "rag-04-review"]
        );
    }

    #[test]
    fn names_the_row_and_step_of_a_bad_field() {
        let text = EXAMPLE.replace("timeout: 1800", "timeout: 0");
        let why = board(&text)
            .expect_err("a zero timeout must fail")
            .to_string();
        assert!(
            why.contains("track `laser` SWARM.yaml row `rag-04-retriever`"),
            "{why}"
        );
        assert!(why.contains("step 4: field `timeout`"), "{why}");
    }

    #[test]
    fn names_the_ids_of_a_loop() {
        let text = EXAMPLE.replace(
            "- id: rag-04-retriever",
            "- id: rag-04-retriever\n    after: [rag-04-review]",
        );
        let why = track(&text)
            .expect_err("a loop in `after` must fail")
            .to_string();
        assert!(
            why.contains("rag-04-retriever -> rag-04-review -> rag-04-retriever"),
            "{why}"
        );
    }

    #[test]
    fn next_file_keeps_the_branches_and_the_running_row() {
        let config = Config::parse(CONFIG).expect("test config must parse");
        let running = track(EXAMPLE).expect("example must parse");
        let check =
            |text: &str, id| running.check_next("next.yaml", text.to_string(), id, &[&config]);
        check(EXAMPLE, "rag-04-retriever").expect("same file must pass");
        for (text, id, field) in [
            (
                EXAMPLE.replace("feat/laser", "feat/other"),
                "rag-04-retriever",
                "`to`",
            ),
            (EXAMPLE.to_string(), "gone", "`rows`"),
            (EXAMPLE.to_string(), "rag-04-review", "`rows`"),
        ] {
            let why = check(&text, id).err().expect("next file must fail");
            let why = why.to_string();
            assert!(why.contains("next file `next.yaml`"), "{why}");
            assert!(why.contains(&format!("field {field}")), "{why}");
        }
        let other = Config::parse(&CONFIG.replace("  codex:\n    exec: codex exec\n", ""))
            .expect("other config must parse");
        running
            .check_next(
                "next.yaml",
                EXAMPLE.to_string(),
                "rag-04-retriever",
                &[&config, &other],
            )
            .err()
            .expect("an agent one config lacks must fail");
    }

    #[test]
    fn publish_names_one_file_at_the_worktree_root() {
        let with = |value: &str| {
            EXAMPLE.replace(
                "- id: rag-04-review",
                &format!("- id: rag-04-review\n    publish: {value}"),
            )
        };
        let settled = board(&with("next.yaml")).expect("a root file must pass");
        let row = settled.row("rag-04-review").expect("row must stand");
        assert_eq!(row.publish.as_deref(), Some("next.yaml"));
        for value in ["a/next.yaml", ".", "..", ".GIT", ""] {
            let why = track(&with(value)).expect_err(value).to_string();
            assert!(why.contains("field `publish`"), "{value}: {why}");
        }
    }

    #[test]
    fn rejects_a_slash_in_an_id() {
        let text = EXAMPLE.replace("- id: rag-04-review", "- id: rag/04");
        let why = track(&text)
            .expect_err("an id with `/` must fail")
            .to_string();
        assert!(why.contains("row 2: field `id`"), "{why}");
    }
}
