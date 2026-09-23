# 014 — steps

**What.** Row `turns` becomes `steps`: prompt, `run`, `commit`, composed in any order. Every retry, wait and time cap comes from config, not code.

**Why.**

- Bench after an agent session needs an agent row plus a manual run today. A deterministic script costs agent tokens and agent judgment.
- Agent moves a long Bash command to background, ends turn with "the script runs, we wait". Next turn or commit lands over half-done work. Claude Code moves a command to background at its 120 s Bash timeout: seen in this repo's own session on 2026-09-23.
- `turn.rs:17-20` and `serve.rs:16` hold retry count, waits and poll as constants. `turn.rs:67` waits with no deadline: a hung agent held 1 of 2 slots for 13 min 27 s on the beta board, 2026-09-12.

**Done when.** A live board row runs prompt, `run: make bench`, literal `commit`, prompt, agent `commit`, and lands one branch with two commits. `scswarm log` shows both step kinds. A `run` step past its `timeout` fails the row and leaves no process in the container. Policy defaults sit in `step.rs` alone, poll default in `host.rs` alone.

**Status.** Implemented 2026-09-23. Every done-when point met on the live run below. `cargo test` 18 of 18, `make lint` clean.

- Review fixes after build: `Input::Message` renamed `Input::AgentCommit`, one name per concept with `Step::AgentCommit`. `Reply::result` holds script stdout on `run`. `Tuning` drops unused `Clone`.
- Kept: `script.rs` reads the last commit with `expect`, guarded by `step::settle`. A `Steps` type holding the final commit apart costs every caller.
- Open: agent step past `timeout` not run live; same `Call` path as `run`, which was.
- Migration: boards on `turns` fail to parse. alpha, beta, gamma move to `steps` before the workstation binary updates.
- Transcript lines written before this task carry `turn`, not `step`: `scswarm log` prints them raw.

## Shape

```yaml
# SWARM.yaml
- id: rag-04
  steps:
    - "Swap the retriever to hybrid BM25 plus dense."
    - commit:
    - run: make bench > bench/out.txt
      tries: 2
      timeout: 1800
    - commit: "bench: hybrid retriever"
    - "Read bench/out.txt. Fix the recall drop."
    - run: make test > test/out.txt 2>&1
      continue_on_error: true
    - "Read test/out.txt. Fix every failure."
    - run: make test
    - commit:

# scswarm.yaml
defaults:
  prompt: { tries: 3, wait: 1, wait_max: 60, timeout: 3600 }
  run: { tries: 1, timeout: 7200 }
```

Step kinds:

| Step | Form | Runs |
|---|---|---|
| prompt | string, or `prompt: <text>` | one agent turn in the row session |
| run | `run: <text>` | `sh -c <text>` in slot container, worktree as cwd |
| agent commit | `commit:` with no value | config `commit` prompt in the row session, host commits with the reply |
| literal commit | `commit: <text>` | host commits with the text, no agent call |

Fields, lookup order step, then `defaults.<kind>`, then tool default:

| Field | Steps | Tool default |
|---|---|---|
| `tries` | prompt, run, agent commit | prompt 3, run 1 |
| `wait` | same | 1 s, doubles per try |
| `wait_max` | same | 60 s |
| `timeout` | same | absent = no cap |
| `continue_on_error` | run | false |

Agent commit takes `defaults.prompt`. Host file gains `poll`, default 5 s: loop pace depends on the machine, not the project.

## Rules

- Steps join through the worktree alone. `run` output reaches a later step as files. No text passes into prompts, so prompts still go as typed.
- `run` text goes to the shell as typed. No substitution: row ids allow `;`, `$`, `|`.
- Agent session spans the whole row. `run` and literal commit steps leave it open.
- Row must end with a commit step. Host appends nothing.
- Many commits per row. Land ref = last commit. Branch with no land ref is still a session cut short: replay from base.
- Commit step with no diff commits empty.
- `run` non-zero exit after its tries: fails the row, writes fail mark. With `continue_on_error`, transcript records the exit, next step runs.
- `timeout` expiry counts as one failed try.
- Literal commit takes no field. Git failure is exit 2, as today.
- `merge` in `scswarm.yaml` takes the same step list, must end with a commit step. First commit closes the conflicted merge. `{parent}` and `{id}` fill prompt steps only.
- Step with no prompt in its row still has an agent: agent commit uses the row `agent`, else `default_agent`.
- Output a later row needs must sit on a tracked path. `git.rs:131` cleans with no `-x`: ignored files miss the commit and stay in the slot worktree for the next row on that slot.

## Process sweep

- After every try of an exec'd step, timeout kill included, host runs `podman restart --time=0` on the slot container through the `podman` module.
- Same container: name, labels, mounts, `--init` stay. `env` rides on `exec` (`podman.rs:110`), so nothing is lost. Restart creates nothing, so `init` still owns the slot set.
- Covers `nohup cmd &` from agent Bash, which no agent flag blocks, and the process a timeout leaves inside.
- Restart failure: slot error, exit 2 path, as any podman failure.

Measured on the workstation, podman 5.8.4, throwaway container with slot flags, 2026-09-23:

| Probe | Result |
|---|---|
| `nohup sleep 301 &` through `podman exec` | survives the exec |
| `kill -KILL` on host `podman exec` client | `sleep 302` inside survives |
| `kill -KILL -1` through `podman exec` | kills `sleep infinity`, container `Exited (137)` |
| `podman restart --time 0` | 116 ms, strays gone, `podman-init` and `sleep infinity` back |

