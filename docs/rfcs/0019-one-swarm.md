# One swarm, many repos

**What.** One `serve`, one slot pool and one host file run rows of many repos. Each repo is a project entry in the host file.

**Why.**
- One project is one repo: host file stem, bare repo, slot dir, container name, build lock, log dir (`src/paths.rs` lines 53-151, `src/podman.rs` line 247).
- Dozens of client repos today mean dozens of host files, `serve` units and slot pools. No cap on slots across them. No fairness across them.
- A slot binds to one bare repo: the worktree sits in that repo, and the container mounts it read-only (`src/slot.rs` lines 75-87).

**Done when.** `make e2e` runs two projects with different remotes on one `serve` with `workers: 1`. Both projects land rows on their `to`. The single slot switches between the two repos.

## Design

### Host file

One file per unix user: `${XDG_CONFIG_HOME:-~/.config}/scswarm/host.yaml`. One user, one swarm.

```yaml
workers: 8
poll: 5
devices: [/dev/dri]
projects:
  alpha:
    remote: git@github-alpha:client-a/alpha.git
    tracks: track/
    fetch: 60
    mounts: [~/cache/alpha/sccache:/sccache]
    env:
      - match: "*"
        set:
          ANTHROPIC_BASE_URL: RELAY_URL
          ANTHROPIC_AUTH_TOKEN: ALPHA_GATEWAY_KEY
```

- Machine fields: `workers`, `poll`, `devices`.
- Project fields: `remote`, `tracks`, `fetch`, `mounts`, `env`.
- Project name = key under `projects`. Same check as today's `Project::try_from`.
- `poll`: seconds between scheduler cycles, 1 or more. Absent = 5. A cycle reads local refs alone, no network.
- `fetch`: seconds between fetches of one remote, 1 or more. Absent = 60. Fetch picks up human input alone: tracks, `from`, commits on `to`. A worker push of `to` updates the remote-tracking ref, and kept refs live in the bare repo, so rows need no fetch. A new track waits up to `fetch` seconds for its first row.
- `env` rule field `set` maps container variable name to credential file name. Today the file name is the container name (`src/podman.rs` lines 109-110), so two projects cannot pass different values under one name, for example `ANTHROPIC_AUTH_TOKEN`. The value still travels in the podman environment, never argv.
- Check: host paths of `mounts` must not overlap between two projects. The error names both projects and field `mounts`.

### Paths

| Thing | Today | New |
|---|---|---|
| host file | `scswarm/<project>.yaml` | `scswarm/host.yaml` |
| slot lock | slot worktree dir | `~/swarm/slot/<n>` |
| slot worktree | `~/swarm/slot/<project>/<n>` | `~/swarm/slot/<n>/tree` |
| `serve` lock | bare repo | `~/swarm` |
| container | `scswarm-<project>-<n>` | `scswarm-<n>` |
| bare repo, build lock, logs | per project | unchanged |

### Slot

- `Stamp` gains the bare repo path. A slot whose stamp names another repo gets a new container through the remake that exists today (`src/slot.rs` line 78).
- `Slot::prepare` checks the worktree `.git` link names the row's bare repo. Other repo: delete `tree`, prune the old repo's worktree list, add `tree` from the new repo. Ignored files of one project never reach another.
- The slot lock sits on `~/swarm/slot/<n>`, the parent of `tree`. Today the lock is an flock on the worktree dir itself (`src/work.rs` line 67, `src/lock.rs` lines 15-18). A deleted and remade dir has a new inode, so `Pool::busy` reads the slot free while its worker runs (`src/pool.rs` line 145).
- `Slot::open` drops the project argument. The owning repo comes from the `.git` link.

### Key

`state::Key` gains `project` (`src/state.rs` lines 32-35). Today `Pool::holds` compares `track` and `id` alone (`src/pool.rs` line 115), so two projects with one track and row name read as one row.

### Serve

- `serve` holds the lock on `~/swarm`, so one `serve` runs per user. Remote, build and row locks stay per project.
- Each cycle, one thread per project through `std::thread::scope`:
  1. If the project's `fetch` seconds passed since its last fetch, write `origin` and fetch.
  2. If kept refs changed since the last mirror, mirror them. Today's rule (`src/serve.rs` lines 115-124).
  3. Read tracks from local refs.
