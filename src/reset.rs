use crate::board::Board;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::Git;
use crate::state;

/// Delete the branch of `id` and of every row that waits on it, straight or through others.
pub fn reset(repo: &Git, id: &str) -> Result<()> {
    let config = Config::from_main(repo)?;
    let board = Board::from_main(repo, &config)?;
    let checkouts = repo.checkouts()?;
    let mut dropped = 0;
    for branch in branches(&board, id) {
        if !repo.branch_exists(&branch)? {
            continue;
        }
        if let Some(path) = checkouts.get(&branch) {
            Git::at(path.clone()).detach()?;
        }
        repo.branch_delete(&branch)?;
        println!("deleted {branch}");
        dropped += 1;
    }
    if dropped == 0 {
        return Err(Error::Invalid(format!(
            "id `{id}` must name a task with a branch: none of `{}` or its dependants exists",
            state::branch(id)
        )));
    }
    Ok(())
}

/// Branch of `id`, then of every dependant. An id off the board stands alone:
/// no row can name it, or the board would not parse.
fn branches(board: &Board, id: &str) -> Vec<String> {
    let rows = board.rows();
    let Some(root) = rows.iter().position(|row| row.id() == id) else {
        return vec![state::branch(id)];
    };
    let mut marked = vec![false; rows.len()];
    marked[root] = true;
    let mut grown = true;
    while grown {
        grown = false;
        for (index, row) in rows.iter().enumerate() {
            if marked[index] || !row.after.iter().any(|&parent| marked[parent]) {
                continue;
            }
            marked[index] = true;
            grown = true;
        }
    }
    rows.iter()
        .zip(marked)
        .filter(|(_, hit)| *hit)
        .map(|(row, _)| state::branch(row.id()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
(
  workers: 1,
  image: "Containerfile.swarm",
  default_agent: "claude",
  agents: {"claude": (exec: "claude -p")},
  stages: [(turns: ["{commit}"])],
)
"#;

    /// Third row waits on the first through the second, fourth waits on nothing.
    const BOARD: &str = "\
| task | after | agent |
| --- | --- | --- |
| one | | |
| two | one | |
| three | two | |
| four | | |
";

    #[test]
    fn walks_dependants() {
        let config = Config::parse(CONFIG).expect("test config must parse");
        let board = Board::parse(BOARD, &config).expect("test board must parse");
        assert_eq!(
            branches(&board, "one"),
            ["swarm/one", "swarm/two", "swarm/three"]
        );
        assert_eq!(branches(&board, "four"), ["swarm/four"]);
        assert_eq!(branches(&board, "gone"), ["swarm/gone"]);
    }
}
