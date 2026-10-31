# Publish: a row writes the next board

**What.** Field `publish: <file>` on a commit step. The host checks that file from the row's worktree, commits it as `SWARM.yaml` on the track branch and pushes it, then commits the row. A row can then plan the rows after it.

**Why.**
- A human writes every track today. Sercos has 9 tracks on fedora, read 2026-10-31. `token-issue-ultimate-review` takes `from: design/token-issue-ultimate`, so one track's `to` feeds the next by hand.
- The human cycle is mechanical in part: issues, RFC, tasks, rows, audits, new issues. Track `issue-triage`, 2026-10-31, asks an agent to mark open issues "fix directly, design or defer".
- A row cannot reach a track today. Git is read-only in the container (`docs/spec/contract.md` line 268), and the tool never writes a track (line 7).
- The board must stay off `to`. Mini-rts `2094821f` deleted `SWARM.yaml` from the code tree, and the named violation of "Track is input only" is 3 branches that edit one file (`docs/refs/architecture.md` line 26).

**Done when.** `make e2e` runs a track with seed row `a`, one `run` step that writes `next.yaml`, and a literal commit with `publish: next.yaml`. The file holds `a` and row `b` with `after: [a]`. Checks:
- The track tip holds `SWARM.yaml` alone, with both rows.
- `to` holds the merge of `a`, then the merge of `b`, and no `next.yaml`.
- A second track whose file holds another `to` reads `failed` on its row, its track tip stays, and `to` gets no merge.

## Design

### Project side

The tool holds none of this. A project command, for example `/direct`, tells one agent to read its context paths and plan the next round: write task files, then write the next board. To end a run, the last director row publishes a board with only itself, and lands a final review such as `completed.md`.

### Track file

```yaml
from: main
to: build
rows:
  - id: direct-0002            # publisher, carried over, its `after` removed
    steps: &direct
      - "/direct"
      - "Take a step back. Are these the 3 tasks that matter most?"
      - commit:
        publish: next.yaml
  - id: task-tank-recoil
    after: [direct-0002]
    steps:
      - "/execute docs/tasks/next/task-tank-recoil.yaml. Go ahead."
      - "Correct, clean, beautiful?"
      - commit:
  - id: direct-0003
    after: [task-tank-recoil]
    steps: *direct
```

- `publish`: file name at the worktree root, on an agent commit or a literal commit. Absent = today's commit step.
- A value with `/` or equal to `..` skips the track, as other step checks. A deeper path can pass through a symlink directory to a host file.
- A name that fails to resolve to a regular file spends the step. A symlink counts as no regular file: the host reads this file, and the agent wrote it.
- `merge` steps refuse the field. The check sits in `Config` settling (`src/config.rs` line 124).
- A board holds the current round. Rows that landed leave it: "Row removed = row gone from swarm" (`docs/spec/contract.md` line 89). Past rounds stay in the track log.

### Commit step with `publish`

Runs in `Session::step` (`src/script.rs` line 95), on the host, in this order:

1. Read the file with `symlink_metadata` first. Parse with `Track::parse` (`src/board.rs` line 142).
2. Check `from` and `to` equal the running track. Settle with `Track::board` against `Config::parse` of `scswarm.yaml` in the worktree. A failure spends the step: no track moves.
3. Agent commit alone: get the message. A failure spends the step: no track moves.
4. Delete the file from the worktree.
5. Write a commit on the remote-tracking track tip with `hash-object`, `mktree` and `commit-tree`, in the bare repo. Its tree holds `SWARM.yaml` alone.
6. Push with `Git::push` (`src/git.rs` line 98), no force. `Push::Behind`: fetch, write again on the new tip, push. If that fetch brings no new commit, return `Err`, as `deliver` does (`src/work.rs` line 210). `Push::Declined`: spend the step.
7. Commit the row with `Slot::commit`.

`publish` sits on the commit step to keep steps 5 and 6 away from any step that can spend. As a separate step, a spent agent commit after a moved track leaves a fail ref that names the old tip, so the row reads queued and replays with no bound.

### State

`state`, `board::scan` and `work::deliver` stay as they are. The next `serve` cycle reads the new track tip. The publisher reads `running` through its row lock, and new rows wait on `after`.

### Track branch

`work` argv holds the track name and tip, and no track branch (`src/work.rs` lines 36-59). A commit step with `publish` reads the `tracks` prefix of its project from the host file, as `reset` does (`src/main.rs` line 181). A commit step with no `publish` reads nothing new.

