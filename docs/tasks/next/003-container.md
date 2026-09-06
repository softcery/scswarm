# 003 — container per slot

**What.** `init` builds the image and starts one long-lived container per slot. Turns run by `podman exec`.

**Why.** Agents run with permission checks off. Container is the only boundary.

**Done when.** Turn from 002 runs inside the container with worktree rw, gitdir ro, cargo registry rw, project mounts, credentials as env. `git commit` inside the container fails. Host commit works.

## Shape

- `podman.rs`: `build`, `run` (create slot container), `exec`. Sole owner of podman arguments.
- `init.rs`: bare repo, slots, image, containers.
- `turn.rs`: command runs through `podman::exec` instead of host.

## Plan

1. `podman.rs`, `init.rs`.
2. Route `turn.rs` through exec.
3. Check ro gitdir by hand.

## Notes
