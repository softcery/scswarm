# 010 — row is a session

**What.** One board row is one agent session: own turn list, one commit, one branch. Stages, `Swarm-*` trailers and the markdown board go. Board, project config and host file become YAML.

**Why.**

- A row cannot pick its own workflow. `config.rs:27` holds one `stages` list, every row runs all of it; `state.rs:33` takes one `total` for the whole board.
- Agents copy host trailers into their own commit messages. alpha commit `483038d` carried `Swarm-Task` and `Swarm-Stage: 3/3` twice, `parse_trailer` joined the two values, `trailer_commit` skipped to the stage 2 commit, stage 3 ran again: 6 min 57 s and one extra commit. See [issue-trailer-duplicate.md](../../notes/issue-trailer-duplicate.md).
- A markdown cell holds one line and no `|` (`contract.md:129`). A turn is a prompt, and a prompt wants paragraphs.

**Done when.** `cargo test` and `make lint` clean; `grep -rn 'stage\|Stage\|trailer\|Trailer\|ron' src/` returns nothing; every check under "Verify by hand" passes.

## Shape

Three files, one format. `serde-saphyr` 1.3.0 in, `ron` out, dependency count stays at 4.

`SWARM.yaml`, project root, read at trunk. Sequence of rows. `id` and `turns` required, `after` and `agent` default. Anchor at first use carries a repeated turn, alias points back.

```yaml
- id: parse-10-repair
  turns:
    - |
      Repair the parser.
      Spec in docs/spec/parser.md.
    - &check "Correct, clean, beautiful?"
- id: parse-10-review
  after: [parse-10-repair]
  agent: codex
  turns:
    - "Audit the parser repair. Write the plan, close nothing."
    - &gaps "Audited thoroughly, no gaps? If any, close them."
    - *check
- id: parse-10-close
  after: [parse-10-review]
  turns:
    - "Close every issue the audit names."
    - *check
```

`scswarm.yaml`, project root, read at trunk.

```yaml
image: Containerfile.swarm
default_agent: claude
agents:
  claude:
    exec: claude --model opus -p --output-format json
    resume: --resume {session}
commit: "Reply with the commit message for the work in this session. Message only."
merge:
  - "Resolve every conflict of the merge of {parent} into {id}."
```

`<project>.yaml`, host file, fields unchanged: `path`, `trunk`, `workers`, `mounts`, `env`.

Host appends the `commit` turn to every row and to `merge`. No board row writes it, no row holds two commits.

State, two refs, no trailer, no worktree scan:

- `refs/swarm/land/<id>` holds the row's commit. `pool::work` writes it on success, next to `refs/swarm/fail/<id>`.
- `blocked`, `ready` from `after`. `running`: branch exists, no land ref. `landed`: land ref set. `done`: land commit is ancestor of `refs/swarm/trunk`. `failed`: fail ref equals trunk tip, no land ref.
- Crash mid-row leaves a branch with no land ref. It reads `running`, no slot holds it, `serve` assigns it again. `slot::checkout` on an existing branch with no land ref: delete branch, rebuild from base and stack, replay the row. `slot.rs:76` keeps the branch named last, so a crash mid-merge leaves no branch.
- `reset` deletes the land ref beside branch and fail mark.

Session of one row:

- `script::play(slot, agent, turns, commit, log) -> Result<String>`: send each turn in one session, append `commit`, host commits with the reply, return the sha. Blank reply retried under `turn::ATTEMPTS`, as `stage.rs:117` today.
- Board turns go as typed. `{id}` and `{parent}` fill `merge` turns alone; `script::fill` keeps its one-pass rule.
- Merge session runs at `slot::checkout` through `resolve`, as today. Its commit is a merge commit on the detached head, no ref.
- Transcript per row: `~/swarm/log/<project>/<id>/run.jsonl` and `merge.jsonl`.

## Rules

