# Architecture

Why the tool has this shape. Contract: [docs/spec/contract.md](../spec/contract.md).

## Purpose

Fully autonomous execution of every planned task, at any time, on one workstation, with no client machine in the loop. Human writes tasks and merges. Nothing else.

## Pattern

Job queue, worker pool, explicit outcome states, persisted state. Queue = board on the trunk. Workers = processes, one `scswarm work` per row attempt, one slot each. `serve` schedules only. State = git. Textbook shape, no framework.

## Invariants

- **Board is input only.** Tool reads `SWARM.yaml`, writes no file in the project. Chokepoint: `board` module has no write path. Violation: three branches edit one file, merges conflict.
- **State derives from git.** Ready, running, failed, landed, done come from branch existence, the land ref and the fail mark ref. Chokepoint: `state` module reads git only. Violation: a crash leaves a database and a repo that disagree.
- **Host owns git.** Agents see gitdir read-only. Every commit runs on the host with the agent's message, and nothing in `src/` reads a commit message back. Chokepoint: `git` module, sole caller of `git commit`. Violation: agent rewrites history, or its text steers the loop.
- **Row is atomic.** One row is one session, one branch, one land ref at its last commit. A branch with no land ref holds a session cut short: the slot drops it and replays the row from base. Chokepoint: `slot::checkout` deletes the standing branch. Violation: half-edited worktree continues with lost context.
- **Spend is bounded.** Every exec'd step caps its tries and, with `timeout`, the time per try. A failed attempt marks the trunk tip it ran against, so the row waits for new input. Chokepoint: `step::Policy`, enforced in `call`; fail mark written in `work`, read in `state`. Violation: one broken agent burns budget every cycle forever.
- **Machine fault marks no row.** Spent tries and a conflict left after the `merge` steps end the row as `step::Outcome::Spent`, which writes the fail mark. A git or podman failure returns `Err`: the worker exits 2, `serve` exits 2, no mark. Workers on other slots run on. Chokepoint: `work::Attempt`. Violation: a broken slot marks every row it takes, each waiting for a new trunk tip.
- **No process outlives its try.** The slot container restarts after every exec, and when `serve` attaches the slot: once per `serve` life, the first cycle the slot is idle. Chokepoint: `call::Call` and `slot::attach`, sole callers of `podman::Container::restart`. Violation: a job the agent sent to the background, or one a dead worker left, writes into the next step or the next row on that slot.
- **Values live in config, not code.** Tries, waits, timeouts and poll come from step fields, `defaults` and the host file. Tool defaults sit in one place each: `step` for policy, `host` for poll. Violation: a project needs a release to change a retry count.
- **Trunk is a ref, not a name.** The host file declares the branch, the fetch refspec maps it to `refs/swarm/trunk`, and `src/` reads that ref alone. Every project text that becomes a ref passes `git::valid_ref` first. Chokepoint: `git::TRUNK`, refspec in `git::set_origin`, gate in `host` and `board`. Violation: a project whose primary branch is not `main` cannot run, or a board row kills the loop.
- **Slot set converges to the host file.** `init` makes the bare repo and image. `serve` alone creates, attaches, rebuilds and drops slots, each cycle, against `workers`, `mounts` and the image ID. `env` rides in each job, so it changes no container. A busy slot keeps its row and changes once idle. Chokepoint: `pool::Pool::converge`, sole caller of `slot::create`, `slot::attach` and `slot::remove`. Violation: a slot changes under a running row, or a stale container runs work no one declared.
- **Branch names finished work.** The stack merges on a detached head; the ref appears once every parent is in. Chokepoint: `slot::checkout` names the branch last. Violation: a crash mid-merge leaves a branch that skips its parents on the next cycle.
- **Container is the boundary.** Agent permission checks off, container mounts fixed. One rule for every agent. Chokepoint: `podman` module builds every `run` and `exec`. Violation: two sandbox semantics, one agent escapes.
- **One row, one slot.** The scheduler hands a row to a single slot and skips it while a worker holds it. `serve` takes the row lock before spawn and the worker inherits it as stdin, so it is held from spawn to exit. The worker holds the slot lock for its life; `serve` holds the project lock, so no second scheduler runs. Chokepoint: `lock`, sole caller of `File::lock`; `pool` checks own workers, then probes both locks, before attach, rebuild and assign, and takes the row lock at assign. `status` reads the same fact from worktree checkouts. Violation: two containers commit one branch.
- **Rows outlive `serve`.** Each row attempt is its own process. `serve` holds no row a restart cannot rebuild: kernel locks mark what a live worker holds and drop on its death. Chokepoint: `pool::Pool::assign`, sole spawner of workers. Violation: a restart, an upgrade or a fault on one slot kills every row in flight.
- **Tool knows no project.** Ids and steps come from the board; image, agents, `commit`, `defaults` and `merge` steps from the project file; checkout path, trunk, workers, mounts and env from the host file. Project content reaches the tool at `refs/swarm/trunk` only, image build included. Chokepoint: `board` reads the board, `config` reads the project file, `step` settles every step list, `host` reads the host file. Violation: second project needs a code change.

