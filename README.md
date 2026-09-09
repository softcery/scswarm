# scswarm

Runs coding agents on a task board with no human at the keyboard. One binary, one workstation. You push your trunk branch, workers land a branch per task, you review and merge.

## What it does

You write tasks, list them in `SWARM.md` with dependencies, push the trunk. Every 5 seconds the loop:

1. Picks every row whose dependencies are done.
2. Gives each row a slot: own git worktree, own podman container.
3. Runs the agent as scripted turns, grouped in stages. Example stage set: execute, audit, execute again.
4. Commits on the host after each stage, message written by the agent.
5. Leaves branch `swarm/<id>` for you.

Rows stack. A task whose parent is landed but unmerged branches off that parent, so a chain builds on itself while you sleep.

## Why it exists

Agent CLIs run one session, in one directory, while you watch. Three costs follow.

- **You are the scheduler.** Nobody starts task 7 when task 3 lands at 03:00. The board holds the dependency graph, the loop runs it.
- **One directory, one task.** Parallel work needs separate checkouts, containers, branches. A slot is all three.
- **Session state dies with the terminal.** State here sits in git branches and commit trailers only. No database, no state file. Kill the process, restart, every row state rebuilds from the repo.

Design: [docs/refs/architecture.md](docs/refs/architecture.md).

## Why the constraints

- **Host owns git.** Agents get the gitdir read-only. Every commit runs outside the container with the agent's message. An agent cannot rewrite history or land half a stage.
- **Container per slot.** Agent permission checks run off inside. The container is the only boundary, so one rule covers every agent.
- **Board is input only.** The tool writes no file under the project root. Three branches editing one board file conflict on every merge.
- **Spend is bounded.** 3 tries per turn. A failed row records the trunk tip it ran against and waits for a new one, so a broken row costs one attempt per push, not one per cycle.
- **Tool knows no project.** Task dir names, id patterns, prompts and image come from the project's own files; checkout path, trunk branch, workers, mounts and credentials from a host file. A second project needs no code change.

## Files

In your project:

- `SWARM.md`: board. Markdown table, one row per task: task, after, agent.
- `scswarm.ron`: image, agents, stages, merge turns.
- `Containerfile.swarm`: image with the agent CLI and your build tools.

On the host, `~/.config/scswarm/<project>.ron`: checkout path, trunk branch, workers, mounts, env.

Every format binds: [docs/spec/contract.md](docs/spec/contract.md).

## Commands

`PROJECT` is the stem of the host file.

```
scswarm init PROJECT      bare repo, image, slot worktrees and containers
scswarm serve PROJECT     scheduler and workers, foreground
scswarm status PROJECT    board rows with derived state
scswarm log PROJECT ID    transcript of one task
scswarm reset PROJECT ID  drop branch of task and dependants
```

## Build

`cargo build --release`, then `cargo install --path .`. Lint: `make lint`.

Run under systemd: copy the template unit [systemd/scswarm@.service](systemd/scswarm@.service) to `~/.config/systemd/user/`, then `systemctl --user enable --now scswarm@<project>`.

## Limits

- Reads the board from the trunk branch only. A row on any other branch stays invisible.
- Trunk and checkout path reload each cycle. Worker count, mounts and env are baked into the containers, so they need `init`.
- Needs rootless podman. `init` replaces every slot container, so stop `serve` first.
- One `serve` per project. Two share the slot worktrees and fight over the same branches. Nothing locks them.
- A task stacked on a rejected parent needs `reset`. Stop `serve` first.
- One configured agent means one model both writes and audits the work.
- Credentials come from the `serve` environment. A refreshed token needs a restart, then `reset` on each row that failed against the old one.
- Worker count times cargo jobs must fit the core count. Set `CARGO_BUILD_JOBS` in the image.
- No per-turn token or time budget. Agent flags in `exec` carry any cap.
