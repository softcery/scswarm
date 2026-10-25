use std::collections::BTreeMap;
use std::fmt;

use crate::board::{self, Board, Track};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};

const BRANCHES: &str = "swarm/";
/// Kinds of kept ref, under `git::KEPT`.
const LAND: &str = "land/";
const FAIL: &str = "fail/";

/// Row standing. Read from git and row locks, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// A row in `after` is not pushed.
    Waiting,
    /// Can start: no worker holds it.
    Queued,
    Running,
    /// Fail refs equal the track tip and the `to` tip.
    Failed,
    /// Land commit is an ancestor of `to`.
    Pushed,
    /// Land commit is an ancestor of `from`.
    Merged,
}

/// Row of one track. Names branch, land ref, fail refs, transcripts, row lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub track: String,
    pub id: String,
}

/// One track at one fetch: rows settled at `base`, each with its state.
pub struct Snapshot {
    pub track: Track,
    pub board: Board,
    /// `to` tip, else the commit `to` starts at.
    pub base: String,
    pub states: Vec<State>,
}

/// Rows of `track` settled at its base, each with its state. `held`: a worker
/// holds the row.
pub fn snapshot(git: &Git, track: Track, held: &dyn Fn(&Key) -> Result<bool>) -> Result<Snapshot> {
    let base = base(git, &track)?;
    let board = track.board(&Config::at(git, &base)?)?;
    let facts = Facts {
        git,
        kept: git.refs(git::KEPT)?,
        tip: &track.tip,
        to: git.commit_of(&git::remote_branch(&track.to))?,
        from: git.commit_of(&git::remote_branch(&track.from))?,
    };
    let own = board
        .rows()
        .iter()
        .map(|row| facts.own(&Key::new(&track.name, &row.id), held))
        .collect::<Result<Vec<Option<State>>>>()?;
    let states = standing(&board, &own);
    Ok(Snapshot {
        track,
        board,
        base,
        states,
    })
}

/// `to` tip, else the commit `to` starts at: where the next row starts.
pub fn base(git: &Git, track: &Track) -> Result<String> {
    match git.commit_of(&git::remote_branch(&track.to))? {
        Some(tip) => Ok(tip),
        None => track.start(git),
    }
}

/// One fetch on one track, read once for all its rows.
struct Facts<'a> {
    git: &'a Git,
    kept: BTreeMap<String, String>,
    /// Track tip.
    tip: &'a str,
    to: Option<String>,
    /// `None` once `from` leaves the remote: no row reads `merged`.
    from: Option<String>,
}

impl Facts<'_> {
    /// Standing from the row's own refs and lock. `None`: parents decide, also for
    /// a land ref outside `to`, which merges again.
    fn own(&self, key: &Key, held: &dyn Fn(&Key) -> Result<bool>) -> Result<Option<State>> {
        let kept = |name: &str| self.kept.get(name).map(String::as_str);
        if let Some(land) = kept(&key.land_ref()) {
            for (tip, state) in [(&self.from, State::Merged), (&self.to, State::Pushed)] {
                if let Some(tip) = tip {
                    if self.git.is_ancestor(land, tip)? {
                        return Ok(Some(state));
                    }
                }
            }
        }
        if held(key)? {
            return Ok(Some(State::Running));
        }
        let [on_track, on_to] = key.fail_refs();
        let failed = self.to.is_some()
            && kept(&on_track) == Some(self.tip)
            && kept(&on_to) == self.to.as_deref();
        Ok(failed.then_some(State::Failed))
    }
}

impl Key {
    pub fn new(track: &str, id: &str) -> Self {
        Self {
            track: track.to_string(),
            id: id.to_string(),
        }
    }

    pub fn branch(&self) -> String {
        format!("{BRANCHES}{self}")
    }

    /// Ref holding the last commit of the row's session. Sole record of finished work.
    pub fn land_ref(&self) -> String {
        format!("{}{LAND}{self}", git::KEPT)
    }

    /// Track tip and `to` tip of the last failed attempt, in that order.
    pub fn fail_refs(&self) -> [String; 2] {
        ["track", "to"].map(|side| format!("{}{FAIL}{self}/{side}", git::KEPT))
    }
}

impl TryFrom<&str> for Key {
    type Error = Error;

    fn try_from(text: &str) -> Result<Self> {
        match text.split_once('/') {
            Some((track, id)) if board::one_name(track) && board::one_name(id) => {
                Ok(Self::new(track, id))
            }
            _ => Err(Error::Invalid(format!(
                "row must read `<track>/<id>`: two names joined by one `/`, each {}",
                git::REF_RULE
            ))),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.track, self.id)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Waiting => "waiting",
            State::Queued => "queued",
            State::Running => "running",
            State::Failed => "failed",
            State::Pushed => "pushed",
            State::Merged => "merged",
        })
    }
}

/// Own state gated by parents: a row with no state of its own waits until every
/// parent is pushed.
fn standing(board: &Board, own: &[Option<State>]) -> Vec<State> {
    board
        .rows()
        .iter()
        .zip(own)
        .map(|(row, state)| match state {
            Some(state) => *state,
            None if row.after.iter().all(|&parent| settled(own[parent])) => State::Queued,
            None => State::Waiting,
        })
        .collect()
}

/// Parent a dependant can start after.
fn settled(state: Option<State>) -> bool {
    matches!(state, Some(State::Pushed | State::Merged))
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
commit: Message only.
";

    const TRACK: &str = "\
from: main
to: feat/a
rows:
  - id: one
    steps: [Work., {commit: Work.}]
  - id: two
    after: [one]
    steps: [Work., {commit: Work.}]
";

    #[test]
    fn row_waits_until_parent_pushed() {
        let config = Config::parse(CONFIG).expect("test config must parse");
        let track = Track::parse("a", &"0".repeat(40), TRACK).expect("test track must parse");
        let board = track.board(&config).expect("test board must settle");
        let fresh = standing(&board, &[Some(State::Running), None]);
        assert_eq!(fresh, [State::Running, State::Waiting]);
        let pushed = standing(&board, &[Some(State::Pushed), None]);
        assert_eq!(pushed, [State::Pushed, State::Queued]);
    }
}
