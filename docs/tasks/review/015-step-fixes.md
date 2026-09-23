# 015 — step fixes

**What.** Close gaps left by 014: stray process after `serve` restart, host fault marks rows, lost try output, board number stops loop. Types carry step rules. Transcript shows commits.

**Why.**

- `slot.rs:61`: `attach` calls `start`, a no-op on a running container. Process inside survives kill of host `podman exec` client (014 probe table). `systemd/scswarm@.service` sets `Restart=always`. Agent from killed `serve` writes into next row worktree. Breaks invariant "No process outlives its try".
- `pool.rs:129-131`: every `Err` from a row writes fail mark. Failed `podman restart` (`call.rs:103`) or `git commit` marks row; row waits for new trunk tip. Broken slot marks each row it takes, one per cycle. `014-steps.md:77`, `:87` claim exit 2.
- `call.rs:102-104`: restart runs before transcript line. Restart error loses output of that try. Doc comment `call.rs:23-24` states reverse order.
- `proc.rs:97` `Instant::now() + timeout` and `call.rs:121` `wait * 2` panic near `u64::MAX` s. Worker panics, `pool.rs:57` reads dead thread, `serve` exits. Board field stops loop.

**Done when.**

- `serve` killed during `run: sleep 600`, started again: `podman top` on slot shows no `sleep 600`.
- Slot container removed during `run`: `serve` exits 2, row holds no fail mark.
- Three 014 live rows land or fail as in 014.
- `scswarm log` shows one commit entry per commit step.
- `cargo test`, `make lint` clean.

**Status.** Built 2026-09-23. `cargo test` 20 of 20, `make lint` clean, comment-budget warnings at baseline (tree 13.1%).

- Open: every live Done-when point. Build host has no podman.
- Open: `podman restart --time=0` on a stopped container, podman 5.8.4. podman-restart docs: stopped containers start. `attach` relies on it; fallback in step 2.4 not built.
- Shape past plan: `Call::prompt` and `Call::run` became one `Call::ask` over trait `Ask`, built by `Turn`, `Message`, `Shell`. Machine fault reaches `pool::collect` as a channel report, not a panic; release and record panics went the same way.
- `Entry` moved to `transcript.rs` beside `Transcript`.

## Decisions

| Question | Taken | Other |
|---|---|---|
| Machine fault | worker panics, `serve` exits 2, systemd restarts it, `attach` restarts containers. Cost: rows in flight on other slots replay | stop one slot, `serve` stays up. Dead slot gives no signal |
| Transcript size | contract limit, code unchanged | tail cap, needs config field |
| Reply with no `session_id` | try fails when agent sets `resume`. Agent with no `resume`: contract limit | contract limit alone |
| Agent lookup `script.rs:168-175`, `Invalid` unreachable after board check | keep | index, rely on board check |

## Steps

### 1. Types

1. `call.rs`: `Outcome<T> { Done(T), Spent(String) }`. One name for step result and row result.
2. `call.rs`: `Input` goes. `Call::prompt` returns `Result<Outcome<Reply>>`, `Call::run` returns `Result<Outcome<()>>`. One private try loop takes a judge fn. Agent commit = prompt with blank-reply judge. `Reply` for agents alone: drops `run` stdout built at `call.rs:150-153`, dropped at `script.rs:120`.
3. `step.rs`: `Steps(Vec<Step>)`, private field, built by `settle` alone, read as slice. `Row.steps` and `Config.merge` take it. `expect` at `script.rs:83` rests on the type.
4. `step.rs:37`: `continue_on_error` leaves `Policy`, moves to `Step::Run`. `Tuning` keeps it for parse. Prompt-kind rejection `step.rs:185-190` stays at parse.

### 2. Faults and sweep

1. Row result is `Result<Outcome<String>>`, built in `script` and `slot`, read in `pool::work`:
   - `Done(sha)`: land ref.
   - `Spent(why)`: fail mark. Covers spent tries, conflict left after merge steps, absent `merge` block.
   - `Err`: machine fault. Worker panics, writes no mark. `pool::collect` reads dead thread, `serve` exits 2.
2. `slot.rs:96-106`: `git merge` error with `MERGE_HEAD` set = conflict, `Spent`. With no `MERGE_HEAD` = git failure, `Err`. Today every merge error reads as conflict (`slot.rs:98`).
3. `slot::Resolve` returns `Result<Outcome<()>>`. `checkout` aborts merge on `Spent` and on `Err`.
4. Confirm `podman restart --time=0` starts a stopped container, podman 5.8.4. If it refuses, `attach` runs `start` then `restart`.
5. `slot.rs:61`: `attach` calls `restart`. Delete `Container::start` (`podman.rs:73-75`).
6. `call.rs`: per try: capture, judge, append transcript line, restart. Restart runs on capture error too. Doc comment `call.rs:23-24` matches.

### 3. Parse and overflow

1. `proc.rs:97`: `Instant::checked_add`. Overflow = no deadline.
2. `call.rs:121`: `saturating_mul(2)`.
3. `step.rs:81-86`: `present` on `prompt` and `run` too. `run:` with no value errors naming `run`, not "exactly one of the keys".
4. `step.rs:149`: error lists key names received. Values never.
5. `step.rs:111-114`, `:182-222`: split `Tuning` check. Field rules (`tries`, `timeout`, `continue_on_error`) run on `defaults` and steps. Pair rule `wait` ≤ `wait_max` runs on settled step alone, as `contract.md:71` states.
6. `step.rs:41-54`: drop redundant `#[serde(default)]` on `Option` fields. `is_empty` = derived `PartialEq` against `Default`.
7. `proc.rs:93-107`: on `wait` error, kill child, reap.

### 4. Transcript

1. `Transcript { path }` with `append(&Entry)` owns file writes, today `call.rs:205-221`. `Call` and `Session` use it.
2. `Entry` gains `commit` (sha) and `message`. Both commit kinds append one (`script.rs:136-140`).
3. `log.rs:61-68`: stderr gets own mark. `!` for `fault` alone. Commit gets own mark.
4. `call.rs`: agent sets `resume`, reply holds no `session_id`: try fails, fault names `session_id`.

### 5. Docs

1. `docs/refs/architecture.md`:
   - `:20` chokepoint: `call::Call` and `slot::Slot::attach` call `restart`.
   - `:45` try order: capture, judge, record, restart.
   - Row fault writes fail mark. Machine fault exits `serve` 2.
2. `docs/spec/contract.md`:
   - `:139`: row waits for new trunk tip, as `:145`, `:197` state.
   - `:159`: `log` names commit entries.
   - Limits: podman fault inside `exec` exits 125, counts as failed try. A step command that exits 125 reads the same.
   - Limits: agent with no `resume` opens new session per prompt.
   - Limits: transcript holds full stdout and stderr per try. Redirect big output to a file.
3. `README.md`: troubleshooting and limits match contract.
4. `docs/tasks/review/014-steps.md`: `:77`, `:87` exit claims match step 2.1. `:86` `podman.rs:110` becomes `podman.rs:113-124`.

### 6. Checks

1. Tests, `step.rs`: `run:` with no value names `run`. `defaults.prompt: { wait: 120 }` settles under step `wait_max: 300`.
2. `cargo test`, `make lint`.
3. Live, workstation, 1 worker: every Done-when point.

## Limits

- Step command exit 125 reads as failed try, same as podman fault. No split: `sh -c` passes the code through.
- Machine fault costs a replay of every row in flight.
