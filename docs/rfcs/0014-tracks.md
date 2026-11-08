# Tracks replace the trunk

**What.** Each track of work is an orphan branch holding its board. Rows start from and merge into the track's `to` branch, which the tool pushes to the remote. The trunk goes.

**Why.** One trunk per project blocks many teams on one repo.

- Contract: `trunk` (docs/spec/contract.md:95) names one branch. Every row starts there (contract.md:140). The board, config and image come from that one ref (contract.md:128, src/git.rs:9).
- With 10 teams on one board, all teams edit one file, race to push it, and have no owner per row. GitHub sets push rights per branch, not per file.
- Row branches stay in the VM's bare repo (contract.md:137). A team without VM access sees no output.
- Usage on `dev@fedora.tailac7dec.ts.net`, 2026-10-22, 8 host files, 5 `serve` units running:
  - Logs name groups of rows "lane" 116 times in mini-rts and 69 in hexasim, plus "track", "phase" and "chain".
  - One board commit adds a whole lane: mini-rts `bde6e7c9`, `14377941`.
  - One merge takes the last rows of 3 lanes: mini-rts `92470541`, "Merge swarm/laser-fix-1, swarm/pathfinding-respec-3 and swarm/net-release-audit-7, with view-15-laser-bolt, …".
  - hexasim merges middle rows of one lane one by one: `f02ab00`, `00c6455`, `b285b96`.
  - mini-rts runs with `trunk: feat/weather`, that is, one feature branch as trunk.
  - None of mini-rts, sercos or hexasim has a git remote. All merges happen on the VM.

**Done when.** On a test repo with a remote, two tracks with different `from` branches each push a `to` branch that holds every row. `scswarm status` in a laptop clone shows `pushed` for each row.

## Design

### Split

| Side | Holds | Lives in |
|---|---|---|
| meta | what to run, order, base, output | track file, orphan branch |
| work | code, task files, Containerfile, `scswarm.yaml` | code branches |
| machine | remote, slots, mounts, devices, credentials | host file, VM |

### Track file

Branch `<tracks><name>`, for example `track/laser`. Orphan: no shared history with code. Its tree holds `SWARM.yaml` alone.

```yaml
from: main          # branch the work belongs to
at: a1b2c3d         # optional; absent = tip of `from` when `to` is created
to: feat/laser      # output branch
priority: 10        # optional; absent = 0
rows:
  - id: laser-bolt
    agent: claude   # optional; key in scswarm.yaml `agents`
    steps: [ ... ]
  - id: laser-burn
    after: [laser-bolt]
    steps: [ ... ]
```

- Track name = branch name minus the `tracks` prefix: `laser`.
- `id` contains no `/`. `after` names ids of the same track.
- Row fields and step kinds stay as in contract.md.

### `scswarm.yaml`

Code tree, read at the row's base, the `to` tip. Fields stay: `image`, `agents`, `commit`, `defaults`, `merge`. Agents and image travel together, so a track cannot name an agent its image lacks.

### Host file

```yaml
remote: git@github.com:org/mini-rts.git
tracks: track/
workers: 3
mounts: [ ... ]
devices: [ ... ]
env:
  - match: ai-*
    names: [AI_TEAM_CLAUDE_TOKEN]
  - match: net-*
    names: [NET_TEAM_CLAUDE_TOKEN]
poll: 5
```

- `remote` replaces `path`. `tracks` replaces `trunk`. Both required.
- `env`: ordered list. `match` is a glob on track names. The first match gives the names forwarded into the row's container. Values stay on the VM, so a track file cannot ask for another team's key.

### Refs

| Ref | Holds | Where |
|---|---|---|
| `refs/swarm/heads/*` | fetched remote branches | VM |
| `swarm/<track>/<id>` | row in progress | VM |
| `<to>` | track output | pushed, no force |
| `refs/swarm/land/<track>/<id>` | row's last commit | pushed with force |
| `refs/swarm/fail/<track>/<id>/track` | track tip of the failed attempt | pushed with force |
| `refs/swarm/fail/<track>/<id>/to` | `to` tip of the failed attempt | pushed with force |

Fetch refspec: `+refs/heads/*:refs/swarm/heads/*`.

### Loop

