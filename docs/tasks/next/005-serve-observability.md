# 005 — serve event log

**What.** `serve` prints one line per event: cycle, assignment, stage commit, land, failure, retry.

**Why.** A 12 s `serve` run on an empty board printed 0 lines. Operator reads a stalled swarm only through `status` and `log`, and a stall inside a turn shows nowhere.

**Done when.** `journalctl --user -u sercswarm` shows, in order: fetch of `main`, each row assigned with slot and stage, each stage commit with short sha, each landed task, every failure with cause and every retry with attempt number. `status` output unchanged.

## Shape

- New `event` module: sole writer of operator lines, one function per event, fixed field order.
- `serve.rs` cycle: fetch done, rows pending, slots idle.
- `pool.rs`: assign slot, row id, first stage; collect result.
- `stage.rs`: stage commit sha, task id, stage number of total.
- `turn.rs`: turn start, exit code, retry attempt.
- Failures already carry text through `Error`; print them, do not restate.

## Rules

- No timestamp field: journald stamps every line.
- No progress bars, no spinners, no color. Lines must stay greppable.
- Print nothing per poll when nothing changes. An idle swarm stays silent.
- Agent reply text stays in the transcript, never on stdout.

## Notes

Line shape open: `key=value` pairs or plain words. Pick one, apply to every event.

## Impl

- Line shape: `key=value`, event name as bare first word, `task=` first field after it. Applied to all 7 events.
- New `src/event.rs`: `fetch`, `assign`, `turn`, `retry`, `commit`, `land`, `fail`. Each one `println!` call, no branching, no state.
- `git.rs`: added `branch_commit(branch)`, `rev-parse` of a ref's tip. Used to detect `main` movement.
- `serve.rs`: `serve()` reads `main` sha once before the loop, `cycle()` takes `&mut head`, fires `event::fetch` only when the fetched sha differs from `head`. Silences the per-poll spam the rule bars.
- `pool.rs`: `assign()` fires `event::assign(id, slot, from-stage, total)` before the job moves into the channel. `work()` fires `event::land` on `Ok(())` from `run()`, `event::fail(id, &why)` on `Err`. Replaced the old `eprintln!`.
- `stage.rs`: `commit()` swapped its own `println!` for `event::commit(id, stage, total, sha)`. Dropped local `short()`, moved into `event.rs`.
- `turn.rs`: `Turn` struct gained `id`, `stage`, `total` fields (event context only, no behavior change). `run()` fires `event::turn` once at the top, `event::retry(id, stage, total, number, attempt, exit)` before every backoff sleep, attempt counter starts at 1.
- `status.rs` untouched, confirmed by diff.

**Verified.** `cargo build`, `cargo test` (7 pass), `make lint` (fmt, clippy, comment-budget all clean). `pool.rs` comment-budget warn (15.2%) predates this change (15.9% on prior `HEAD`, checked against a throwaway copy) — not a regression.
