# 013 — row is a session

**What.** One board row becomes one agent session: its own turn list, one commit, one branch. Stage sequencing, both `Swarm-*` trailers and the markdown board go. Board, project config and host file become YAML.

**Why.**

- A row cannot pick its own workflow. `config.rs:27` holds one `stages` list and every row runs all of it; `state.rs:33` takes one `total` for the whole board.
- Agents copy host trailers into their own commit messages. alpha commit `483038d` carried `Swarm-Task` and `Swarm-Stage: 3/3` twice, `parse_trailer` joined the two values, `trailer_commit` skipped to the stage 2 commit, stage 3 ran again: 6 min 57 s and one extra commit. See [issue-trailer-duplicate.md](../../notes/issue-trailer-duplicate.md).
- A markdown cell holds one line and no `|` (`contract.md:128`). The unit of work is a prompt, and a prompt wants paragraphs.

**Done when.** A three-row chain on one board runs execute, audit, execute through `after` and lands three branches; a one-row task lands after one session; `git log` of a landed branch holds no `Swarm-` line; `scswarm status` prints one line per row with no stage number; `cargo test` and `make lint` clean.

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

Host appends the `commit` turn to every row and to `merge`, so no board row writes it and no row holds two commits.

State, two refs and no trailer:

- `refs/swarm/land/<id>` holds the row's commit, written by `pool::work` on success next to `refs/swarm/fail/<id>`.
- `blocked` and `ready` from `after`. `running` from the branch checked out in a slot worktree, as `status.rs:27` reads it today. `landed` from the land ref. `done` when that commit is an ancestor of `refs/swarm/trunk`. `failed` when the fail ref equals the trunk tip.
- Branch creation stays at `slot.rs:76`: detached merge of parents, branch named last, so a crash mid-merge leaves no branch.
- Crash mid-row leaves a branch with no land ref. The next attempt resets it to base and replays the row.

Slot affinity, no stored state: among idle slots, `serve` prefers one whose worktree HEAD is an ancestor of the row's base, else the lowest idle index. `git worktree list --porcelain` already prints that sha and `git.rs:200` already parses that output. A chained row's base holds its parent's commit, so the slot that ran the parent keeps its warm `target/`.

## Rules

- Keep no back-compat. `SWARM.md` and `scswarm.ron` stop being read.
- Host owns git. Nothing in `src/` reads a commit message.
- `{id}` and `{parent}` stay slots for `merge` turns alone. A board row types its own text.
- Empty `turns`, unknown `agent`, duplicate `id`, a loop in `after`, an `id` git refuses: each exits 1 naming the row.
- One name per concept: row, turn, session. No stage, no script, no snippet.

## Steps

1. `Cargo.toml`: `serde-saphyr` with `default-features = false`, `features = ["deserialize"]`. Drop `ron`. Update the `CLAUDE.md` dependency line, which names `ron`.
2. `config.rs`: parse YAML, fields `image`, `default_agent`, `agents`, `commit`, `merge`. Drop `Stage`, `stages`, `COMMIT_TURN`, `TASK_SLOT`.
3. `host.rs`: parse YAML, fields unchanged.
4. `board.rs`: parse YAML rows, `Row { id, after, agent, turns }`. Delete `table_lines`, `cells`, `is_separator`, `header`, `Draft`, `COLUMNS`. Keep the id ref check, duplicate check, `after` resolution, loop check.
5. `state.rs`: `land_ref`, `fail_ref`, six states with no numbers. Delete `Progress`, `TASK_TRAILER`, `STAGE_TRAILER`, `committed_stages`. `git.rs`: delete `trailer_commit`, `Trailer`, `parse_trailer`, `trailer_format`; `commit` takes no trailers.
6. `stage.rs` renamed to `script.rs`: `play` sends the row's turns, appends the `commit` turn, returns the commit sha. Delete `MERGE`, `total`, stage numbering.
7. `pool.rs`, `serve.rs`: write the land ref on success, drop `Job.from` and `FIRST_STAGE`, add the affinity rule to slot pick.
8. `status.rs`, `log.rs`, `paths.rs`, `event.rs`: drop stage numbers, transcript at `~/swarm/log/<project>/<id>/run.jsonl` and `merge.jsonl`, `log PROJECT ID` prints merge then run, `--stage N` gone.
9. This repo: `SWARM.md` to `SWARM.yaml`, `scswarm.ron` to `scswarm.yaml`, host file example in `README.md` usage and config sections.
10. `docs/spec/contract.md` and `docs/refs/architecture.md`: rewrite the board, config, git and CLI sections, plus the limits below. Delete plan 010. Rewrite plans 006 and 007.
11. Tests, minimal: `board.rs` parses the sample above and names the row on empty `turns`; `config.rs` parses the sample above and names the bad field. Nothing else.
12. Verify: `cargo build`, `cargo test`, `make lint`, then a live three-row chain on one board.

## Limits

- `status` prints one line per row, so a three-row task reads as three rows. The tool learns no id pattern, so it groups nothing.
- A row's failure costs its whole turn list. Plan 007 bounds that to one turn with a checkpoint ref.
- A turn that starts with `{` must be quoted: bare `- {commit}` parses as a flow mapping and fails. A brace later in the line is safe, so `resume: --resume {session}` needs no quotes.
- Board holds plain string turns. Plan 006 adds the `command` turn as a map, and an untagged mismatch then reads `data did not match any variant of untagged enum Turn` with the line and a caret.
- An alias bomb is no path in: the board schema is three levels deep and holds no recursive type, so a nested alias fails on type at the first level. Probe stopped at line 2 column 10 before expanding.
- A row that fails waits for a new trunk tip, unchanged.
- Chained rows on a moved trunk take the newer base, so a task's chain can hold a trunk merge.

## Notes

Crate choice, measured 2026-09-17 on the crates.io API and a probe crate:

- `serde-saphyr` 1.3.0, released 2026-09-16, 5.16M downloads in 90 days, MIT or Apache, MSRV 1.89. Errors carry line, column and a caret snippet. 13 crates added to the 38 in the tree.
- `serde_norway` 0.9.42 rejected: no release since 2024-12-21, and it rests on `unsafe-libyaml-norway`, 248 `unsafe` mentions in 13 of 29 files, against 12 in 6 of 72 for saphyr.
- `serde_yaml_bw` 2.5.8 rejected: `from_str` errors carry no line or column.
- `serde_yaml` 0.9.34 and `serde_yml` 0.0.13 rejected: both archived by their authors.

Probe passed: anchor and alias on a scalar in a sequence, block scalar with the block indentation stripped, `#[serde(default)]` for absent `after` and `agent`, `deny_unknown_fields`, `Option<String>` for `resume`, duplicate mapping key rejected, untagged string-or-map turn for plan 006.