`work` argv and stdin stay as they are. A track with no `publish` keeps today's behavior, and an upgrade needs no release note and no `serve` restart.

Each of these returns `Err` before any track moves: exit 2, no fail mark, the project rests, the row replays:
- A host file that fails to load.
- A project entry that is gone.
- A branch of prefix plus track name that is absent, for example after a prefix change during the row.

### Crash and failure

| Event | Result |
|---|---|
| Kill before step 6 lands | No track move. Row has no land ref, replays |
| Kill after step 6, before the land ref | Track moved. Carried row has no `after`, reads queued, replays and publishes again. New rows wait |
| Merge into `to` fails after publish | Fail ref holds the old tip, so the row reads queued. Land ref stands: `attempt` skips the session (`src/work.rs` line 169) and merges once more. A second failure holds the new tip and reads `failed` |
| Task row fails | Next director waits on `after`. Run stops until a commit on `to` or a push to the track |

## Steps

1. `step`: field `publish` on `Fields`, `Step::AgentCommit` and `Step::Commit`, with the path check.
2. `config`: refuse `publish` in `merge`.
3. `git`: commit one file on a tip, in the bare repo.
4. `script`: `Session` gains `Git` and `Track`; the 7 steps above; the host file read on a `publish` step alone; event line `publish <project>/<track>/<id>`.
5. `tests/e2e.rs`: the track from Done when.
6. Docs: contract step table, git rights table, limits. `src/help/track.txt` and `src/help/track.yaml`.
7. Architecture: replace "Track is input only" with "Tool writes a track only through `publish`", chokepoint `script::Session`. Change the "Host owns git" violation (line 29): agent text steers the loop through `publish` alone, past its checks. Change the track text trust (line 72): a published `run` step is agent text, bound by the container.

## Notes

### Overlap with PR 24

Open PR 24, `rfc/passes`, targets the same use case: a game built over hundreds of sessions. It keeps fixed rows and repeats them. The 2 RFCs answer one need, so one must be chosen before build.

| | PR 24 `passes` | This RFC `publish` |
|---|---|---|
| Tool writes a track | No | Yes, 1 path |
| Rows per round | Fixed by the track | Set by the agent |
| Recipe per task | Fixed by the track | Copied by the agent, can drift |
| Agent text steers the scheduler | No | Yes, inside the checks |
| Total spend | Bound by `passes` | No bound |
| New state | Pass refs, current-land rule, root rule, 3 worker paths | None |
| Task files on `to` | Fixed names per pass | One name per task |
| End of run | Failed `run` step such as `test ! -f docs/DONE` | All rows `pushed` |

### Limits

- "Spend is bounded" (`docs/refs/architecture.md` line 32) holds per row alone. A director that keeps publishing runs until usage ends. A `max_rows` track field, counted from land refs, can bound it. Left out until a real run needs it.
- A failed row stops the run by design. Field `ended`, wait for pushed or failed, can route around it. Left out for the same reason.
- A usage limit, 429, fails rows today (issue 6). An unattended run needs that fix first.
- Nothing alerts on a stop or an end. `status` shows `failed` or every row `pushed`.
- A publish replaces rows a human pushed during that row. Humans steer through docs on `to`.
- The agent can write wrong `after` links. A board with no publisher row, or new rows with no `after` on it, starts rows before the task files stand on `to`.
- A new row with an id whose land commit is on `to` reads `pushed` and never runs. The run looks stalled. The project command must pick new ids.
- `status` prints the current board alone. Old row ids come from the track log.
- The agent can edit code in a director row, and any task row can edit the project command. No tool check stops either.
- Step 5 drops other files on the track branch. The contract already says the root holds `SWARM.yaml`.
- A remote that protects track branches declines the push and fails the row.
- A kill after step 6 costs one repeated director session.
- A replay runs the carried row as the agent wrote it. A carried row with changed steps replays those steps.
- The agent can change `priority`. It orders tracks inside one project alone.
- The host reads the file with no size cap, so a big file costs host memory.
- Land refs and row branches of past rounds stay. Each `serve` cycle reads every kept ref (`src/state.rs` line 62). Cost at hundreds of rows is unmeasured.

### Unverified

- Nothing is built or run.
- Director planning quality is unmeasured, and it is the main risk. One plain row that writes its picks to a note tests it with no tool change.
- 9 of 9 `*check` replies in sercos `reference-monitor` report gaps. Those are agent reports, not checked against diffs.
- `Fields` combines `deny_unknown_fields` with `flatten` (`src/step.rs` lines 92-102). The new field needs a parse test against that form.