- Keep no back-compat. `SWARM.md` and `scswarm.ron` stop being read.
- Host owns git. Nothing in `src/` reads a commit message.
- Empty `turns`, unknown `agent`, duplicate `id`, a loop in `after`, an `id` git refuses: each exits 1 naming the row by `id`, or by index when `id` is absent.
- One name per concept: row, turn, session. No stage, no script, no snippet, no task text.
- Tool holds no prompt text: `commit` and `merge` come from `scswarm.yaml`.

## Steps

1. `Cargo.toml`: `serde-saphyr = { version = "1.3", default-features = false, features = ["deserialize"] }`. Drop `ron`. Run `cargo fetch`, then `rmap --crate serde-saphyr` for the `from_str` and error API. `CLAUDE.md` line 87 names `ron`; line 4, 85, 99, 110 name `SWARM.md`, `scswarm.ron`, "commit per stage": rewrite each.
2. `config.rs`: YAML. `Config { image, default_agent, agents, commit: String, merge: Option<Vec<String>> }`. Delete `Stage`, `stages`, `COMMIT_TURN`, `TASK_SLOT`, `check_stages`, `check_stage`. Checks: `commit` non-empty; `merge` present means non-empty and each turn non-empty.
3. `host.rs`: YAML through the same parser. `paths::CONFIG_EXT` to `yaml`. Fields, checks, `split` unchanged.
4. `board.rs`: YAML. `Row { id, after: Vec<usize>, agent, turns: Vec<String> }`. Delete `Entry`, `table_lines`, `header`, `cells`, `is_separator`, `Draft`, `COLUMNS`, `bad_line`. Keep ref check, default agent, unknown agent, duplicate id, `after` resolution, loop walk. Error names the row.
5. `state.rs`: `land_ref`, `fail_ref`, `LAND_REFS`, `FAIL_REFS`. `State` six unit variants. `derive(board, git)`: rules above. Delete `Progress`, `TASK_TRAILER`, `STAGE_TRAILER`, `row_progress`, `committed_stages`.
6. `git.rs`: delete `trailer_commit`, `Trailer`, `trailer_format`, `parse_trailer`. `commit(message)` takes no trailers. `slot::commit` follows.
7. `stage.rs` to `script.rs`: `play` and `resolve` as shaped. Delete `StageRun`, `MERGE`, `COMMIT_PROMPT`, `at`, `agent` lookup by stage. Agent lookup by row alone.
8. `pool.rs`: `Job { row, config, stack, base }`, drop `from`, `Staged`. `work`: `Ok(sha)` sets land ref and clears fail ref; `Err` sets fail ref. `stages` to `run`.
9. `serve.rs`: drop `FIRST_STAGE`, `job` builds no `from`, `derive` takes no total. `pending` unchanged.
10. `slot.rs`: `checkout` rebuilds an existing branch. Add `branch_delete` call before `detach_at`.
11. `event.rs`: delete `At`. Every line names `task=<id>` alone; `turn`, `retry`, `commit`, `fail`, `merge`, `assign` lose `stage=`.
12. `paths.rs`: `transcript(project, id, name)` with `name` `run` or `merge`, delete `transcript_stage`. `log.rs`: print `merge.jsonl` then `run.jsonl` when present, header `<name> turn N`. `main.rs`: drop `--stage`.
13. `reset.rs`: delete land ref beside fail mark; test config to YAML.
14. `status.rs`: columns `id`, `state`, `branch`, `worker`. Branch cell: `git.branch_exists`. Delete `stage_cell`, `total`. Doc comment stays true: every cell from git.
15. This repo: `SWARM.md` to `SWARM.yaml`, `scswarm.ron` to `scswarm.yaml` with the turns of the three stages as three anchored rows per task. `README.md` usage, config, operate sections.
16. `docs/spec/contract.md`: rewrite `SWARM.yaml`, host file, `scswarm.yaml`, Git, CLI sections and Limits. `docs/refs/architecture.md`: invariant "Stage is atomic" becomes "Row is atomic"; Loop step 3, Turn, Known limits.
17. Tests, minimal: `board.rs` parses the sample above, names the row on empty `turns`, keeps the loop and unknown-id cases; `config.rs` parses the sample above and names the bad field; `script.rs` keeps the `fill` test. Delete the rest.
18. `cargo build`, `cargo test`, `make lint`.

