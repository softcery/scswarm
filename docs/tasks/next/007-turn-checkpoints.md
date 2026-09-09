# 007 — turn checkpoints

**What.** Host commits the worktree after every turn. Restart resets to the last checkpoint and resumes the same session at the next turn. Stage commit squashes the checkpoints of its stage.

**Why.** Restart loses a whole stage.

- `slot.rs:52` `checkout()` puts the branch at tip, tree clean. `architecture.md` invariant "Stage is atomic" resets to the last stage commit.
- alpha `parse-18-cache`, 2026-09-07: stage 1 ran 4 of 5 turns, 25 files, +561 / -73, no stage commit. Turn 3 killed at 551 s. Restart at `21:42:23Z` dropped the tree. Only transcripts survive.
- Cost of one turn is bounded by `turn.rs:19` tries. Cost of one stage is not: a stage is 5 turns, each turn up to 3 tries, every restart repays all of them.

**Done when.** Kill `serve` mid-turn 3 of a 5-turn stage, restart: worktree holds the turn-2 tree, turn 3 reruns in the same session (`--resume` with the old id), stage commit lands with one commit, no checkpoint commit in `git log refs/swarm/trunk..branch`. `scswarm status` shows `stage 1/3 turn 2/5` for the interrupted row.

## Shape

Trailers, host-owned, on the task branch:

```
Swarm-Task: parse-18-cache      every commit the host makes for the row
Swarm-Stage: 1/3                  stage commit, as today
Swarm-Turn: 1/3 2/5               checkpoint commit: stage, turn
Swarm-Session: 7317ed68-...       checkpoint commit: agent session id
```

`stage.rs` `play()`:

- After every non-commit turn: `slot.checkpoint(stage, turn, session)`. Commit message `checkpoint 1/3 2/5`, host text, no prompt. `--allow-empty`, so turn index and session id persist when the turn changed no file.
- `{commit}` turn: `slot.squash(onto)` then commit with the agent's message and stage trailers, as `stage.rs:112` today. `onto` = newest commit on the branch with no `Swarm-Turn` trailer. That is the last stage commit, or the branch base. `git reset --soft <onto>` keeps the tree, drops the checkpoints.
- Stage start: read resume point from state. `turn > 0` means skip turns `1..=turn`, seed `self.session` from `Swarm-Session`.

`state.rs`:

- `Progress::Stage(done)` gains the turn: `Progress::Stage { done, turn, session }`.
- Read newest `Swarm-Task = id` commit. `Swarm-Stage` present: stage done, `turn = 0`, as today. `Swarm-Turn` present: parse `<stage>/<total> <turn>/<count>`, session from `Swarm-Session`; `done = stage - 1`.
- `git.rs:107` `trailer_commit()` returns one `want` trailer. Extend to a list of wanted keys, one call reads stage, turn, session.

`slot.rs`:

- `checkout()`: branch exists, tree dirty: `git reset --hard <tip>`. Tip is the last checkpoint, so the reset drops the interrupted turn only.
- `checkpoint()`, `squash()`: thin over `git.rs`. `git.rs` stays the sole caller of `git commit` and `git reset`.

`status.rs`: `stage` column reads `1/3 turn 2/5` when a checkpoint is the tip.

`event.rs`: `checkpoint` event, same shape as `commit`.

## Rules

- Resume failure (non-zero exit on `--resume`, container rebuilt, session gone): after the try cap, open a fresh session at turn 1 of the stage. Worktree stays at the checkpoint. Work survives, context does not. State: fresh session id lands in the next checkpoint.
- Checkpoint carries no prompt text and no agent text. Message is tool text. The rule "Host owns git" holds.
- Landed branch shape unchanged: one commit per stage, `Swarm-Stage` trailer. Nothing downstream reads `Swarm-Turn`.
- Interrupted turn is lost, by design. A half-applied turn has no tool results in the session; rerun is the only correct replay.
- No new dependency.

## Steps

1. `git.rs`: `commit_empty(message, trailers)`, `reset_soft(sha)`, `reset_hard(sha)`, `last_without_trailer(branch, key)`, `trailer_commit()` takes `&[&str]` wants.
2. `state.rs`: `Progress::Stage { done, turn, session }`, trailer constants `TURN_TRAILER`, `SESSION_TRAILER`, parse `<stage>/<total> <turn>/<count>`, error names the trailer and the expected shape.
3. `slot.rs`: `checkpoint()`, `squash()`, hard reset on dirty tree in `checkout()`.
4. `stage.rs`: checkpoint after each turn, skip done turns, seed session, squash before stage commit.
5. `status.rs`, `event.rs`: turn in the row line, checkpoint event.
6. `docs/refs/architecture.md`: replace invariant "Stage is atomic" with "Turn is atomic": chokepoint `stage::play` checkpoint after every turn, `state` reads `Swarm-Turn`. Loop step 3: resume point. Known limit: resume failure drops session context.
7. `docs/spec/contract.md`: trailer table, `status` column shape.
8. Tests: `state.rs` parses a checkpoint trailer and a stage trailer, names the bad field. `git.rs` squash on a temp repo: three checkpoints plus one stage commit yield one commit over base. Nothing else.
9. Verify: `cargo build`, `cargo test`, `make lint`, then the "Done when" kill test on a live slot.

## Limits

- Turn killed mid-edit loses that turn's edits. Bounded to one turn.
- Session files live in the container `~/.claude/projects`. Container rebuild (`scswarm init` after an image change) loses every session; every in-flight row restarts its stage from turn 1 on its checkpoint.
- `git reset --soft` before the stage commit rewrites the task branch. Anyone with the branch checked out elsewhere diverges. Slots are the only checkouts.
- 006 changes `stage.rs` turn rendering. Land 006 first, or rebase this on it; both touch `play()`.

## Notes

- Session resume: `turn.rs` passes `resume` argv from `scswarm.ron`. Seeding `self.session` from the trailer needs no new plumbing.
- Turn wall-clock cap is a separate task. Checkpoints bound the loss, not the wait.
