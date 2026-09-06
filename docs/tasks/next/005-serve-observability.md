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
