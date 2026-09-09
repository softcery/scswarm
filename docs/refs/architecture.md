# Architecture

Why the tool has this shape. Contract: [docs/spec/contract.md](../spec/contract.md).

## Purpose

Fully autonomous execution of every planned task, at any time, on one workstation, with no client machine in the loop. Human writes tasks and merges. Nothing else.

## Pattern

Job queue, worker pool, explicit outcome states, persisted state. Queue = board on the trunk. Workers = slots. State = git. Textbook shape, no framework.

## Invariants

- **Board is input only.** Tool reads `SWARM.md`, writes no file in the project. Chokepoint: `board` module has no write path. Violation: three branches edit one file, merges conflict.
- **State derives from git.** Ready, running, failed, landed, done come from branch existence, trailer commits and the fail mark ref. Chokepoint: `state` module reads git only. Violation: a crash leaves a database and a repo that disagree.
- **Host owns git.** Agents see gitdir read-only. Every commit runs on the host with the agent's message. Chokepoint: `git` module, sole caller of `git commit`. Violation: agent rewrites history or lands a half-stage.
- **Stage is atomic.** Restart resets the worktree to the last stage commit and reruns the next stage in a fresh session. Chokepoint: worker start reads `Swarm-Stage` trailer. Violation: half-edited worktree continues with lost context.
- **Spend is bounded.** Every turn caps its tries, and a failed attempt marks the trunk tip it ran against, so the row waits for new input. Chokepoint: `turn::ATTEMPTS`, fail mark written in `pool`, read in `state`. Violation: one broken agent burns budget every 5 seconds forever.
- **Trunk is a ref, not a name.** The host file declares the branch, the fetch refspec maps it to `refs/swarm/trunk`, and `src/` reads that ref alone. Chokepoint: `git::TRUNK`, refspec built in `git::set_origin`. Violation: a project whose primary branch is not `main` cannot run.
- **Branch names finished work.** The stack merges on a detached head; the ref appears once every parent is in. Chokepoint: `slot::checkout` names the branch last. Violation: a crash mid-merge leaves a branch that skips its parents on the next cycle.
- **Container is the boundary.** Agent permission checks off, container mounts fixed. One rule for every agent. Chokepoint: `podman` module builds every `run` and `exec`. Violation: two sandbox semantics, one agent escapes.
- **One row, one slot.** The scheduler hands a row to a single slot and skips it until that slot reports back. Chokepoint: `pool` holds the row id per slot; `status` reads the same fact from worktree checkouts. Violation: two containers commit one branch.
- **Tool knows no project.** Dir names, ids, prompts and image come from the project file; checkout path, trunk, workers, mounts and env from the host file. Chokepoint: `config` reads the project file, `host` reads the host file. Violation: second project needs a code change.

## Loop

1. Read the host file, write `origin` from it, fetch the trunk into `refs/swarm/trunk`. Parse board and config at that ref. A bad host file keeps the last good copy; a failed fetch skips the cycle.
2. Derive state per row.
3. For each idle slot and open row, in board order: create branch on base, resolve a conflicted parent merge with the `merge` turns, run stages, land. Slots run at once, one row each.
4. Slot detaches its worktree at the end, and at open. Git allows one worktree per branch, so a held branch blocks every other slot.
5. Sleep, repeat.

## Turn

`podman exec <slot> sh -c '<exec> "$@"' scswarm [<resume>] <prompt>`. Prompt, resume and session id ride as argv, so agent text never reaches the shell. Stdout parsed for `session_id` and `result`. Transcript appended to `~/swarm/log/<project>/<id>/<stage>.jsonl`. Non-zero exit: backoff, same turn again.

## Known limits

- One model executes and audits when only one agent is configured.
- Stacked branch on a rejected parent needs `reset`.
- Two landed parents that conflict: the `merge` turns get one try, then the row is marked failed and waits for a new trunk tip.
- `reset` during `serve` races the worker holding that branch.
- Two `serve` processes on one project share the slot worktrees. Nothing locks them.
- Board on the trunk branch only.
- Worker count times cargo jobs must fit the core count; set `CARGO_BUILD_JOBS` in the image.
- Credential expiry fails every row it touches after 3 tries. Fix the credential, then `reset` each marked row or push the trunk.

## Smells

- Any path, dir name or branch name from a project in `src/`.
- A file written under the project root by the tool.
- `git commit` outside the `git` module.
- A `podman` argument outside the `podman` module.
- State kept in memory that a restart cannot rebuild from git.
