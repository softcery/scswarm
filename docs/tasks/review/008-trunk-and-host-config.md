# 008 — trunk ref and host config

**What.** Host file `${XDG_CONFIG_HOME:-~/.config}/scswarm/<project>.ron` declares checkout path, trunk branch, workers, mounts, env. Tool reads the trunk tip at local ref `refs/swarm/trunk` and holds no branch name.

**Why.** Branch name `main` is baked in, and host facts sit in the project file.

- `git.rs:8` `pub const MAIN: &str = "main";`, read at 11 sites. A project whose primary branch is `blahblah` cannot run.
- `git.rs:62` writes refspec `+refs/heads/main:refs/heads/main` at `init`. Branch rename needs a hand edit of bare repo config.
- Trunk name cannot move into `scswarm.ron`: `config.rs:86` reads that file at the trunk revision, so the pointer would live inside the thing it points at.
- `config.rs:21,24,27` hold `workers`, `mounts`, `env`. All three are host facts: core count bounds workers (`architecture.md:45`), mounts carry host paths, env names host credentials. Same project on a second host needs the project file edited.
- `init.rs:13` and `init::open` derive the project name from the working directory, so command output depends on cwd.

**Done when.** A project whose primary branch is not `main` runs a row end to end from a host file. `grep -rn '"main"' src/` prints nothing. Editing `trunk` while `serve` runs moves the base within one cycle, no restart.

## Shape

Host file, one per project, name = file stem:

```
(
  path: "~/alpha",
  trunk: "blahblah",
  workers: 3,
  mounts: ["~/alpha/records:/records", "~/.cargo/registry:/cargo/registry"],
  env: ["ANTHROPIC_API_KEY"],
)
```

Project file `scswarm.ron`, read at `refs/swarm/trunk`: `image`, `default_agent`, `agents`, `stages`, `merge`.

Ownership rule: a field belongs to the host file when a second host running the same project changes it.

### Git

- Refspec is derived, never declared: `+refs/heads/<trunk>:refs/swarm/trunk`. `serve` writes `remote.origin.url` and the refspec each cycle, so a hand-edited bare repo self-heals.
- `src/` holds one ref name, `refs/swarm/trunk`. `MAIN` dies.
- Fail mark moves from `refs/swarm-fail/<id>` to `refs/swarm/fail/<id>`. One tool namespace, both outside `refs/heads/`.
- `git.rs:226` `head_ref` becomes `refname`: pass through when the argument starts with `refs/`, else prefix `refs/heads/`. Same precedence git itself uses. Branch-name callers keep their disambiguation, trunk rides as a full ref.

### Modules

- New `host.rs`. Sole reader of the host file. Type `Host { path, trunk, workers, mounts, env }`. `Mount` moves here from `config.rs:39`.
- `config.rs`: drop `workers`, `mounts`, `env` and their checks; `from_main` becomes `from_trunk`.
- `paths.rs`: add config path from `XDG_CONFIG_HOME`, default `~/.config`. `project()` stops reading a repo root and validates a name instead, same charset check.
- `main.rs`: every subcommand takes `<PROJECT>`. `init --root` dies.
- `init.rs`: `open(project)` loads the host file, then the bare repo. Checkout path comes from the host file, not cwd.
- `serve.rs:31`: reconcile origin, then fetch, then read config at the trunk ref.

### Hot and cold

- Hot, next cycle: `path`, `trunk`.
- Cold, needs `init`: `workers`, `mounts`, `env`. Containers bake them at create time.

### Failures

- Host file absent or unparsable at start: exit 1, name file and field.
- Unparsable mid-run: keep last good, report, keep serving.
- Declared trunk absent on remote: fetch fails, no scheduling that cycle, report each cycle.

## Rules

- Tool learns no branch name. Trunk reaches `src/` as a ref, never as a name.
- Host file is config, not state. Branches and trailers stay the only state.
- Keep no back-compat. Old refspec, old fail refs and the `workers`/`mounts`/`env` fields of `scswarm.ron` die with this change.
- No new dependency. `ron` and `serde` already parse the project file.

## Steps

1. `host.rs`: type, RON parse with `IMPLICIT_SOME`, checks for `path`, `trunk`, `workers`, `env`, `mounts`. Move `Mount` and `check_env` out of `config.rs`.
2. `paths.rs`: `config(project)` under `XDG_CONFIG_HOME`, default `~/.config/scswarm`. Rework `project()` into a name check.
3. `main.rs`: `<PROJECT>` positional on `init`, `serve`, `status`, `log`, `reset`. Drop `--root`.
4. `init.rs`: read host file, open or create bare repo at `paths::repo(project)`, build image from `host.path`, create slots with `host.workers`, `host.mounts`, `host.env`.
5. `git.rs`: `TRUNK` const, `set_origin(url, trunk)` writing both config keys, `refname` rule, delete `MAIN`.
6. `config.rs`, `board.rs`, `state.rs`, `pool.rs`, `serve.rs`, `status.rs`, `reset.rs`: read `git::TRUNK`. Rename `from_main` to `from_trunk`.
7. `state.rs:10`: `FAIL_REFS` to `refs/swarm/fail/`.
8. `serve.rs`: reconcile origin each cycle before fetch.
9. `scswarm.ron`: drop the three host fields. Host file lives outside the checkout, so this repo ships the example in the contract only.
10. `systemd/scswarm.service`: `ExecStart=%h/.cargo/bin/scswarm serve %i`, template unit `scswarm@.service`, drop `WorkingDirectory`.
11. Docs: contract 47-50 (field split), 59-61 (refspec, base), 70 (fail ref), 76-83 (CLI), 95-99 (limits); architecture 11, 19, 27, 41, 44; README 3, 32, 46-47, 61.
12. Tests: `host.rs` parses the example and names the bad field. `config.rs` test loses the three fields. Nothing else.
13. Verify: `cargo build`, `cargo test`, `make lint`, then a live run on a project whose primary branch is not `main`, with `trunk` edited mid-run.

