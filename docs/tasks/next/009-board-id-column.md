# 009 — board id column

**What.** Board row gains `id` cell, key of the row. `task` cell becomes free text. Tool derives no id from a path.

**Why.** `board.rs:64` reads a path-like `task` cell as a path and takes its file stem as id. Two effects:

- The tool holds a project layout rule: cell with `/` or `.` and no space is a file path. Invariant `architecture.md:25` bars that.
- `{task}` hands the agent a path frozen at push time. beta run 2026-09-12: stage 1 moved `docs/tasks/next/api-02-pipeline.yaml` to `done/`, stage 2 and 3 prompts still named `next/`. 2 of 3 rows lost the file, each agent picked a folder itself. See [issue-task-slot-path-goes-stale.md](../../notes/issue-task-slot-path-goes-stale.md).

**Done when.** Board `| id | after | agent | task |` parses, `task` holds any text or nothing, `{id}` and `{task}` fill from their cells, `grep -n 'Path\|file_stem' src/board.rs` returns nothing, and a live row with `task` "Search pipeline. Spec in docs/spec/search.md." lands on branch `swarm/<id>`.

## Shape

Board:

```
| id | after | agent | task |
| --- | --- | --- | --- |
| search-01 | | | Search pipeline. Spec in docs/spec/search.md. |
| fix-auth | search-01 | codex | |
```

- `id`: key. Passes `git::valid_ref`. Branch, fail mark, trailer, transcript dir, `reset`, `status`, `after` all name it. Unique per board.
- `task`: any text, `|` aside. Substituted as `{task}`. Empty allowed: prompt `/execute {id}` and the project command resolves the id.
- `after`, `agent`: unchanged.

`board.rs`:

- `COLUMNS` = `["id", "after", "agent", "task"]`. Header check and cell count check name the four.
- `Row { id, task, after, agent }`, all fields. `Row::id()` goes, callers read `row.id`. Stem rule, `Path`, `OsStr` go.
- `check_rows`: `id` empty = error naming field `id`. `valid_ref` check moves to `id`. `task` empty allowed.

`config.rs`: `ID_SLOT` `{id}` beside `TASK_SLOT`. `stage.rs` `fill()` replaces both.

`docs/spec/contract.md`: `SWARM.md` section rewritten to the four cells. `stages[].turns` bullet lists `{id}`. Known limit: `|` cannot sit in `task`, markdown table cell.

## Rules

- No back-compat. Three-cell board fails naming the header.
- Id case: git refs are case-sensitive, macOS default filesystem is not. Contract states ids as lowercase ASCII.
- `fill()` substitutes in one pass over the template. Chained `replace` calls today: `task` text holding `{parent}` gets substituted. Free text must never be re-read as template.

## Steps

1. `board.rs`: columns, `Row`, checks, tests to the four-cell example.
2. `config.rs`, `stage.rs`: `ID_SLOT`, `fill()`.
3. `SWARM.md` of this repo, `scswarm.ron` example prompts: `{id}`.
4. `docs/spec/contract.md`: board section, turns bullet, limit.
5. Verify: `cargo test`, `make lint`, one live row.

## Notes

- 006 rewrites `fill()` for `Turn` kinds. Land 009 first; the slot set is what 006 substitutes.
- Projects with path cells rewrite them to ids. Their command files own the id-to-file lookup.
