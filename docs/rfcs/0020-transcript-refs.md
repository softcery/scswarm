# Transcripts in clones, command output on the host

**What.** Each row end writes ref `refs/swarm/log/<track>/<id>`: the row transcripts with `stdout` and `stderr` dropped. `serve` mirrors it with the land and fail refs. `scswarm log --clone ROW` reads it from any clone.

**Why.**
- `log` reads `~/swarm/log/<project>/<track>/<id>/*.jsonl` on the host alone (`src/log.rs` lines 18-35). A team member who needs to know why a row failed needs a shell on the swarm host.
- Shell on the swarm user is access to every project on it: transcripts, bare repos, credential files (RFC 0019, Notes).
- Full transcripts must not reach git. `stdout` and `stderr` hold raw command output (`src/transcript.rs` lines 31-33). Every forwarded credential is visible to every process in the container (`SECURITY.md`, trust model), so one `env` step writes it there. A ref deleted on the remote stays in each clone that fetched it.
- The other fields of `Entry` (`src/transcript.rs` lines 17-42) add no output class git lacks:

| Field | Source | In git today |
|---|---|---|
| `prompt`, `run` | track file, `scswarm.yaml` | yes |
| `commit`, `message` | commits the host pushes to `to` | yes |
| `result` | agent reply | no; same author and class as `message` and as committed code |
| `at`, `step`, `exit`, `session` | host clock, step index, exit code, agent session id | no; no content |
| `fault` | 5 fixed texts of `call::Fault` (`src/call.rs` lines 229-240) | no; no content |
| `stdout`, `stderr` | command output | no; stays on host |

**Done when.** `make e2e` runs a row with a `run` step that prints a marker to stdout. A fresh clone fetches `refs/swarm/*`. `scswarm log --clone <track>/<id>` prints the row's prompts and replies, and its output holds no marker.

## Design

### Public entry

- `transcript::Entry::public(self) -> Entry`: same entry, `stdout` and `stderr` set to `None`. Sole place that decides what leaves the host.
- Failed agent turn: today `call::output` keeps raw `stdout` and no `result` (`src/call.rs` lines 124-129). `Turn` parses `stdout` on a failed try too and sets `result` when the reply parses, so the public entry keeps the agent's last words. `stdout` stays in the host copy.

### Ref

- Name: `refs/swarm/log/<track>/<id>`, under `git::KEPT`. `Key::log_ref` next to `land_ref` and `fail_refs` (`src/state.rs` lines 128-136).
- Target: commit with no parent. Tree holds `run.jsonl` and `merge.jsonl` when each exists, public entries alone, one per line.
- Written in the bare repo through `git hash-object -w`, `git mktree`, `git commit-tree`, then `ref_set`. One `Git` method: `Git::publish(name, files)`.
- Each write replaces the last. A replay appends to the host transcript (`docs/spec/contract.md` line 260), so the ref holds every try.

### Writer

- `work`, when a row ends: after the land ref and push, or after the fail refs. Same points that write kept refs today (`src/work.rs` lines 167-176, 208-221).
- A git or podman failure returns `Err` and writes no ref, same as no fail mark (`docs/refs/architecture.md` line 32). The replay publishes at its end.
- `serve` mirrors it with no change: `mirror_kept` pushes all of `refs/swarm/*` with force and prune (`src/git.rs` lines 85-89).

### Reader

- `scswarm log PROJECT ROW`: host copy, full, as today.
- `scswarm log --clone ROW`: clone in the working directory, `git show refs/swarm/log/<row>:run.jsonl` then `merge.jsonl`, same `render`. Same clone rule as `status --tracks`: the clone fetches `+refs/swarm/*:refs/swarm/*`.
- No ref: exit 1, error names the ref and the fetch refspec.

### Reset

`reset` deletes `log_ref` with the land and fail refs of each target row (`src/reset.rs` lines 14-21).

## Steps

1. `transcript`: `Entry::public`.
2. `call`: failed `Turn` try parses `result` from `stdout`.
3. `git`: `Git::publish`. `state`: `Key::log_ref`.
4. `work`: publish at row end, both outcomes.
5. `reset`: drop the log ref.
6. `log`: `--clone ROW` reader. CLI in `main`.
7. `tests/e2e.rs`: marker row, fresh clone, `log --clone`.
8. Docs: contract ref table and CLI, architecture rule "command output stays on the host", chokepoint `Entry::public`, README operate section.

## Notes

- `result` is agent text, unchecked. An agent that quotes a credential in its reply publishes it. The same agent can write it into a committed file or a commit message today. The rule removes command output, not agent intent.
- Access to the log ref = read access to the remote. With RFC 0019, each project has its own remote, so a client's team reads its own transcripts alone.
- The bare repo mounts read-only into each slot container (`src/podman.rs` line 73). Agents of one project can read the log refs of that project.
- Log refs of merged rows stay until `reset`, same as land refs. No pruning here.
- Host transcripts grow with no limit. Retention belongs to host operations. The row directory is also the row lock (`docs/spec/contract.md` line 212): a cleanup must skip held rows.
- A running row has no log ref until it ends. Live view stays on the host.
- Unverified: GitHub accepts a parentless commit under `refs/swarm/*` from an org repo deploy key. Checked so far: land refs on a personal repo (`docs/spec/contract.md` line 307).
- Unverified: transcript size per row. Prompts and replies only; no measured row yet.
