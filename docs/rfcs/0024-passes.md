# Passes: run a track's rows again

**What.** Track file field `passes: <n>`. Once every last row of a pass pushes, the rows run again from the new `to` tip, up to `n` passes. Each pass after the first starts at ref `refs/swarm/pass/<track>/<n>`.

**Why.**
- Open-ended work, for example a game an agent designs and builds over hundreds of sessions, needs a fixed loop of roles: design, execute, audit and fix, repeat. The plan lives in the code tree; the track holds only the roles.
- A track runs each row once. A land ref inside `to` reads `pushed` for good (`src/state.rs` lines 105-113), and a worker skips the session whenever a land ref stands (`src/work.rs` lines 169-182).
- A loop in `after` skips the track (`docs/spec/contract.md` line 83, `src/board.rs` lines 317-326), so no track text can express a repeat today.
- Rows written out ahead, `iter-001` to `iter-500`, need a human to plan the count and the lanes, and a failed row in an `after` chain blocks every row after it.

**Done when.** `make e2e` runs a track with `passes: 2` and rows `a`, then `b` after `a`, each one `run` and one literal commit. `to` holds 4 row merges in the order `a`, `b`, `a`, `b`. `refs/swarm/pass/<track>/2` stands at the `to` tip after the first `b`. `status` reads both rows `pushed` and no worker spawns after.

## Design

### Track file

```yaml
from: main
to: game/tanks
passes: 1000
rows:
  - id: design
    steps: ["Read docs/. Update docs/design.md. Write docs/next.md.", commit:]
  - id: execute
    after: [design]
    steps: ["Build docs/next.md.", run: make test, commit:]
  - id: audit
    after: [execute]
    steps: ["Audit this pass. Fix every gap. Write docs/audit.md.", run: test ! -f docs/DONE, commit:]
```

- `passes`: integer, 1 or more. Absent = 1, today's behavior. Any other value skips the track, as other track checks.
- Roots: rows with no `after`. Lasts: rows no row names in `after`. From pass 2 on, roots run after every last of the pass before. This edge is implicit. `after` still refuses a loop.
- `passes` reloads with the track tip. A higher value resumes a finished track. A lower value lets the current pass finish, then stops.

### Pass ref

- `refs/swarm/pass/<track>/<n>`, n 2 or more: the `to` tip pass `n` starts at. Under `git::KEPT`, so `mirror_kept` pushes it with the land and fail refs (`src/git.rs`).
- Current pass: highest `n` that stands, else 1. Pass start `S`: its ref, else none.
- Writer: the worker of a root row that opens a pass, before its session. It creates the ref at its base with `git update-ref <ref> <base> ""`, create-only. With two roots, the first write wins; the second reads it and goes on.
- `Key::pass_ref(n)` beside `land_ref` and `fail_refs` (`src/state.rs` lines 137-145), on the track.

### State

A land commit `L` is current when `L` is not an ancestor of `S`. With no `S`, every `L` is current.

| State | Fact, on a track with `passes` |
|---|---|
| merged | `L` current, ancestor of `from` |
| pushed | `L` current, ancestor of `to` |
| queued, root | no current `L`; or every last pushed or merged, current pass below `passes` |
| queued, other | no current `L`, every `after` row pushed or merged |

- Running and failed read as today. Precedence holds: merged, pushed, running, failed, then queued or waiting.
- A root due for the next pass reads `queued` even though its own `L` is current. The check sits in `standing` (`src/state.rs` lines 191-202), which sees every row.
- `passes: 1` writes no pass ref, so every `L` stays current: the table reduces to today's rules.

### Worker

`attempt` (`src/work.rs` lines 169-182) picks one of 3 paths from the land ref `L` and the base:

1. `L` current and not an ancestor of the base: merge it into `to` with no session, as today. Covers a force-push that dropped the row.
2. `L` current, an ancestor of the base, row a root, current pass below `passes`: write `refs/swarm/pass/<track>/<n+1>` at the base, then run the session.
3. No `L`, or `L` not current: run the session. `checkout` resets the standing branch at the base (`src/slot.rs` line 99).

- `L` current, an ancestor of the base, any other case: the row reads pushed. Exit 0, no push.
- The worker reads `S` and `passes` from the bare repo and the track file at the track tip. `work` argv and stdin stay as they are.
- Event line on path 2: `pass <project>/<track> n=<n>`.

### Reset

`reset ROW` acts as today: branch, land ref and fail refs of the row and its dependants. The implicit edge from lasts to roots adds no dependant. Pass refs stay; the count goes on.

## Steps

1. `board`: parse and check `passes`. `Board::roots`, `Board::lasts`.
2. `state`: `Key::pass_ref`, current pass and `S` in `Facts`, the current-`L` test in `own`, the root rule in `standing`. Unit test: a finished pass queues the roots below `passes` and holds them at it.
3. `work`: the 3 paths in `attempt`, the create-only ref write in `git`, the `pass` event.
4. `tests/e2e.rs`: the `passes: 2` track from Done when.
5. Docs: contract track file field, state table, ref table, the `after` loop rule; architecture rule "rows repeat by pass, the pass ref marks the start", chokepoint `work::Attempt` sole writer of pass refs; `src/help/track.yaml`.

## Notes

- Early stop is project side: a `run` step such as `test ! -f docs/DONE` fails the row, and the pass halts until a tip moves.
- A failed row retries only when the track tip or the `to` tip moves (`docs/spec/contract.md` line 211). A usage limit, 429, that fails a row with nothing else in flight halts the loop until a human pushes (line 299). A run of days needs a fix for that, as its own issue or RFC.
- No token cap (line 333). Spend over a long run is bounded by `passes` times the per-step `tries` and `timeout`.
- Each pass runs its rows one stage at a time when the rows form a chain. One track uses one slot at a time; parallel work needs more rows per stage or more tracks.
- A crash after the pass ref write and before the root lands leaves a pass with no current land on any row. The root reads queued under the "no current `L`" rule and replays; path 3 runs it and writes no second ref.
- Two lasts that push at once: each worker reads the other's merge on `to` only after its own push. The root reads queued on the cycle after both land.
- Rows removed from the track: their land refs stay. A row added mid-run has no `L` and runs in the current pass once its `after` rows push.
- A track removed: its pass refs stay with its land refs, until `reset` of each row and a manual `git update-ref -d`. No command drops a pass ref.
- `status` shows no pass number. Read it with `git for-each-ref refs/swarm/pass/<track>`. A column fits with the status columns change (PR 22).
- Transcripts append across passes, so step numbers restart in `run.jsonl` each pass (`docs/spec/contract.md` line 277).
- Unverified: cost of `is_ancestor` per row per cycle on a `to` with thousands of merges.
