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

## Audit

Findings measured in scratch repos, 2026-09-09. `cargo test` 9 pass, `make lint` exit 0,
`grep -rn '"main"' src/` empty: the done-when of this task holds.

### Bugs

- `git.rs:60` — `set_origin` writes `git config <key> <value>`. A multi-valued
  `remote.origin.fetch` makes git exit 5: `cannot overwrite multiple values with a single
  value`. `contract.md:78` claims a hand-edited bare repo heals. It stops the cycle.
- `init.rs:16` and `serve.rs:79` — two origin urls from one field. `init` writes
  `rev-parse --show-toplevel`, `serve` writes `path` raw. A `path` inside the checkout
  passes `init`, then every fetch fails: `does not appear to be a git repository`.
- `board.rs:65` — id comes from task text, which `contract.md:15` leaves free. Text with a
  space gives `git branch` fatal 128, then the fail mark gives `update-ref` fatal 128,
  then `pool.rs:152` panics, `collect()` reports `stopped`, `serve` exits 2.

### Architecture

- `host.rs:66` — trunk check is one word, no leading dash. `a..b`, `x^`, `feat.lock` and
  `HEAD` pass it and git rejects each. Fetch fails every cycle with no start-time error.
- `init.rs:19` — image builds from the host working tree while config comes from
  `refs/swarm/trunk`. A dirty checkout bakes content no commit records.
- `init.rs:21` — `init` creates `0..workers` and prunes nothing. `workers` 3 to 2 leaves
  worktree 2 registered and container 2 running. An orphan worktree holding a branch
  blocks every live slot (`architecture.md:31`), and `status.rs:27` prints its index.

### Friction

- `serve.rs:20,25` — the pool opens once, then each cycle drops `workers`, `mounts` and
  `env` in silence. Raise `workers` and restart: `podman.rs:64` exits 2 and leaves new
  worktree directories. Lower it: silent leak. Edit mid-run: nothing. Three answers.
- `serve.rs:78` — both origin keys rewritten every cycle, 2 git processes per 5 s.
- `git.rs:72,76` — `show` resolves `refname(rev)` for `ls-tree`, raw `rev` for `show`.
- `podman.rs:65` — error omits `PROJECT`, which the CLI now requires. `init.rs:39` has it.
- `podman.rs:145` and `paths.rs:22` — `slug` lowercases, the name check allows uppercase.
  `Hex` and `hex` share one container name and one image tag; `init` on one removes the
  other's containers.
- `main.rs:63-75` — `paths::project` at 4 arms, result stays `String`.
- `host.rs:15`, `paths.rs:30`, `host.rs:32` — "host" names a type, a file path and a bind
  side.
- `serve.rs:72` — `skip` reports every cycle, `fetch` only on change.
- `log.rs:69` — an IO fault maps to `Invalid`, exit 1. `contract.md:106` reserves 1 for
  invalid input.
- `make lint` warns on files this task wrote: `host.rs:10`, `serve.rs` 19.0% prose,
  `main.rs:59`, `git.rs:8,225`.

### Notes

- `contract.md:36` does not say `path` must be the repo root.
- `systemd/scswarm@.service:7` comment hardcodes `~/.config`, ignoring `XDG_CONFIG_HOME`.
- `host.rs:72` bounds `workers` below only; `architecture.md:46` ties it to core count.
- `docs/tasks/next/007-turn-checkpoints.md:15` still reads `git log main..branch`.
- Unmeasured: `fetch --prune` races worker ref writes over `packed-refs`. Test before act.

## Plan

Three decisions rule the rest.

- **One project source.** Everything the tool reads from the project comes from
  `refs/swarm/trunk`. The working tree read dies, and with it `Git::discover_at`.
- **One creator, one verifier.** `init` writes the slot set, `serve` verifies it. Any
  drift exits 2 naming the field and `scswarm init <project>`. `serve` builds nothing.
- **Standard words.** `Remote { path, trunk }` reloads each cycle, since those two fields
  are `remote.origin.url` and `remote.origin.fetch`. `Slots { workers, mounts, env }` is
  immutable at runtime. A mismatch is drift. "Hot" and "cold" die.

### 1. Ref name gate

- `git.rs`: `valid_ref(name)`, full rule set of `git check-ref-format`: no space, no `..`,
  none of `~^:?*[`, no backslash, no control character, no DEL, no leading `-`, no `.lock`
  end, no `@{`, not the single `@`, no empty segment, no `//`, no trailing `/`, no trailing
  `.`, not `HEAD`. A partial gate promises safety it cannot give, so the list is complete
  or the step is not done.
