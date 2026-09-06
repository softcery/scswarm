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