## Limits

- Trunk change mid-run leaves running rows on their old base. Their fail marks hold the old tip, so a failed row retries against the new one.
- Trunk moved to a branch without landed work: those rows read `landed` again, not `done`. Correct by the contract definition.
- Host file holds one project. Two projects need two files and two `serve` processes.
- `workers`, `mounts` and `env` edits need `init`, which replaces every container, so stop `serve` first.
- Project name comes from the file stem, so two checkouts with the same stem need different file names.

## Impl

- `src/host.rs` — new module, sole reader of the host file. `Host { path, trunk, workers, mounts, env }`, `load`, `parse`, `checkout`, checks. `Mount` moved here from `config.rs`. Two tests: contract example parses, a two-word `trunk` names the field.
- `src/paths.rs` — `host(project)` builds `${XDG_CONFIG_HOME:-~/.config}/scswarm/<project>.ron`. `project()` now checks a name instead of reading a repo root; error drops to `Invalid`, exit 1. `HOME` reaches `env::var_os` through a const, same as `XDG_CONFIG_HOME`.
- `src/git.rs` — `TRUNK = "refs/swarm/trunk"` replaces `MAIN`. `set_origin(url, trunk)` writes url and refspec `+refs/heads/<trunk>:refs/swarm/trunk`. `head_ref` became `refname`: a name starting with `refs/` stands, the rest take `refs/heads/`. `branch_commit` became `tip`, since its argument is now a ref. Dead `discover()` deleted with the cwd rule it served.
- `src/config.rs` — `workers`, `mounts`, `env` and `check_env` gone. `from_main` became `from_trunk`.
- `src/state.rs` — fail mark at `refs/swarm/fail/<id>`. Verified in a scratch bare repo: `refs/swarm/trunk` and `refs/swarm/fail/one` coexist, `status` reads the row as `failed`.
- `src/serve.rs` — `reload` rereads the host file each cycle and keeps the last good copy on error (`event::stale`). `fetch` writes origin, fetches, reads the tip; `trunk` turns its failure into a skipped cycle (`event::skip`). Board and config errors stay fatal, per contract.
- `src/pool.rs`, `src/init.rs` — worker count, env and mounts come from `Host`. `init::open(project)` opens the bare repo; the checkout path comes from the host file, never cwd.
- `src/main.rs` — every subcommand takes `<PROJECT>`, validated in `dispatch`. `--root` gone.
- `src/podman.rs` — `Mount` from `host`. The `env` error names the host file, not `scswarm.ron`.
- `scswarm.ron`, `systemd/scswarm@.service`, `README.md`, `docs/spec/contract.md`, `docs/refs/architecture.md` — host fields moved out, template unit takes `%i`, docs carry the host file, the trunk ref and the new limits. Architecture gains the invariant `Trunk is a ref, not a name`.

**Verified.** `cargo build`, `cargo test` (9 pass), `make lint` exit 0 (`serve.rs` 19.5% prose warn, under the 20% fail line; `pool.rs`, `slot.rs`, `paths.rs`, `podman.rs` warns predate this change). `grep -rn '"main"' src/` prints nothing.

Live, git side, on a scratch project whose branch is `blahblah`, with `HOME` and `XDG_CONFIG_HOME` redirected:

- `init proj` wrote refspec `+refs/heads/blahblah:refs/swarm/trunk`, fetched the tip, then failed at the image step with `podman must sit on PATH`, exit 2. No podman on this machine.
- `status proj` printed both rows, `ready` and `blocked`, read at `refs/swarm/trunk`.
- Branch renamed to `other`, host file edited, `init proj` again: refspec now `+refs/heads/other:refs/swarm/trunk`, trunk tip moved to the new commit.
- `status no/such` exit 1, `status missing` exit 2 with the absent bare repo, absent host file exit 1, `path: "relative"` exit 1 naming `field \`path\``, `trunk: "ghost"` exit 2 with `couldn't find remote ref refs/heads/ghost`.
- `reset proj one` cleared the fail mark.

Fetch with `--prune` keeps `refs/swarm/trunk` when the declared branch vanishes: git fails with `couldn't find remote ref` before pruning. Measured in a scratch repo. So a lost trunk skips cycles and never deletes the last tip.

Open check: container path. `serve`, slot containers, mounts and `env` forwarding need podman, absent here.
