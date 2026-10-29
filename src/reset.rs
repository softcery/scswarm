use crate::board;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::state::{self, Key};

/// Delete branch, land ref, fail refs and stop ref of `key` and of each dependant
/// in its track: here, then on the remote by mirror.
pub fn reset(repo: &Git, tracks: &str, key: &Key) -> Result<()> {
    let checkouts = repo.checkouts()?;
    let kept = repo.refs(git::KEPT)?;
    let mut dropped = 0;
    for row in targets(repo, tracks, key)? {
        let [on_track, on_to] = row.fail_refs();
        for name in [row.land_ref(), on_track, on_to, row.stop_ref()] {
            if kept.contains_key(&name) {
                repo.ref_delete(&name)?;
                println!("deleted {name}");
                dropped += 1;
            }
        }
        let branch = row.branch();
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
            "row `{key}` must name a row with a branch, land, fail or stop ref: none of `{}` or its dependants has one",
            key.branch()
        )));
    }
    repo.mirror_kept()
}

/// `key`, then every dependant in its track. A row off every track stands alone:
/// no row can name it.
fn targets(repo: &Git, tracks: &str, key: &Key) -> Result<Vec<Key>> {
    let scan = board::scan(repo, tracks)?;
    let Some(track) = scan
        .tracks
        .into_iter()
        .find(|track| track.name == key.track)
    else {
        return Ok(vec![key.clone()]);
    };
    let board = track.board(&Config::at(repo, &state::base(repo, &track)?)?)?;
    let ids = board.dependants(&key.id);
    Ok(match ids.is_empty() {
        true => vec![key.clone()],
        false => ids.iter().map(|id| Key::new(&key.track, id)).collect(),
    })
}