## Loop

1. Read the host file, write `origin` when it changed, fetch the trunk into `refs/swarm/trunk`. Parse board and config at that ref. An unreadable host file keeps the last good copy; a failed fetch, an invalid board or config, or an absent image skips the cycle. Rows in flight run on.
2. Reap ended workers; a non-zero exit stops `serve` with exit 2. Converge slots: skip each busy slot, attach each fitting slot once per `serve` life, rebuild or drop each idle slot that differs, create each missing one.
3. Derive state per row.
4. For each idle slot and open row no worker holds, in board order: spawn `scswarm work` with the trunk tip and the parents to merge. The worker reads board and config at that tip, creates the branch on it, resolves a conflicted parent merge with the `merge` steps, runs the row steps, writes the land ref at the last commit or the fail mark, exits. Workers run at once, one row each.
5. Worker detaches its worktree at the end; attach detaches it too. Git allows one worktree per branch, so a held branch blocks every other slot.
6. Sleep `poll`, repeat.

## Step

Prompt and agent commit: `podman exec <slot> sh -c '<exec> "$@"' scswarm [<resume>] <prompt>`. Prompt, resume and session id ride as argv, so agent text never reaches the shell. Stdout parsed for `session_id` and `result`. The session id carries across every step of the row.

Run: `podman exec <slot> sh -c <text>`. Board text, trusted as the project's own; no substitution, since row ids take `;`, `$` and `|`.

Literal commit: host commit, no exec.

Each try: capture with deadline, judge, record, restart container. The restart runs when capture breaks too. Fails on non-zero exit, signal, `timeout`, blank commit reply, or no session id from an agent with `resume`. Next try after `wait`, doubling to `wait_max`. Each try and each commit appends one line to `~/swarm/log/<project>/<id>/run.jsonl`, or `merge.jsonl` for a merge session. Chokepoint: `transcript::Transcript`.

Steps join through the worktree alone: no step output reaches a prompt, so prompts stay as typed.

## Known limits

- One model executes and audits when only one agent is configured.
- Stacked branch on a rejected parent needs `reset`.
- Two landed parents that conflict: the `merge` steps get one try, then the row is marked failed and waits for a new trunk tip.
- A row's failure or crash costs its whole step list. The replay starts from base.
- The restart after each try kills servers too. A step that needs one starts and stops it inside one `run`.
- A git or podman failure in one slot stops `serve`. That row replays; workers on other slots run on.
- Podman exit 125 from `exec` counts as a failed try. A step command that exits 125 reads the same.
- `status` prints one line per row. The tool learns no id pattern, so a task written as three rows groups into nothing.
- `reset` races a worker running that row. Stopping `serve` stops no worker.
- `systemctl stop` leaves rows in flight running. Hard stop: `systemctl --user kill --kill-whom=all`.
- Rows started before an upgrade finish on the old binary. `work` argv, branch and ref names must read the same in both versions.
- A worker takes the slot lock after spawn. A new `serve` can attach or use that slot first; the worker waits, or exits 2 if the slot drops.
- Locks need a local filesystem. `~/swarm` on NFS breaks flock.
- A worker hung outside a step holds its slot until killed.
- Board on the trunk branch only.
- A slot due to drop or rebuild waits for its row to end. While slots rebuild, rows run on old and new image at once.
- Worker count times cargo jobs must fit the core count; set `CARGO_BUILD_JOBS` in the image.
- Credential expiry fails every row it touches after 3 tries. Fix the credential, then `reset` each marked row or push the trunk.

## Smells

- Any path, dir name, prompt or branch name from a project in `src/`.
- Project text used as a ref name without `git::valid_ref`.
- A container or worktree created or dropped outside `pool::Pool::converge`.
- A file written under the project root by the tool.
- `git commit` outside the `git` module.
- A `podman` argument outside the `podman` module.
- State kept in memory that a restart cannot rebuild from git and locks.
- A row run as a thread inside `serve`.
- A commit message read for anything but the commit itself.
- A retry count, wait or timeout constant outside `step` and `host`.
- Step output passed into a prompt.
- A fail mark written for a git or podman failure.
