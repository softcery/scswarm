# 005 — serve event log

**What.** `serve` prints one line per event: cycle, assignment, stage commit, land, failure, retry.

**Why.** A 12 s `serve` run on an empty board printed 0 lines. Operator reads a stalled swarm only through `status` and `log`, and a stall inside a turn shows nowhere.

**Done when.** `journalctl --user -u sercswarm` shows, in order: fetch of `main`, each row assigned with slot and stage, each turn, each stage commit with short sha, each landed task, every failure with cause and every retry with attempt number. `status` output unchanged.

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

## Audit

- 🔴 `serve.rs:17-25` — `serve()` reads `head` before the loop, never calls `event::fetch` for it. `cycle()` only fires `fetch` when the polled sha differs from `head`. When `main` never moves during a run, no `fetch` line prints, ever, even with rows assigning and landing throughout. Breaks the Done-when's literal first line.
- 🔴 `pool.rs:104-121` — `work()` picks `land` vs `fail` from `run()`, and `run()` is `stages(...).and(slot.release())`. `slot.release()` (`slot.rs:68-70`, `git checkout --detach`) runs regardless and its failure overrides a successful `stages()` result. A task with every stage committed (git already reports it `Landed`, `state.rs:86-90`) prints `fail` if `release` alone fails. Contract's own known limit (`architecture.md:41`, two `serve` processes share slot worktrees, nothing locks them) makes this reachable, not hypothetical.
- 🟡 `event.rs:39-41` — `fail(id, why)` is the one event with no `stage=`/`total=` field. `assign`, `turn`, `retry`, `commit` all carry it. A failed multi-stage task's log line gives no stage, forcing a fallback to `sercswarm log ID --stage N` for exactly the context this task exists to put on stdout.
- 🟡 `event.rs:3` — doc comment claims "sole writer of operator lines." False: `init.rs:19,28` and `reset.rs:21` `println!` directly, `main.rs:61` `eprintln!` directly. None route through `event`.
- 🟢 Done-when (line 7) lists fetch, assign, commit, land, failure, retry — omits `turn`, which Shape and Impl both add. Doc-only gap, no code fault.

Verified against source: `cargo build`, `cargo test` (7 pass), `make lint` (clean, `pool.rs` warn predates this change) all still hold as of the audit; no regression from what Impl recorded.

## Plan

1. `serve.rs:20` — add `event::fetch(&head);` right after `head` loads, before the loop. `cycle()`'s change-only gate stays as is; it guards repeat polls, not the one-time startup line.
2. `pool.rs` — drop `run()`. In `work()`, match on `stages()` directly for `land`/`fail`, so the event reflects real landing, not release. Call `slot.release()` after, unconditionally; on its error, panic the thread instead of routing through `event::fail`. `collect()` already turns a finished (panicked) thread into `Err(stopped(index))` (`pool.rs:52-58`) — reuse that, no new mechanism.
3. `pool.rs` + `event.rs` — change `stages()` to `Result<(), (usize, Error)>`, tagging every `?` site with the stage in flight (`job.from` for the checkout, `number` inside the loop). Extend `event::fail` to `fail(id, stage, total, why)`, same field order as `commit`/`retry`.
4. `event.rs:3` — narrow the comment to `Sole writer of \`serve\` loop operator lines.` Leave `init.rs`/`reset.rs` alone: one-shot command output is a different concept from the polling loop's event stream, out of this task's scope.
5. This file, Done-when (line 7) — add `turn` to the listed line order.
6. No new tests: `pool.rs` and `stage.rs` carry none today (threading and `Slot`/`Container` need real podman to fake), matches "in doubt, write no test." `event.rs` stays untested, same as now.
7. Verify: `cargo build`, `cargo test`, `make lint`, confirm `status.rs` still shows no diff.

## Fixed

- `serve.rs:21` — `event::fetch(&head)` fires once at startup, before the loop. Startup fetch line now prints every run, moving swarm or not.
- `serve.rs:20` — plan's literal wording read `head` from `branch_commit` (a bare `rev-parse`, no network) with no `repo.fetch()` first, so the new startup event named a fetch that never ran. Added `repo.fetch()?` right before `head` loads. Line now reports a real fetch, not a stale local ref.
- `pool.rs` — dropped `run()`. `work()` matches `stages()` directly: `land` on `Ok`, `fail` on `Err`. `slot.release()` runs after, unconditional, `.expect()` on error. A release failure panics the thread; `collect()` already turns a finished thread into `stopped` (`pool.rs:59`), no new mechanism.
- `pool.rs` — `stages()` returns `Staged<()>` (`type Staged<T> = Result<T, (usize, Error)>`, `pool.rs:15`). Checkout failure tags `job.from`, each loop-body failure tags `number`. `fail` now reports the real stalled stage, never the release step.
- `event.rs:39-41` — `fail(id, why)` → `fail(id, stage, total, why)`. Field order matches `commit`/`retry`: `task=`, `stage=N/T`, then payload.
- `event.rs:3` — doc comment narrowed to "Sole writer of `serve` loop operator lines." `init.rs`/`reset.rs` untouched: one-shot command output stays outside this task's scope.
- Done-when (line 7) — added `turn` to the listed event order.
- No new tests: matches plan's "in doubt, write no test," `pool.rs`/`stage.rs` still carry none.

**Verified.** `cargo build`, `cargo test` (7 pass), `make lint` exit 0 (fmt, clippy clean; comment-budget: `pool.rs` 16.0% prose, warn only, under 20% fail line; `paths.rs`/`slot.rs` warns pre-exist, untouched by this change). `status.rs`, `init.rs`, `reset.rs`, `main.rs` show no diff.