1. Fetch the remote. Each branch under `tracks` is a track. Parse each track file.
2. If `to` is absent, create it at `at`, or at the `from` tip, and push it.
3. Repair: a row whose land commit is not in `to` merges into `to` again.
4. Pick rows for free slots: highest `priority` first; tracks of equal priority take turns; file order inside a track. No preemption.
5. Worker: build `image` from the `to` tip, create `swarm/<track>/<id>` at the `to` tip, run steps, write the land ref.
6. Merge the row into `to`. If `to` moved, merge; on conflict, run `merge` steps in the row's own session.
7. Push `to` without force, then the land ref with force. If the `to` push is rejected, fetch, merge, push again.

### States

| State | Fact |
|---|---|
| waiting | a row in `after` is not pushed |
| queued | can start, no free slot |
| running | a worker holds the row lock |
| failed | fail refs stand, and the track tip and `to` tip equal them |
| pushed | land commit is an ancestor of `to` |
| merged | land commit is an ancestor of `from` |

`status` runs in any clone that fetches `+refs/swarm/*:refs/swarm/*`. Outside the VM, running reads as queued.

### Faults

| Fault | Effect |
|---|---|
| step tries spent, merge into `to` fails, image build fails | fail refs written; row retries when the track tip or the `to` tip moves |
| git or podman fault | no mark; worker exits 2, `serve` exits 2 |
| push rejected for human commits on `to` | fetch, merge, push again |

### Checks

Each failed check skips its track and names the field:

- row `id` contains `/`, or fails `git check-ref-format`
- `after` names an id outside the track
- 2 tracks share one `to`
- `at` is not an ancestor of `from`
- `from` or `to` fails `git check-ref-format`

### Removed

`trunk`, `path`, `refs/swarm/trunk`, the parent stack merge on a detached head (contract.md:140-141, architecture.md:25), the state `done` against the trunk (contract.md:149), image build in `init` (contract.md:128).

## Steps

1. `host.rs`: `remote`, `tracks`, `env` as an ordered `match` list. Drop `path`, `trunk`.
2. `git.rs`: fetch refspec for all heads; push `to` without force; push land and fail refs. Drop `TRUNK`.
3. `board.rs`: parse a track file per branch under `tracks`: `from`, `at`, `to`, `priority`, `rows`. Checks above.
4. `config.rs`: read `scswarm.yaml` at a given commit.
5. `state.rs`: 6 states; key `<track>/<id>`; fail state from 2 refs.
6. `pool.rs`: priority, then turns between tracks.
7. `work.rs`, `slot.rs`: base on the `to` tip; build the image per row; merge into `to` with `merge` steps in the row session; push.
8. `init.rs`: bare repo and remote only; no image build.
9. `status.rs`, `log.rs`, `reset.rs`: `<track>/<id>` keys.
10. `docs/spec/contract.md`, `docs/refs/architecture.md`: replace trunk text with the rules above. Delete this RFC.
11. Verify: `cargo test`, `make lint`, `make e2e`, then the Done when check.

Slice 2, after slice 1 lands: `priority` in step 6 and the `env` list in step 1. Slice 1 runs with priority 0 for all tracks and one `env` set.

## Notes

- `merged` misses squash and rebase merges: they write new commits, so the land commit is never an ancestor of `from`.
- `reset` after a push leaves the old commits on `to`. The tool never rewrites `to`. A human reverts them there.
- Repair re-merges a row dropped from `to` by force-push. To drop a row, delete it from the track file, or revert it on `to`.
- An urgent track waits for a free slot, up to one row's length.
- A git or podman fault in one slot stops `serve` for every track.
- A Containerfile with `COPY . .` rebuilds the image for every row.
- A track with several last rows is valid: `to` joins them all.
- A row that needs another track's output names that track's `to` as `from`. `after` never crosses tracks.
- A 429 usage limit fails rows of every track that shares the credential. Per-team `env` limits the blast to one team.
- Push needs a deploy key or token on the VM, never in a container.
- GitHub accepts `refs/swarm/*` from a write deploy key. Checked 2026-10-22, private repo on a personal account, ssh, git push:
  - create of `refs/swarm/land/laser/bolt`, `refs/swarm/fail/laser/burn/track` and `refs/swarm/fail/laser/burn/to`: accepted
  - fetch `+refs/swarm/*:refs/swarm/*` in a fresh clone: 3 refs
  - `GET /repos/{owner}/{repo}/branches`: lists `main` and `feat-x` alone, so the refs stay out of the web UI branch list
  - non-force update of `refs/swarm/land/laser/bolt` to a non-descendant: `! [rejected] (non-fast-forward)`, so land and fail refs need force
  - delete with `git push origin :<ref>`: accepted
- Unverified: organization repos, GitHub Enterprise, and branch rulesets that target all refs.
