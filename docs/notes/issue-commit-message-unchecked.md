# Issue: commit message taken from agent reply unchecked

Seen on alpha board, 2026-09-11. Seen again on beta board, 2026-09-12. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `script.rs` agent commit step takes `reply.trim()` as message. `call.rs` `judge()` rejects only a blank reply.
- Config `commit` prompt asks "Message only." Reply format has no check.

## Evidence

- Bare repo `~/swarm/alpha.git`, branches `swarm/*`, reflogs of 2026-09-11.
- Agent wraps message in ``` fence. Subject = "```" on 20 of 25 stage commits.
- Commit `3ba7c9b` holds `Swarm-Task` and `Co-Authored-By` lines inside fence.
- Commit `483038d` holds agent-written `Swarm-Task` and `Swarm-Stage` trailers.
- Bare repo `~/swarm/beta.git`, 3 branches, 2026-09-12: subject = "```" on 9 of 9 stage commits. `Swarm-Task` and `Swarm-Stage` once each, so no duplicate trailer on that board.

## Effect

- `git log --oneline` shows no change summary for swarm commits.

## Fix

- Reply check before commit: non-empty subject, no fence line. Fail = ask again under the agent commit `tries` cap.
- Claude CLI in container, 2.1.263, has `--json-schema`. Agent returns message as structured output. Host builds message text.
- Structured output is per agent: belongs in adapter. Overlap: task `006-harness-adapters` owns commit prompt and adapters.
