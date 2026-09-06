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