## Verify by hand

Operator, after merge to `main`. Slot containers hold no podman.

1. `cargo install --path .`. Write `SWARM.yaml` with a three-row chain: execute, audit, execute through `after`. Restart `serve`.
2. Three branches land. `git log swarm/<id>` holds no `Swarm-` line, one commit over base per row.
3. A one-row task lands after one session. `scswarm log PROJECT ID` prints `run turn 1` to `run turn N` and no stage number.
4. `scswarm status` prints one line per row, columns `id state branch worker`.
5. Kill `serve` mid-row, restart: the row reads `running`, then replays from base; landed rows stay `landed`.

## Limits

- `status` prints one line per row, so a three-row task reads as three rows. Tool learns no id pattern, so it groups nothing.
- A row's failure or crash costs its whole turn list. Backlog `turn-checkpoints` bounds that to one turn.
- A turn that starts with `{` must be quoted: bare `- {commit}` parses as a flow mapping and fails. A brace later in the line is safe, so `resume: --resume {session}` needs no quotes.
- Board holds plain string turns. Backlog `harness-adapters` adds the `command` turn as a map; an untagged mismatch then reads `data did not match any variant of untagged enum Turn` with line and caret.
- Alias bomb is no path in: board schema is three levels deep, no recursive type, so a nested alias fails on type at the first level. Probe stopped at line 2 column 10 before expanding.
- A row that fails waits for a new trunk tip, unchanged.
- Chained rows on a moved trunk take the newer base, so a chain can hold a trunk merge.
- Slot pick is lowest idle index. A chained row can land on a cold `target/`. Backlog `slot-affinity`.

## Notes

Crate choice, measured 2026-09-17 on the crates.io API and a probe crate:

- `serde-saphyr` 1.3.0, released 2026-09-16, 5.16M downloads in 90 days, MIT or Apache, MSRV 1.89. Errors carry line, column and a caret snippet. 13 crates added to the 38 in the tree.
- `serde_norway` 0.9.42 rejected: no release since 2024-12-21, rests on `unsafe-libyaml-norway`, 248 `unsafe` mentions in 13 of 29 files, against 12 in 6 of 72 for saphyr.
- `serde_yaml_bw` 2.5.8 rejected: `from_str` errors carry no line or column.
- `serde_yaml` 0.9.34 and `serde_yml` 0.0.13 rejected: both archived by their authors.

Probe passed: anchor and alias on a scalar in a sequence, block scalar with block indentation stripped, `#[serde(default)]` for absent `after` and `agent`, `deny_unknown_fields`, `Option<String>` for `resume`, duplicate mapping key rejected, untagged string-or-map turn.

Backlog rows `harness-adapters`, `status-context`, `turn-checkpoints`, `codex-live-row` build on this one. Renumber and fix their cross-refs when pulled.

## Impl

Landed 2026-09-18.

