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

Landed: `podman.rs` (build, run, exec, sole podman caller), `init.rs` (bare repo, image, slots,
containers), `proc.rs` (subprocess mechanics shared by `git.rs` and `podman.rs`), `init` in
`main.rs`, container in `slot.rs` and `turn.rs`, `Git::init_bare`, `Git::set_origin`,
`Git::discover_at`, `paths::repo`, `paths::arg`, `paths::expand`. 1 test: run arguments bind
worktree rw and bare repo ro.

Shape changes from the plan:

- `serve` and `status` read the bare repo `~/swarm/<project>.git`, not the checkout. `init::open`
  maps checkout to bare repo and fails when `init` never ran. Board and state then come from one
  place for both commands.
- `init` sets bare `remote.origin.url` to the checkout and `remote.origin.fetch` to
  `+refs/heads/main:refs/heads/main`. A cycle fetch moves `main` alone, so no swarm branch is
  pruned.
- Contract dropped the fixed cargo registry mount. A project cache is project shape, so it rides in
  `mounts`. Contract gained `env`: host variable names forwarded with `podman exec --env NAME`, so
  no credential value reaches an argument list.
- `Config.mounts` parses into `Mount { host, guest }` through serde `try_from`, replacing the
  string check.
- Container sleeps under `--entrypoint=sleep`, one `podman exec` per turn. `init` runs
  `podman rm --force --ignore` first, so a rerun picks up a rebuilt image.
- `paths::project` rejects a root name outside ascii letters, digits, `.`, `_`, `-`. Image and
  container names carry it.

Review fixes:

- `git.rs` and `podman.rs` each held spawn, exit code and error text. Both now call `proc.rs`; the
  two modules keep their arguments and nothing else. A missing binary reads `podman must sit on
  PATH`, not `os error 2`.
- Built image tag is `localhost/sercswarm/<project>`. An unqualified tag can resolve to a remote
  image of the same name.
- `Mount` rejects a relative side at parse. podman refuses a relative bind with a message that
  names no field. Its error names the fault, not the part count alone.
- `Spec.gitdir` renamed `repo`: the bind is the whole bare repo, the worktree gitdir sits inside it.
- Names cleaned: `paths::named` to `misnamed`, `podman::forwarded` to `present`, `serve::cycle`
  takes `repo` like its caller. `git.rs` groups constructors before behaviour.

Open limits:

- Not run against podman: this workstation has none. `init` reaches image build and exits 2 with
  `podman must sit on PATH`. Read-only gitdir, `keep-id` and same-path binds wait for a host with
  podman.
- Verified by hand: bare repo created and fetched, origin refspec set, slot worktrees added,
  `status` prints rows, bad mount exits 1 at the named field, rerun of `init` reuses the repo.
- `podman exec --env NAME` passthrough rests on podman docs, not on a run here.
- Slot count still 1 in `serve`. `init` leaves stale containers when `workers` shrinks. Pool task
  owns slot lifecycle.
- `env` values come from the `serve` process. A refreshed credential needs a restart.
