# Issue: commit message taken from agent reply unchecked

Seen on alpha board, 2026-09-11. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `stage.rs` `StageRun::commit()` takes `reply.trim()` as message. Rejects only empty reply.
- Prompt `COMMIT_PROMPT` asks "Message only." Reply format has no check.

## Evidence

- Bare repo `~/swarm/alpha.git`, branches `swarm/*`, reflogs of 2026-09-11.
- Agent wraps message in ``` fence. Subject = "```" on 20 of 25 stage commits.
- Commit `3ba7c9b` holds `Swarm-Task` and `Co-Authored-By` lines inside fence.
- Commit `483038d` holds agent-written `Swarm-Task` and `Swarm-Stage` trailers. See [issue-trailer-duplicate.md](issue-trailer-duplicate.md).

## Effect

- `git log --oneline` shows no change summary for swarm commits.
- Agent text reaches host-owned trailers. Cause of trailer-duplicate issue.

## Fix

- Reply check before commit: non-empty subject, no fence line, no `Swarm-` key. Fail = ask again under 3-try cap.
- Claude CLI in container, 2.1.263, has `--json-schema`. Agent returns message as structured output. Host builds message text.
- Structured output is per agent: belongs in adapter. Overlap: task `006-harness-adapters` owns commit prompt and adapters.
