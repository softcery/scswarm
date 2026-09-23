# Issue: blocked git write inside slot reads as a project defect

Seen on beta board, 2026-09-12. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `podman.rs:97` mounts the bare repo `ro`. `slot.rs:11` leaves `.git` as the worktree link into it.
- Read commands work. Any git write fails, since the index sits under the bare repo.
- Host commits with `git.rs:203` `add --all`, so a plain `mv` lands. Agent has no way to know that.

## Evidence

- `podman exec --workdir=/home/dev/swarm/slot/beta/1 scswarm-beta-1 git status --porcelain`: exit 0.
- Same container, `git mv README.md README2.md`: `fatal: Unable to create '/home/dev/swarm/beta.git/worktrees/1/index.lock': Read-only file system`.
- Task `deployment-01-config` step 1 read "Move this task file into `docs/tasks/progress/` with git mv".
- Its result row reads "The task file moved by mv, not by git mv. The git directory of this worktree mounts read-only." Agent filed the mount as a defect of its own task.

## Effect

- Agent spends turn budget on a git failure it cannot fix.
- Task file carries a false defect row, which the next audit step reads.
- No message states the rule, so each row can repeat it.

## Fix

- Write the rule where the agent reads it. 2 places carry it today, neither reaches the session: `README.md` "Host owns git" and `docs/refs/architecture.md`.
- Option A: `init` writes the rule into the slot container image, for example `/home/dev/swarm/README`. Tool text, no project layout.
- Option B: project states it in its board prompts. Tool changes nothing, every project repeats the text.
- Option A costs 1 file in `Containerfile.swarm` and covers every project.