- A fault skips its own project. Today a failed fetch skips the whole cycle (`src/serve.rs` lines 75-77).
- A project new to the host file opens on first sight: bare repo, `origin`, fetch of remote `refs/swarm/*`, then the first mirror. Without that fetch the first mirror deletes the remote land and fail refs (`docs/spec/contract.md` line 170). `init` goes away.
- A project gone from the host file stops scheduling. Its rows in flight run on.

### Schedule

`Queue` gains a project index. Rank: fewest rows running in the project, then highest `priority`, then fewest rows running in the track, then file order.

Today `priority` ranks first (`src/pool.rs` line 49). Track files set `priority`, so one client can take every slot. Inside one project, `priority` keeps its meaning.

### CLI

```
scswarm serve
scswarm status [PROJECT]
scswarm log PROJECT ROW
scswarm reset PROJECT ROW
scswarm work PROJECT --slot N ...
```

`init` goes. `status` with no project lists every project, with the project as its own column.

### Isolation

Rule: nothing writable is shared across projects. That covers slot worktrees, `mounts` and logs. The host file check enforces `mounts`. The slot switch enforces worktrees. Credentials sit outside the rule: agents read them, never write them, so a shared one, for example `RELAY_URL`, opens no channel between projects.

Levels, strongest last. The tool changes for none of them:

1. One user, rootless podman. The container is the wall (`docs/refs/architecture.md` line 38).
2. Same, plus a VM-backed runtime in `~/.config/containers/containers.conf`, `[engine] runtime = "krun"` or `"runsc"`. Every container of the user runs under it. Linux only: on macOS every container runs in one podman machine VM and shares its kernel.
3. One unix user per client group, one swarm each.
4. One VM per client group.

### Credentials

- Remote: one deploy key per repo through an ssh host alias. GitHub binds a deploy key to one repo.
- Agent: one gateway key per repo when an AI relay fronts the model API. `set` maps it to the variable the agent reads.

### Caches

Build caches ride on per-project `mounts`, for example pnpm store, sccache dir, cargo registry. Do not share `CARGO_TARGET_DIR` across slots: cargo locks it, so rows build one at a time. Slot affinity stays out: with mounted caches a project switch costs a container create and a fresh checkout.

## Steps

1. Spike: `make e2e` under rootless podman with `krun`, then `runsc`, in `containers.conf`. Record the result in this RFC.
2. `host`: machine and project fields, `set` map, `mounts` overlap check.
3. `paths`, `podman`, `slot`, `work`: project-free slot and container names, slot lock on the parent of `tree`, repo in `Stamp`, worktree rehome.
4. `state::Key` gains `project`. `pool::schedule` ranks by project first.
5. `serve`: machine lock, open on first sight, parallel due fetch, faults per project. Delete `init`.
6. CLI and `main`.
7. `tests/e2e.rs`: two projects, one slot.
8. Measure cycle time with 30 projects of one track each, no rows running. Record it in the contract.
9. Docs: contract, architecture, README. Credentials per repo, isolation levels, caches, offboarding paths.

## Notes

- Breaking: host file, paths, container names, CLI, `work` argv. Drain every row before the upgrade. Rows in flight on the old binary cannot finish under the new layout.
- Offboarding a client: remove its project entry, then delete its bare repo, `~/swarm/log/<project>`, `~/swarm/build/<project>`, its credential files and its cache dirs.
- A git or podman failure in one worker still stops `serve` for every project (`docs/refs/architecture.md` line 70). Rows in flight survive.
- Shell access to the swarm user is access to every client on it: transcripts, bare repos, credential files.
- `devices` are shared by every project. GPU memory can leak between processes, for example LeftoverLocals, CVE-2023-4969.
- Network stays on. An agent can send out its own project's code.
- A pnpm store on a bind mount cannot hardlink into the worktree (`EXDEV`). pnpm falls back to copy.
- sccache turns off incremental builds. The crate under work compiles in full each row.
- Unverified: GitHub limits on ssh fetches from one host. 30 projects at `fetch: 60` make 30 fetches a minute. Per-project `fetch` is the lever.
- Unmeasured: git calls per cycle. Each cycle reads every track of every project from local refs, one set of git subprocesses per track. Step 8 measures it.
- Unverified: sccache local disk cache shared by several sccache servers at once.
- Unverified: step 1, rootless podman with `krun` or `runsc` under `--userns=keep-id`, same-path binds, `restart --time=0`, `--device`.
- Out of scope: slot affinity (issue #5), pause on usage limit (issue #6), runtime per project, transcript access without a shell on the swarm host.
