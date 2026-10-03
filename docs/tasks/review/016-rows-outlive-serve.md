# 016: rows outlive serve

**What.** Row attempt runs as own process, `scswarm work`. `serve` schedules only. Kernel locks mark slot and row a live process holds.

**Why.** Rows run as `serve` threads: `serve` exit kills all rows in flight.

- `pool.rs` `spawn`: one thread per slot in `serve`. Unit stops whole cgroup: `KillMode=control-group`, SIGKILL after `TimeoutStopUSec=45s` (Fedora, 2026-10-03).
- Binary upgrade = restart. mini-rts, 2026-10-03: 2 rows running, 3 ready, no idle window; upgrade costs 2 replays from base.
- Machine fault in one slot exits `serve` 2, kills rows on other slots.

**Done when.** Fedora, rows running on 2 slots: `cargo install`, `systemctl --user restart scswarm@<project>`. Both rows land, no second `assign`, one try per step in `scswarm log`. Container IDs unchanged. e2e covers same with SIGKILL on `serve`.

## Shape

Processes:

- `serve`: fetch, read inputs, converge slots, pick rows, spawn one `scswarm work` per assignment, reap children. Holds no row in memory a restart cannot rebuild.
- `scswarm work PROJECT --slot N --row ID --base SHA [--merge BRANCH]... [--env NAME]...`: one row attempt, today's `pool::attempt`. Reads config and board at `--base`, not moving trunk ref. Exit 0: land ref or fail mark written. Exit 2: git or podman failed, no mark. Hidden from `--help`.
- Scheduler picks stack and env names; worker runs them. Env values from inherited process env, as today.

Locks: `std::fs::File::lock` (flock), no file written.

| Lock | Target | Holder, span |
|---|---|---|
| slot | slot worktree dir | worker, whole life |
| row | `~/swarm/log/<project>/<id>/` | `serve` takes it before spawn, worker inherits it as stdin, holds to exit. Covers spawn-to-lock gap and gap between `checkout` dropping old branch and naming new one |
| project | bare repo dir | `serve`, whole life. Second `serve` exits 2, names first |

Kernel drops lock on holder death. Crash leaves nothing to clean.

`serve` rules:

- Slot busy: own child for it running, or slot lock held. `converge` skips busy slot: no attach, restart, rebuild.
- Row held: own child for it running, or row lock held. Held row gets no assignment.
- Probe: open dir, `try_lock`, drop. Worker locks with blocking `lock`: probe in flight costs it microseconds, no failure.
- Attach (restarts container): once per slot per `serve` life, first cycle slot is idle. Kills leftover processes of dead worker.
- Reap: child exit 0 frees slot. Non-zero: `stopped` error, `serve` exits 2. Other workers run on; restarted `serve` waits on their locks.
- Orphan worker (its `serve` dead): reparents to user manager, which reaps it. Outcome in git; exit code not needed.

Unit file: `KillMode=process`. `systemctl stop` ends `serve` only; rows in flight finish, no new row starts.

## Rules

- No new dependency. `File::lock` needs Rust 1.89; hosts: 1.95, 1.98.
- No state file. Locks on existing dirs.
- Worker argv = contract between versions: old `serve` alive across `cargo install` spawns new binary. Argv change needs release note.
- Worker stdout, stderr inherit from `serve`: events reach same journal, also after `serve` dies.
- No systemd in tool: `serve` runs in foreground too, e2e included.

## Steps

1. `config.rs`, `board.rs`: read at given rev; `from_trunk` calls it with `git::TRUNK`.
2. `lock.rs`: `hold(dir)` blocking, `held(dir)` probe. Sole caller of `File::lock`.
3. `work.rs`: `work` subcommand. Adopt row lock from stdin, take slot lock, bind slot, run today's `attempt`, exit 0 or 2.
4. `pool.rs`: children per slot, no threads. `busy`, `holds` from children and locks. `attach` once per slot. `reap` replaces `collect`. `assign` spawns binary path resolved once at `Pool::open` with work argv: Linux `current_exe` names replaced binary `<path> (deleted)` (Fedora, 2026-10-03).
5. `serve.rs`: project lock at open. `job` becomes `work` argv.
6. `main.rs`: hidden `Work` command.
7. `systemd/scswarm@.service`: `KillMode=process`.
8. Docs:
   - `contract.md`: CLI gains internal `work`. Git section gains 3 locks. Limits: drop "one `serve` per project"; add stop behavior, mixed versions.
   - `architecture.md`: Pattern: workers are processes. "One row, one slot" chokepoint: locks. New invariant "Rows outlive `serve`". New smell "row thread inside `serve`".
   - README Operate: upgrade = `cargo install` + `systemctl restart`. Hard stop: `systemctl kill --kill-whom=all`.
9. e2e: SIGKILL `serve` mid-row, start new one. Row lands, one `assign`, container ID unchanged. Second `serve` while one runs exits 2.
10. Verify: `make lint`, `cargo test`, `make e2e`, "Done when" restart on Fedora.

## Limits

- `systemctl stop` leaves rows in flight running. Hard stop: `systemctl --user kill --kill-whom=all scswarm@<project>`.
- systemd logs "left-over process" per restart while workers run. Expected under `KillMode=process`.
- Upgrade: rows started before it finish on old binary. Git refs and branch names must read same in both versions.
- After `cargo install`, path resolved at `serve` start names new file: old `serve` assigning before its restart spawns new worker.
- First rollout costs rows in flight: old binary has no locks, old unit kills cgroup. Roll out on idle project, or accept replays.
- Locks need local filesystem. `~/swarm` on NFS breaks flock.
- Worker hung forever holds slot forever. Step `timeout` bounds a try; hang outside a step, for example in git, does not end.