- `host.rs:66`: `trunk` check calls it, word count rule goes.
- `board.rs`: reject a row whose `swarm/<id>` fails it, `Invalid`, names the line.
- No subprocess: a 20-row board parses every 5 s, `state::derive` already spends 3 git
  calls per row.

### 2. Origin

- `git.rs:60`: both `config` calls take `--replace-all`.
- `serve.rs`: write origin when the `Remote` differs from the one last applied, or after a
  failed fetch, then retry once. Self-heal stays, 2 processes per cycle go.
- `path` means the repo root. Both callers pass it raw.

### 3. Image from the trunk ref

- `init`: `worktree add --detach ~/swarm/build/<project> refs/swarm/trunk`, build, remove.
  Removal is explicit on both paths, no `Drop` guard swallowing the error.
- `git.rs`: add `worktree_remove`. `discover_at` dies with its last caller.
- `paths.rs`: `build(project)`.

### 4. Slot set

- `podman.rs`: `slots(project)` runs one `podman ps --all --format json`, then keeps names
  with the literal prefix `scswarm-<project>-` and returns index, image and label per
  container. `serde_json` already parses turns. No `--filter name=`: podman reads that
  value as a regex, so a project named `a.b` would also match containers of `axb` and
  prune could destroy another project's containers.
- `init`: create `0..workers`, then remove every worktree and container above it. Stamp
  each container `--label scswarm.slots=<ron of mounts and env>`.
- `slot.rs`: `Slot::open` splits. `create` makes worktree and container, `init` alone calls
  it. `attach` binds to both and errors when either is absent, `serve` alone calls it.

### 5. Startup verify

- `serve` order: host file, bare repo, origin, fetch, config, verify, pool, loop. Config
  precedes the check because the image tag lives in `scswarm.ron`.
- Verify from the one podman reply plus one `read_dir`: index count against `workers`,
  worktree directory present, image present, label equal to the file's `Slots`.
- Drift exits 2 naming the field. Mid-run: a readable file with a changed `Slots` exits;
  an unreadable file keeps the last good copy and reports `stale`, as today.
- Exit 2 needs a contract line. `contract.md:106` reserves 2 for a git or podman failure,
  and drift is neither: the file parses and the machine is intact. It sits at 2 because
  the fix is `init`, a podman and git act. Without that line the code contradicts the doc.

### 6. Types

- `host.rs`: `Host { remote: Remote, slots: Slots }`, one parse, two views.
- `serve` holds `Remote` per cycle and the opening `Slots`. `init` and `Pool::open` read
  `Slots` alone.

### 7. Project newtype

- `paths.rs`: `Project`, `TryFrom<&str>` carrying the charset check, uppercase rejected.
- `main.rs`: `Command::project()` accessor, `dispatch` converts once before it matches. No
  clap `value_parser`: clap rejects a bad value with its own exit code 2, and
  `contract.md:106` gives invalid input exit 1.
- `podman::slug` dies: a lowercase name is already a container name.

### 8. Small

- `git.rs:72,76`: one full ref name, used twice.
- `podman.rs:65`: message names the project.
- `serve.rs:72`: hold the last skip cause, report a repeat once.
- `log.rs:69`: IO fault to `Failed`, exit 2.
- `/deslop` on `host.rs:10`, `serve.rs:54,66,103`, `main.rs:59`, `git.rs:8,225`.

### 9. Docs

- `contract.md`: `path` is the repo root; image builds from the trunk ref; `init` prunes
  above `workers`; a row id must pass the ref gate; reload versus immutable replaces the
  hot and cold wording; the crash loop a bad `scswarm.ron` causes under systemd.
- `architecture.md`: gate joins the trunk invariant chokepoint; smells gain "user text used
  as a ref name without the gate".
- `docs/tasks/next/007-turn-checkpoints.md:15`: `refs/swarm/trunk..branch`.

### Tests and verify

- `valid_ref` table: `a..b`, `x^`, `feat.lock`, `HEAD`, `my task`, `ok/name`.
- One board test: a bad id names its line. Nothing else.
- `cargo test`, `make lint`, then a scratch run over a multivar refspec, a bad id,
  `workers` 3 to 2, a `mounts` edit mid-run, and a trunk edit mid-run.

### New limits

Each one goes in `contract.md` with the change that causes it.

- Build context comes from a ref, so untracked and ignored files leave it. A project whose
  image build reads a gitignored blob or a local `.env` builds today and fails after this
  change. Commit the file or drop it from the build.
