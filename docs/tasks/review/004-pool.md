# 004 — worker pool, log, reset, service

**What.** `workers` slots in parallel, stacked branches, `log`, `reset`, systemd user unit.

**Why.** Independent chains run at once. Operator sees and undoes.

**Done when.** Three ready rows run on three slots at once; a row whose parent is landed and unmerged branches from the parent; `reset` drops a branch and every dependant; `log` tails a transcript; unit file starts `serve` on login and restarts on exit.

## Shape

- `serve.rs`: assignment over idle slots, base branch rule.
- `reset.rs`: dependant walk over the board, branch delete, worktree reset.
- `log.rs`: read transcript.
- `systemd/sercswarm.service`: user unit, `Restart=always`.

## Plan

1. Pool in `serve.rs`.
2. Base branch rule.
3. `reset`, `log`.
4. Unit file.

## Notes

Landed: `pool.rs` (slot threads, job queue, report channel), `serve.rs` (cycle, assignment,
base rule), `status.rs` (table moved out of `main.rs`, worker column), `log.rs`, `reset.rs`,
`systemd/sercswarm.service`. Git gained `branch_delete`, `detach`, `merge`, `merge_abort`,
`checkouts`. Slot gained `release` and a stacked `checkout`. Paths gained `slot_index`,
`transcripts`, `transcript_stage`. `turn::Entry` replaced the `json!` transcript literals.
1 test: dependant walk in `reset`.

Shape changes from the plan:

- Threads live in `pool.rs`, policy in `serve.rs`. Mechanism and rule split; `serve.rs` names no
  channel.
- Base rule is one rule, not two: branch from `main`, then merge every landed unmerged parent.
  A single parent fast-forwards, so the branch equals `swarm/<parent>` as the contract states.
  Two landed parents both ride in, which the "or `swarm/<parent>`" wording could not express.
- Slot detaches its worktree when a task ends or fails, and when `serve` opens it. A branch left
  checked out blocks the next slot that picks that row up.
- `Pool::collect` fails when a slot thread is gone. A panicked thread never reports, so its slot
  would count busy for good.
- `status` worker column reads `git worktree list --porcelain`. Assignment is in the `serve`
  process, so a second process can only read it back out of git.
- Transcript turn numbers are 1-based, matching stage numbers.
- `reset` on an id with no branch at all exits 1 and names it, so a typo does not read as success.

Review fixes:

- A crash left branches checked out in slot worktrees. Git allows one worktree per branch, so the
  next cycle picked the first idle slot, `git checkout` failed with `already checked out at`, and
  the row never ran again. `Slot::open` now detaches: an open slot is idle, so it holds no branch.
- `paths::slot_index` compared an unresolved path against `git worktree list`, which resolves every
  symlink. A symlinked home blanked the `status` worker column. Both sides now resolve.
- Two landed parents that conflict left a half-stacked branch. The next cycle saw the branch,
  skipped the merge and ran the task on a base missing a parent. `Slot::checkout` now aborts the
  merge, detaches and deletes the branch, then reports.
- `proc` reported stderr alone, so a merge conflict read as an empty message. It now falls back to
  stdout, where git states conflicts.
- `branch_create` passes `--no-track`. A swarm branch tracking `origin/main` reads as ahead of it.

Verified by hand, podman replaced by a shim that runs `exec` on the host:

- 3 ready rows, 3 slots, `status` shows workers 0, 1, 2 at once.
- Crash with 3 branches checked out, then restart with one row open: it runs on another slot.
- Worker column reads the slot number with `HOME` behind a symlink.
- Row after a landed parent contains that parent, not its siblings.
- Row after two landed parents contains both, through a merge commit.
- Conflicting parents: error names parent and file, branch dropped, worktree clean, retried.
- Kill mid stage 2, restart: resumes at stage 2, one commit per stage, dirty worktree cleaned.
- `reset` deletes the branch of a task and its dependant, detaching a worktree that held one.
- `log` renders both a reply and a failed turn; unknown id exits 1.
- Branch merged into `main` reads `done`.

Open limits:

- Systemd unit never started: this workstation runs macOS. `WorkingDirectory` needs the project
  checkout path.
- Not run against podman: same workstation. Every podman argument stays as 003 left it.
- A conflicting stack retries every 5 seconds and prints every time. Git holds no place to record
  a conflict, so nothing damps it.
- Nothing locks the slot worktrees. Two `serve` processes on one project fight.
- `workers` is read once at `serve` start. A changed count needs a restart, and `init` for the
  containers.