- `Cargo.toml`: `serde-saphyr` 1.3, `deserialize` feature alone. `ron` out. `cargo add` locked 18 packages, against the 13 the plan note measured on a probe crate.
- `yaml.rs`: `parse<T>(file, text)`. Sole caller of `serde_saphyr::from_str`. Board, project config and host file read here, so one error shape carries file, line, column and caret.
- `config.rs`: `Config { image, default_agent, agents, commit, merge: Option<Vec<String>> }`. `Stage`, `stages`, `COMMIT_TURN`, `TASK_SLOT`, `check_stages`, `check_stage` gone. Checks: `commit` non-blank, `merge` non-empty with no blank turn.
- `board.rs`: `Row { id, after, agent, turns }` from a YAML sequence. `Written` is the serde shape, `id` and `turns` defaulted so an absent `id` still names its place: error reads `row 2` or ``row `parse-10-repair` ``. `Entry`, `table_lines`, `header`, `cells`, `is_separator`, `Draft`, `COLUMNS`, `bad_line` gone.
- `state.rs`: `land_ref`, `fail_ref`. `State` six unit variants. `derive(board, git)` reads land ref, then fail mark against the trunk tip, then branch. `Progress`, `TASK_TRAILER`, `STAGE_TRAILER`, `row_progress`, `committed_stages` gone.
- `git.rs`: `commit(message)` takes no trailer. `Trailer`, `trailer_commit`, `trailer_format`, `parse_trailer`, `borrow` gone. Nothing in `src/` reads a commit message.
- `script.rs` replaces `stage.rs`: `play` returns the commit sha, `resolve` unchanged in contract. `Session` holds slot, agent, id, log, parent. `StageRun`, `MERGE`, `COMMIT_PROMPT`, `at`, per-stage agent lookup gone. `fill` runs on merge turns alone; board turns go as typed.
- `slot.rs`: `checkout` deletes a standing branch before `detach_at`, so a branch with no land ref replays from base.
- `pool.rs`: `Job { row, config, stack, base }`. `work` writes `refs/swarm/land/<id>` at the commit and clears the mark, or writes the mark. `Staged` gone.
- `serve.rs`: `FIRST_STAGE` and `Job.from` gone; `pending` covers ready and running.
- `event.rs`: `At` gone, every line names `task=<id>`.
- `paths.rs`: `transcript(project, id, name)`, `RUN_LOG`, `MERGE_LOG`. `CONFIG_EXT` yaml. `transcript_stage` gone.
- `log.rs`: prints `merge.jsonl` then `run.jsonl` when present, header `<name> turn N`. `main.rs` drops `--stage`.
- `status.rs`: columns `id`, `state`, `branch`, `worker`. Branch cell from `git.branch_exists`.
- `host.rs`: YAML through `yaml::parse`. Stamp label text moves from `ron::to_string` to `serde_json::to_string`: the YAML crate reads only, and a label is machine text. Off plan, forced by the `deserialize` feature.
- `SWARM.yaml` `[]`, `scswarm.yaml` with `commit` and `merge`. `README.md`, `docs/spec/contract.md`, `docs/refs/architecture.md`, `CLAUDE.md`, `systemd/scswarm@.service` follow.

## Review

Three defects found on re-read, all fixed:

- `board.rs` duplicate-id error dropped the first row's place. It now reads ``field `id` must be unique on the board: row 2 holds it too``.
- `architecture.md` invariant "Tool knows no project" still said prompts come from the project file. Turns come from the board. Chokepoint line now names `board` first.
- `contract.md` held no limit for a replay appending to the same `run.jsonl`, so turn numbers restart inside one file. Added.

## Verify

- `cargo test`: 15 pass.
- `make lint`: exit 0, no failure, 15 warnings, all pre-existing prose-budget warns. Tree 12.7% prose.
- `grep -rn 'stage\|Stage\|trailer\|Trailer' src/`: empty. `ron` as a word: empty outside `Environment` in the unit file.
- Reader errors probed on three bad boards: unknown field, `- {commit}` unquoted, unclosed brace. Each names line, column and caret.
- Empty board probed: `[]`, `[]\n`, a comment then `[]`, and an empty file each read as 0 rows.

## Open

- Every item under "Verify by hand". This workstation has no podman and no host file, so no row ran.
- `state::row_own` precedence and the branch delete in `slot::checkout` carry no test: both need a git repo and a slot.
- `status` calls `branch_exists` twice per row: once in `state::derive`, once for the branch cell.
- `event::land` prints before the land ref write. A kill between the two prints `land` for a row that then replays.
- This repo's board is `[]`. The three-stage prompt text of the old `scswarm.ron` survives as the `README.md` board example alone.
