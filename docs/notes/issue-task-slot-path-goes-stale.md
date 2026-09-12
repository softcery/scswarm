# Issue: `{task}` path goes stale after agent moves task file

Seen on beta board, 2026-09-12. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `stage.rs:83` `fill()` replaces `config.rs:14` `{task}` with `row.task`, the board cell.
- `board.rs:28` reads the board from trunk. Trunk holds the path the task file had at push time.
- Project stages move the task file between folders. Stage 2 and stage 3 receive the old path.

## Evidence

- Board row: `docs/tasks/next/api-02-pipeline.yaml`.
- Transcript `~/swarm/log/beta/api-02-pipeline/2.jsonl` turn 1 prompt: `/audit the docs/tasks/next/api-02-pipeline.yaml implementation. Go ahead.`
- Branch `swarm/api-02-pipeline` at `32ba050` holds `docs/tasks/done/api-02-pipeline.yaml` and no file under `docs/tasks/next/`.
- Same on `swarm/conversation-01-conversation`: stage 2 and stage 3 prompts name `next/`, file sits in `done/` from stage 1 on.
- 2 of 3 rows in this run moved the file in stage 1. Third row left it in `progress/`.

## Effect

- Every stage after a move opens with a path that resolves to nothing.
- Agent spends turn budget locating the file, then picks the folder itself. Folder state splits: 2 rows in `done/`, 1 row in `progress/`.
- `api-02-pipeline` sits in `done/` while it runs stage 3 with 11 unexecuted steps.

## Fix

- Add `{id}` slot in `config.rs`, filled from `board.rs:64` `Row::id()`. Project prompts name the id, not the path.
- Project then owns the lookup rule in its own prompt text. Tool learns no task folder name.
- Board cell stays a path, since the id derives from it.
