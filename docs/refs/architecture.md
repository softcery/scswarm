# Architecture

Why the tool has this shape. Contract: [docs/spec/contract.md](../spec/contract.md).

## Purpose

Fully autonomous execution of every planned task, at any time, on one workstation, with no client machine in the loop. Human writes tasks and merges. Nothing else.

## Pattern

Job queue, worker pool, explicit outcome states, persisted state. Queue = board on the trunk. Workers = slots. State = git. Textbook shape, no framework.

## Invariants

- **Board is input only.** Tool reads `SWARM.yaml`, writes no file in the project. Chokepoint: `board` module has no write path. Violation: three branches edit one file, merges conflict.
- **State derives from git.** Ready, running, failed, landed, done come from branch existence, the land ref and the fail mark ref. Chokepoint: `state` module reads git only. Violation: a crash leaves a database and a repo that disagree.
- **Host owns git.** Agents see gitdir read-only. Every commit runs on the host with the agent's message, and nothing in `src/` reads a commit message back. Chokepoint: `git` module, sole caller of `git commit`. Violation: agent rewrites history, or its text steers the loop.
- **Row is atomic.** One row is one session, one branch, one land ref at its last commit. A branch with no land ref holds a session cut short: the slot drops it and replays the row from base. Chokepoint: `slot::checkout` deletes the standing branch. Violation: half-edited worktree continues with lost context.
- **Spend is bounded.** Every exec'd step caps its tries and, with `timeout`, the time per try. A failed attempt marks the trunk tip it ran against, so the row waits for new input. Chokepoint: `step::Policy`, enforced in `call`; fail mark written in `pool`, read in `state`. Violation: one broken agent burns budget every cycle forever.
- **No process outlives its try.** The slot container restarts after every exec. Chokepoint: `call::Call::run`, sole caller of `podman::Container::restart`. Violation: a job the agent sent to the background writes into the next step or the next row on that slot.
- **Values live in config, not code.** Tries, waits, timeouts and poll come from step fields, `defaults` and the host file. Tool defaults sit in one place each: `step` for policy, `host` for poll. Violation: a project needs a release to change a retry count.
- **Trunk is a ref, not a name.** The host file declares the branch, the fetch refspec maps it to `refs/swarm/trunk`, and `src/` reads that ref alone. Every project text that becomes a ref passes `git::valid_ref` first. Chokepoint: `git::TRUNK`, refspec in `git::set_origin`, gate in `host` and `board`. Violation: a project whose primary branch is not `main` cannot run, or a board row kills the loop.
- **`init` writes the slot set, `serve` reads it.** Worktrees, containers and image are made and dropped by `init` alone. `serve` verifies them and exits on drift. Chokepoint: `slot::create` against `slot::attach`. Violation: a restart grows the filesystem, or a stale container runs work no one declared.
- **Branch names finished work.** The stack merges on a detached head; the ref appears once every parent is in. Chokepoint: `slot::checkout` names the branch last. Violation: a crash mid-merge leaves a branch that skips its parents on the next cycle.
- **Container is the boundary.** Agent permission checks off, container mounts fixed. One rule for every agent. Chokepoint: `podman` module builds every `run` and `exec`. Violation: two sandbox semantics, one agent escapes.
- **One row, one slot.** The scheduler hands a row to a single slot and skips it until that slot reports back. Chokepoint: `pool` holds the row id per slot; `status` reads the same fact from worktree checkouts. Violation: two containers commit one branch.
- **Tool knows no project.** Ids and steps come from the board; image, agents, `commit`, `defaults` and `merge` steps from the project file; checkout path, trunk, workers, mounts and env from the host file. Project content reaches the tool at `refs/swarm/trunk` only, image build included. Chokepoint: `board` reads the board, `config` reads the project file, `step` settles every step list, `host` reads the host file. Violation: second project needs a code change.

## Loop

1. Read the host file, write `origin` when it changed, fetch the trunk into `refs/swarm/trunk`. Parse board and config at that ref. An unreadable host file keeps the last good copy; a failed fetch skips the cycle; a changed slot set stops the loop.
2. Derive state per row.
3. For each idle slot and open row, in board order: create branch on base, resolve a conflicted parent merge with the `merge` steps, run the row steps, write the land ref at the last commit. Slots run at once, one row each.
4. Slot detaches its worktree at the end, and at open. Git allows one worktree per branch, so a held branch blocks every other slot.
5. Sleep `poll`, repeat.

## Step

Prompt and agent commit: `podman exec <slot> sh -c '<exec> "$@"' scswarm [<resume>] <prompt>`. Prompt, resume and session id ride as argv, so agent text never reaches the shell. Stdout parsed for `session_id` and `result`. The session id carries across every step of the row.

Run: `podman exec <slot> sh -c <text>`. Board text, trusted as the project's own; no substitution, since row ids take `;`, `$` and `|`.

Literal commit: host commit, no exec.

Each try: capture with deadline, restart container, judge, record. Fails on non-zero exit, signal, `timeout`, blank commit reply. Next try after `wait`, doubling to `wait_max`. Transcript appended to `~/swarm/log/<project>/<id>/run.jsonl`, or `merge.jsonl` for a merge session.

Steps join through the worktree alone: no step output reaches a prompt, so prompts stay as typed.

## Known limits

- One model executes and audits when only one agent is configured.
- Stacked branch on a rejected parent needs `reset`.
- Two landed parents that conflict: the `merge` steps get one try, then the row is marked failed and waits for a new trunk tip.
- A row's failure or crash costs its whole step list. The replay starts from base.
- The restart after each try kills servers too. A step that needs one starts and stops it inside one `run`.
- `status` prints one line per row. The tool learns no id pattern, so a task written as three rows groups into nothing.
- `reset` during `serve` races the worker holding that branch.
- Two `serve` processes on one project share the slot worktrees. Nothing locks them.
- Board on the trunk branch only.
- Worker count times cargo jobs must fit the core count; set `CARGO_BUILD_JOBS` in the image.
- Credential expiry fails every row it touches after 3 tries. Fix the credential, then `reset` each marked row or push the trunk.

## Smells

- Any path, dir name, prompt or branch name from a project in `src/`.
- Project text used as a ref name without `git::valid_ref`.
- A container or worktree created outside `init`.
- A file written under the project root by the tool.
- `git commit` outside the `git` module.
- A `podman` argument outside the `podman` module.
- State kept in memory that a restart cannot rebuild from git.
- A commit message read for anything but the commit itself.
- A retry count, wait or timeout constant outside `step` and `host`.
- Step output passed into a prompt.