`kill -KILL -1` rejected: `--init` runs `sleep infinity` as child of PID 1, the signal reaches it, PID 1 exits with its child.

## Checks

`board.rs`, `config.rs`. Exit 1, error names row, step number, field:

- `steps` empty, or last step not a commit.
- blank prompt, blank `run`, blank literal commit text.
- `tries` 0, `timeout` 0, `wait` above `wait_max`.
- field set on a step kind that takes none, unknown field.
- same rules on `merge`, error names `merge` step number.

Step parse is a hand-written `Deserialize`: string = prompt, map into one strict struct with the policy fields flattened in. serde untagged reports `data did not match any variant`, which names no field. Measured on serde-saphyr 1.3.0: flatten under `deny_unknown_fields` still rejects an unknown key with its line; `commit:` with no value reads as `Some(None)`; a plain scalar through `deserialize_any` types as bool or number first, so `- yes` is not a prompt.

## Steps

1. `step.rs`, new: `Step`, `Policy`, `Tuning`, `Defaults`, `Written` with hand-written `Deserialize`, `settle`. Board and `merge` share it, so neither `board` nor `config` owns it. Tool policy defaults live here.
2. `config.rs`: `Written` settles into `Config`; `defaults` field; `merge` becomes a step list.
3. `board.rs`: `turns` becomes `steps`, settled per row.
4. `proc.rs`: `capture` with deadline: pipes drained on threads, `try_wait` poll, kill at `timeout`.
5. `turn.rs` renamed `call.rs`: `Call` runs one exec'd step for prompt, agent commit and `run` alike. `Entry` gains `run`; `error` renamed `stderr`; `fault` holds signal, timeout and blank-message reasons.
6. `podman.rs`: `restart` call.
7. `script.rs`: walk steps, hold session across them, commit per commit step. `pool.rs` writes land ref from the last commit, unchanged.
8. `host.rs`, `serve.rs`: `poll`.
9. `log.rs`: render both entry kinds.
10. `SWARM.yaml`, `scswarm.yaml` of this repo: new format.
11. `docs/spec/contract.md`: board format, `defaults`, `poll`, commits per row, tracked-path rule, limits below. `docs/refs/architecture.md`: row invariant reads one session, one branch, one land ref.
12. Tests: `board.rs` parses the contract example and names row, step and field of a bad value. `step.rs` resolves fields through all three layers, names step and field, rejects a list that ends without commit.
13. Verify: `cargo test`, `make lint`, live board on the workstation.

## Live run

Workstation, podman 5.8.4, image `localhost/scswarm/alpha:latest` (claude 2.1.263), agent `--model haiku`, 1 worker, 2026-09-23:

| Row | Steps | Result |
|---|---|---|
| `steps-sweep` | `run: exit 3` with `continue_on_error`, `run: nohup sleep 700 &`, `run` that fails if `sleep 700` lives, literal commit | step 1 `exit 3`, row moved on; 0 `sleep 700` lines at step 3; landed |
| `steps-timeout` | `run: sleep 600`, `timeout: 5`, `tries: 2`, literal commit | 2 tries, each ``killed at `timeout` ``; failed; container held `podman-init` and `sleep infinity` alone after |
| `steps-live` | prompt, `run` writes `bench/out.txt`, literal commit, prompt reads it, agent commit | branch holds `bench: record notes` then `notes: add alpha and beta`; land ref at the second; agent quoted `bench/out.txt` content |

`scswarm log` printed `>` blocks for prompts and `$` blocks for commands, one per try.

## Limits

- Failure at step N replays steps 1 to N-1. Checkpoints from commit steps belong to [0-turn-checkpoints.md](../backlog/0-turn-checkpoints.md).
- Slots share CPU. Latency numbers from a `run` step carry noise from other slots. Fix in the project: `workers: 1`, or a lock in the script.
- No `timeout` set = no cap, as today.
- Sweep kills every process in the container, so no step can leave a server running for a later step. Start and stop it inside one `run`.
- A prompt YAML reads as a bool or number must be quoted.
- Boards written with `turns` fail to parse. No back-compat: every board moves to `steps` before the binary updates.

## Notes

Agent config, project side, no tool code: Claude Code 2.1.280 reads `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` env or settings key `backgroundTasksDisabled` (binary: `function zl(){return $V().backgroundTasksDisabled||a.CLAUDE_CODE_DISABLE_BACKGROUND_TASKS}`). Set, the model is not offered `run_in_background`.

Measured on the Mac host, claude 2.1.280, `--model haiku -p`, prompt "run `sleep 200 && echo done > out.txt`, reply with out.txt", 2026-09-23:

| Switch | Exit | Time | `out.txt` | Reply |
|---|---|---|---|---|
| off | 0 | 13 s | missing, still missing 100 s later | "The command is running in the background ... I'll be notified" |
| on | 0 | 209 s | `done` | "`out.txt` contains: done" |

Switch on holds the turn past the 120 s default Bash timeout. Fix lives in `exec` as env prefix, so interactive sessions in the repo keep background tasks. Contract states the rule harness-neutral; README carries the claude form.

Overlap with [0-harness-adapters.md](../backlog/0-harness-adapters.md): its "turn is text or command" becomes a fourth step kind. Its `turn.rs` line refs and its `Turn` to `Call` rename predate this task: `call.rs` now holds `Call`, and `Turn` names one agent prompt.