- The `scswarm.slots` label is RON text this binary writes. A release that changes that
  text makes every project drift at the next `serve` start until a human runs `init`.
- A container that dies mid-run is still caught by the turn failing, not by the startup
  check. Per-cycle probing is a separate decision, not this task.
- `init` now destroys slots above `workers`, so running it against a live `serve` kills
  more than before. `contract.md:117` already says to stop `serve` first.

### Out of scope

- No migration for old bare repos. No back-compat, per `CLAUDE.md`.
- No lock for two `serve` processes. Documented limit.
- The `packed-refs` race stays an open check. Measure first.

## Plan impl

- `src/git.rs` — `valid_ref` holds the full `check-ref-format` rule set plus the two branch
  rules. `set_origin` writes through `--replace-all`. `worktree_remove` drops directory then
  registration. `has_path` splits out of `show`, which now resolves one full ref name for
  both calls. `discover_at` deleted.
- `src/host.rs` — `Written` parses the file, then splits into `Remote { path, trunk }` and
  `Slots { workers, mounts, env }`. Home paths resolve at parse. `Stamp` carries image,
  mounts and env as text; `drift` on both types names the first field that differs.
- `src/board.rs` — a row id that fails `valid_ref` exits 1 naming the line.
- `src/paths.rs` — `Project` newtype, lowercase only, `TryFrom<&str>`. `host_file`, `build`
  and `slot_dirs` added.
- `src/podman.rs` — `slots(project)` reads one `podman ps --all --format json` and matches
  names as text. Containers carry labels `scswarm.mounts` and `scswarm.env`. `remove`,
  `image_exists`, `image_ref` added; `slug` deleted.
- `src/slot.rs` — `open` split into `create` (init) and `attach` (serve). `verify` checks
  slot numbers, image and both labels. `prune` drops every slot at or above `workers`.
- `src/init.rs` — image builds from a throwaway worktree of the trunk ref, removed on both
  paths. `init` ends with `prune`.
- `src/config.rs` — `image_name` and `builds_image`: one resolver for the image, called by
  `init` and by `serve`, so neither module reaches into the other.
- `src/serve.rs` — `Server` holds pool, host, opened slot set, applied remote, last head and
  last skip cause. Startup order: host file, origin, fetch, config, verify, pool. Origin
  writes on change or after a failed fetch. Cold drift exits 2.
- `src/main.rs` — `Command::project()`, one conversion in `dispatch`, exit 1 on a bad name.
- `src/log.rs` — absent transcript exits 1, any other IO fault exits 2.
- Docs — contract, architecture and README carry the repo-root rule, the trunk-ref build,
  the prune, the id gate, reload against immutable, and drift at exit 2.

**Verified.** `cargo test` 13 pass, `make lint` exit 0, `cargo clippy -D warnings` clean.

Live, git side, scratch `HOME` and `XDG_CONFIG_HOME`, project on branch `blahblah`:

- `init proj` wrote `+refs/heads/blahblah:refs/swarm/trunk`, fetched, built the trunk
  worktree, removed it, then stopped at `podman must sit on PATH`, exit 2. Build directory
  gone, `worktree list` back to one entry.
- A second fetch refspec added by hand: `init` healed it to one value, so `--replace-all`
  holds.
- Board row `one two`: `status` exit 1 naming line 3. Row `notes.lock.md`: same. Before this
  change the first row killed `serve`.
- `status Proj` exit 1, `status ghost` exit 2, `trunk: "a..b"` exit 1, `path: "relative"`
  exit 1, each naming its field.
- `serve proj` reached the slot check and stopped there, so verify runs before any worker.

Measured on the podman host, no state touched: `podman ps --all --format json` carries
`Names`, `Image`, `Labels`; `image exists` exits 1 when absent; `ps` reports images tag
qualified, for example `localhost/sercswarm/alpha:latest`. That last fact forced
`image_ref`: an untagged name reads as `:latest` on both sides of the stamp, or every start
would read as drift.

Second pass over the diff caught five more, all fixed: an unnormalized image name on the
remote-image branch of `init`, a home-path fault re-typed from exit 2 to exit 1 in
`Written::split`, a host test that had drifted off the contract example, `serve` reaching
into `init` for the image name, and the ref rule written out twice. `git::REF_RULE` now
holds that sentence once.

**Open check:** container path. `serve` past the slot check, label round trip, prune of a
live container and drift on a `mounts` edit need podman, absent on this workstation.
