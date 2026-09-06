# sercswarm

Autonomous task swarm for any git project. One binary on a workstation. You push `main`, workers do the rest, you merge.

## Flow

1. Add a row to `SWARM.md`: task, dependencies, agent.
2. `git push` `main` to the swarm remote.
3. Swarm picks every ready row: branch `swarm/<id>`, own worktree, own container, stages of scripted agent turns, host commit per stage.
4. `git fetch`, review branch, merge, delete row.

## Files

- `SWARM.md`: board. Contract in [docs/spec/contract.md](docs/spec/contract.md).
- `sercswarm.ron`: workers, image, mounts, agents, stages. Same doc.
- State: git branches and commit trailers only. Design in [docs/refs/architecture.md](docs/refs/architecture.md).

## Commands

```
sercswarm init      bare repo, slots, image
sercswarm serve     scheduler and workers, foreground
sercswarm status    board rows with derived state
sercswarm log ID    transcript of one task
sercswarm reset ID  drop branch of task and dependants
```

## Build

`cargo build --release`, `cargo install --path .`. Lint: `make lint`.

## Limits

- Reads the board from `main` only.
- A task stacked on a rejected parent needs `reset`.
- Agent process errors retry without cap. Expired credentials stall every worker until renewed.
