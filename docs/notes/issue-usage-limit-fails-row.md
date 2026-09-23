# Issue: usage limit fails row in 6 s

Seen on alpha board, 2026-09-11. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `call.rs` `Call::ask()`: prompt step `tries` default 3, `wait` 1 s doubling. `judge()` reads exit code only, not cause.
- Usage limit holds for whole account. Scheduler treats it as one row failure.
- `docs/spec/contract.md` states this retry rule as design.

## Evidence

- Transcript `~/swarm/log/alpha/parse-28-retry/2.jsonl`, stage 2 turn 2.
- 3 tries, 05:06:28 to 05:06:34, each exit 1.
- Each `stdout` holds `"is_error":true`, `"api_error_status":429`, `"result":"You've hit your weekly limit · resets 7am (UTC)"`.
- Journal: 05:06:34 `fail task=parse-28-retry stage=2/3`.
- Row stood failed until manual restart. Branch made again 10:00:42, fast-forward to old stage 1 commit `66eb125` 10:02:22.

## Effect

- Row idle 4 h 55 min 48 s, 05:06:34 to 10:02:22.
- Fail mark needs trunk move or `reset` to clear. Limit reset alone frees nothing.
- Every row on board hits same limit. Other slots fail rows same way.

## Fix

- Adapter reads rate-limit cause and reset time from agent output.
- Scheduler pauses all slots until reset time. Row keeps state, no fail mark.
- Overlap: task 006, [0-harness-adapters.md](../tasks/backlog/0-harness-adapters.md), owns adapters and the `unavailable` wait.
