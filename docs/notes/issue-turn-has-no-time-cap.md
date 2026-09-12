# Issue: turn runs without time cap

Seen on beta board, 2026-09-12. Binary built 2026-09-09 18:57, after HEAD `9ccecf6`.

## Defect

- `turn.rs:67` `Turn::run()` calls `Command::output()`. No deadline, no kill path.
- Retry cap bounds failing exits only: `turn.rs:20` `ATTEMPTS` 3, `turn.rs:17` wait 1 s doubling to 60 s.
- Agent that never exits holds slot. No event, no fail mark, no cap.

## Evidence

- Journal `journalctl --user`, task `api-02-pipeline`, stage 2/3 turn durations: 100 s, 134 s, 152 s, 13 s.
- Stage 3/3 turn 1: `exec` start 20:02:44. Still running 20:16:12, `ps -o etime` on pid 885621 = 13:27.
- Turn sends `/execute the audit fix plan`, a plan of 11 steps. Longest turn of run, 5.3 times the stage 2 median.
- Other slot idle since 19:24, so pool has spare capacity while the turn runs.

## Effect

- Hung agent holds 1 of 2 slots until operator kills `podman exec`.
- `scswarm status` reads `running`, same word for a 13 s turn and a stuck turn.
- Spend cap of 3 tries per turn covers exit codes, not wall clock.

## Fix

- Per-agent `timeout` field in `scswarm.ron`, read by `config.rs`. Tool holds no default value per project.
- `turn.rs`: spawn, wait with deadline, kill on expiry, count expiry as failing try under the 3-try cap.
- Record expiry in transcript entry: `exit` absent, `error` names the cap and the elapsed time.
- Overlap: `007-turn-checkpoints` notes name the wall-clock cap as a separate task. Checkpoints bound the lost work, not the wait.
