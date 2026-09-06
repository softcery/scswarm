# 001 — skeleton: config, board, status

**What.** `sercswarm status` on a project checkout: parse `sercswarm.ron` and `SWARM.md`, derive state from git, print one line per row.

**Why.** Every later step reads the same three modules. Status proves them without a worker.

**Done when.** On a repo with a board, `status` prints id, state, branch, stage, worker per row. Invalid board or config exits 1 and names the row or field.

## Shape

- `config.rs`: `Config` from RON. `Agent { exec, resume }`, `Stage { agent: Option, turns }`.
- `board.rs`: `Row { id, task, after, agent }` from the markdown table. Id rule from contract.
- `git.rs`: thin wrapper over `git` subprocess: `branch_exists`, `trailer_commit`, `is_ancestor`, `fetch`.
- `state.rs`: `State { Ready, Blocked, Running(stage), Landed, Done }` per row.
- `main.rs`: clap, `status` only.

## Plan

1. `config.rs`, `board.rs` with one parse test each on the contract examples.
2. `git.rs`, `state.rs`.
3. `status` output.

## Notes

Landed: `error.rs` (exit codes 1 invalid, 2 git), `config.rs`, `board.rs`, `git.rs`,
`state.rs`, `status` render in `main.rs`. 4 tests: parse + error naming, config and board.

Shape changes from the plan:

- `git.rs` ships `discover`, `branch_exists`, `trailer_commit`, `is_ancestor`. `fetch` waits for
  its caller in 002; an unused `pub fn` fails `clippy -D warnings`.
- `Row` holds `task`, `after`, `agent`. `id()` derives the id from `task`, so the id has one
  source.
- `main.rs` keeps `status` alone. Stub arms for `init`, `serve`, `log`, `reset` exited 0 with no
  work done; each command returns when its task lands.
- `trailer_commit` picks the commit by `Swarm-Task`, not by branch tip. A stacked branch with no
  commit of its own reads the parent stage otherwise, and shows as landed.

Review fixes:

- `cells` split ate empty trailing cells: `|a|b||` parsed as 2 cells and failed. It now strips one
  pipe at each end.
- `git log <branch>` died with `ambiguous argument` when the project held a path of the same name,
  for example a `swarm/` dir. Every rev argument is now `refs/heads/<branch>`, `git log` ends with
  `--`.

Known limit: `status` reads the worktree copy of the board, so a dirty checkout shows rows `main`
does not carry. `serve` reads the board from `main`.

