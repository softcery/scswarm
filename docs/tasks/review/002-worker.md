# 002 — scheduler and one worker on the host

**What.** `serve` with one worker, agent run on the host, no container. Ready row to landed branch.

**Why.** Turn loop, session resume, host commit and trailers get proven before podman enters.

**Done when.** One board row runs through every stage; branch `swarm/<id>` holds one commit per stage with trailers; `status` shows Landed; kill and restart mid-stage resumes at the next stage from a clean worktree.

## Shape

- `slot.rs`: worktree create, checkout branch from base, reset to commit.
- `turn.rs`: build command from `Agent`, run, parse `session_id` and `result`, append transcript.
- `stage.rs`: run turns of one stage, `{commit}` turn, host commit with trailers.
- `serve.rs`: fetch, derive, assign, loop.

## Plan

1. `slot.rs`, `turn.rs`.
2. `stage.rs` with `{commit}`.
3. `serve.rs`, one worker.
4. Restart test by hand: kill during stage 2, rerun, check branch.

## Notes

Landed: `paths.rs` (swarm home layout), `slot.rs`, `turn.rs`, `stage.rs`, `serve.rs`, git write
paths in `git.rs`, `serve` in `main.rs`.

Verified by hand on a scratch repo, 2 stages, 5 rows: one commit per stage with both trailers,
`status` reads landed, dependant row waits for its parent, fresh session per stage, `--resume
<session>` from turn 2 on, transcript per stage under `~/swarm/log/<project>/<id>/<stage>.jsonl`.
Kill during stage 2 left `junk.txt` and a half-written file; restart reset the worktree to the
stage 1 commit, reran stage 2, and neither artefact reached the tree. Agent exit 1 retried after
1 s and logged `exit` plus stderr. Empty commit message asked again.

Shape changes from the plan:

- `Config::load` and `Board::load` read files no more. `from_main` reads both through
  `git show main:<file>`, so `serve` and `status` see the same board, per contract.
- `Error::Git` is now `Error::Failed`: exit 2 covers git, filesystem and environment failures.
- Config rejects a stage whose `turns` hold no `{commit}`. Without a commit the stage never
  advances and the loop reruns it forever.
- Stage commits pass `--allow-empty`. A stage with no diff still moves the state.
- Turn runs `sh -c '<exec> "$@"' scswarm [<resume>] <prompt>`. The config author keeps quoting
  in `exec`; prompt, resume tokens and session id stay argv.

Open limits:

- One slot, index 0. `workers` is read and checked, not used.
- No lock: two `serve` runs on one project share slot 0 and fight.
- `status` still prints `-` under `worker`. A slot keeps the last branch after landing, so the
  worktree cannot say who works now. Pool task owns that column.
- Base branch is always `main`. Stacked bases land with the pool.
- `fetch` refreshes remote refs only. In a checkout, local `main` moves when the operator pulls,
  so the board is as fresh as the checkout. The bare repo of the contract lands with `init`.

Review fixes:

- A blank commit message re-asked with no wait, one agent process per pass. The retry now waits
  `turn::RETRY_WAIT`.
- A failing turn logged stderr alone. The record now carries stdout too; agents that report errors
  as JSON on stdout were invisible.
- `serve::open` and `Slot::open` shared a name for two ideas. The predicate is `pending`.

