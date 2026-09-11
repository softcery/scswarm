# Issue: duplicate trailer hides stage commit, stage reruns

Seen on alpha board, 2026-09-11. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `git.rs` `parse_trailer()` compares joined `Swarm-Task` value to row id.
- Commit with `Swarm-Task` twice yields `<id>\x1e<id>`. No match, no error.
- `trailer_commit()` skips that commit and reads older one.
- Host appends trailers to agent text. Agent text can hold `Swarm-*` lines. Host takes them.

## Evidence

- Bare repo `~/swarm/alpha.git`, branch `swarm/trace-29-seed-grid`.
- Commit `483038d` holds `Swarm-Task: trace-29-seed-grid` and `Swarm-Stage: 3/3` twice: once from agent reply, once from host.
- Commit `e1f859f` holds `Swarm-Stage: 2/3` once. `state::derive()` read it: `Running { stage: 3 }`.
- Scheduler journal, `journalctl --user`:
  - 12:53:46 `land task=trace-29-seed-grid`
  - 12:53:46 `assign task=trace-29-seed-grid slot=0 stage=3/3`
  - 13:00:43 `commit ... stage=3/3 sha=3ba7c9b`, then `land` again

## Effect

- Stage 3 ran twice: 6 min 57 s extra agent time, 1 extra commit on branch.
- `land` event comes from worker outcome, not derived state. Log said landed while git said running.
- Agent can write false `Swarm-Stage` value and move row state.

## Fix

- Trailer reader: `Swarm-*` trailer more than once in one commit = error naming commit and key. No skip to older commit.
- Host owns `Swarm-*` keys. Reply holding one = rejected, asked again under 3-try cap. See [issue-commit-message-unchecked.md](issue-commit-message-unchecked.md).
- Overlap: task `007-turn-checkpoints` changes `trailer_commit()`.
