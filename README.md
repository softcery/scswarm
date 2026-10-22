# scswarm

Runs coding agents on a task board with no human at the keyboard. One binary, one workstation.

You list tasks in `SWARM.yaml` and commit. Workers pick every task whose dependencies are met, run it in its own git worktree and container, and leave branch `swarm/<id>`. You review and merge.

## Requirements

- Linux or macOS with rootless podman.
- git 2.30 or newer.
- Rust toolchain to build. See [rust-toolchain.toml](rust-toolchain.toml).
- An agent CLI that takes a prompt as an argument and prints its reply. Example: `claude -p --output-format json`.

## Install

```
cargo install --path .
```

## Quick start

`PROJECT` below is any name you pick. It keys the host file, the bare repo, the image and the containers.

1. Write the host file `~/.config/scswarm/PROJECT.yaml`:

   ```yaml
   path: ~/myproject
   trunk: main
   workers: 2
   mounts:
     - ~/.cargo/registry:/home/dev/.cargo/registry
   env:
     - ANTHROPIC_API_KEY
   ```

2. In the project checkout, write `scswarm.yaml` and `SWARM.yaml`. Add `Containerfile.swarm` when `image` names a file to build. See [Configure](#configure).

3. Commit them on the trunk branch. The swarm reads them from there, never from your working tree.

4. Build the bare repo and image:

   ```
   scswarm init PROJECT
   ```

5. Export the credentials named in `env`, then run the loop:

   ```
   scswarm serve PROJECT
   ```

6. Watch from another terminal:

   ```
   scswarm status PROJECT
   scswarm log PROJECT <task-id>
   ```

7. Merge what you accept: `git merge swarm/<task-id>`.

## How it works

Job queue with a worker pool. Queue = the board. Workers = slots. State = git.

Every `poll` seconds, 5 by default, the loop:

1. Fetches the trunk branch of the checkout at `path` into ref `refs/swarm/trunk`.
2. Parses `SWARM.yaml` and `scswarm.yaml` at that ref.
3. Derives the state of every row from git: blocked, ready, running, failed, landed, done.
4. Starts one worker process, `scswarm work`, per ready row and free slot.
5. Sleeps.

A slot is a git worktree at `~/swarm/slot/<project>/<n>` plus one long-lived container `scswarm-<project>-<n>`.

One row is one session of scripted steps: agent prompts, shell `run` steps, and commit steps. The host commits at each commit step and writes ref `refs/swarm/land/<id>` at the last one. A branch with no land ref holds a session cut short: once its `after` rows land, the next cycle drops the branch and replays the row from base.

Workers outlive `serve`. A restart or an upgrade leaves rows in flight running; the new `serve` waits on their locks.

Rows stack. A task whose parent landed but is not merged branches off that parent, so a chain builds while you sleep.

Design and reasoning: [docs/refs/architecture.md](docs/refs/architecture.md).

### Where state lives

Git only. No database, no state file.

| Fact | Where |
|---|---|
| task started | branch `swarm/<id>` exists |
| task landed | ref `refs/swarm/land/<id>` holds its commit |
| task done | that commit is an ancestor of the trunk |
| task failed | ref `refs/swarm/fail/<id>` holds the trunk tip it failed against |

Kill the process at any point, restart, every row rebuilds from the repo.

### Git is read-only inside the container

The gitdir mounts read-only. `git status`, `git log`, `git diff` work. `git add`, `git mv`, `git commit`, `git checkout` fail with `Read-only file system`. The host stages every change with `add --all`, so a plain `mv` still lands. Do not ask the agent for a git write in your prompts.

## Configure

Host file, `scswarm.yaml`, `SWARM.yaml`, and a Containerfile when you build the image. Binding formats: [docs/spec/contract.md](docs/spec/contract.md).

### Host file: `~/.config/scswarm/<project>.yaml`

What this machine gives the swarm. Never committed.

| Field | Meaning |
|---|---|
| `path` | project checkout the bare repo fetches from |
| `trunk` | branch every task branch starts from |
| `workers` | slot count |
| `mounts` | extra `host:container` binds, read-write. Build caches ride here |
| `devices` | host device paths passed as `--device`, for example `/dev/dri` for a GPU |
| `env` | host environment variable names forwarded into every container. Credentials ride here |
| `poll` | seconds between loop cycles. Absent = 5 |

Every field reloads each cycle. `serve` creates missing slots, drops slots at or above `workers`, and rebuilds a slot whose container holds old `mounts`, `devices` or image. A busy slot finishes its row first. `env` applies from the next row on, with no rebuild.

### `scswarm.yaml` in the project root

What the agents run. Read from the trunk ref.

```yaml
image: Containerfile.swarm
default_agent: claude
agents:
  claude:
    exec: CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1 claude --model opus --dangerously-skip-permissions --disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification -p --output-format json
    resume: --resume {session}
commit: "Reply with the commit message for the work in this session. Message only."
defaults:
  run: { timeout: 7200 }
merge:
  - "Resolve every conflict of the merge of {parent} into {id}."
  - run: make lint
  - commit:
```

- `image` naming a path on the trunk ref builds at `init`. Any other value names an image podman resolves.
- `exec` runs in the container with the prompt appended as the last argument. It must print JSON holding `session_id` and `result`, or plain text taken as `result`. With `resume` set, a reply with no `session_id` fails the try.
- `exec` must return only when the turn's work ends. The container restarts after every try, which kills anything left running. `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` stops Claude Code from sending a long command to the background and ending the turn with "the command is running in the background". Without it, the next step runs over half-done work. Measured on claude 2.1.280: switch off, turn ends at 13 s, output file never written; switch on, turn holds 209 s, file written. The switch leaves `nohup cmd &` from Bash open; the restart after the try kills it.
- `--disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification` keeps the session flat: no subagent, no skill, no message to another session. A subagent burns the turn budget and writes files the transcript never sees.
- `resume` is added from the second agent turn on, so one row is one session. Absent = every prompt opens a new session.
- `commit` is the prompt of every agent commit step: it asks for a message and the host commits with the reply.
- `defaults` sets policy per step kind, `prompt` and `run`. See the step fields below.
- `merge` is a step list run in a conflicted worktree when a parent merge fails. Its prompts take `{parent}` and `{id}`. It ends with a commit step; the first commit closes the merge. Absent = the branch drops, the row fails and waits for a new trunk tip.

### `Containerfile.swarm` in the project root

Needed when `image` names a path on the trunk ref. The image every slot runs. It must hold the agent CLI, its settings, and every tool your build needs. `init` builds it from a throwaway worktree of the trunk ref, so untracked and ignored files stay out of the build context.

Start from [Containerfile.swarm](Containerfile.swarm) in this repo and change 3 things: the package list, `ARG PROJECT`, `ARG WORKERS`. `init` passes no build argument, so both values live in the file.

`ARG CLAUDE_CODE_VERSION` pins the agent CLI. The build cache keeps an unpinned install at the version of the first build, and the agent user cannot update a root install. To update the CLI, change this value and run `init`.

The agent runs with nobody at the keyboard, so the image must answer every prompt the CLI would ask and switch off every footer it would write:

```
ARG PROJECT=myproject
ARG WORKERS=2

RUN set -eu; \
    trust() { printf '"%s":{"hasTrustDialogAccepted":true}' "$1"; }; \
    projects=$(trust "/home/dev/swarm/${PROJECT}.git"); \
    slot=0; \
    while [ "$slot" -lt "$WORKERS" ]; do \
      projects="$projects,$(trust "/home/dev/swarm/slot/${PROJECT}/${slot}")"; \
      slot=$((slot + 1)); \
    done; \
    printf '{"hasCompletedOnboarding":true,"bypassPermissionsModeAccepted":true,"projects":{%s}}' \
      "$projects" > /home/dev/.claude.json

RUN mkdir -p /home/dev/.claude \
 && printf '%s' '{"attribution":{"commit":"","pr":"","sessionUrl":true}}' \
      > /home/dev/.claude/settings.json
```

- `/home/dev/.claude.json` clears the onboarding screen, the permission-mode notice and the trust dialog. It lists the bare repo and one path per slot, since the dialog fires per directory. A `WORKERS` bump with no rebuild leaves the new slot off that list, so its turns run untrusted.
- `/home/dev/.claude/settings.json` empties `attribution.commit` and `attribution.pr` and sets `attribution.sessionUrl`. The host writes every commit, so a `Co-authored-by` trailer from the agent lands as message text, not as a trailer.
- Run the CLI as a non-root user with `--create-home`. The settings files sit under that home.

### `SWARM.yaml` in the project root

The board. YAML sequence, read and never written by the tool. Empty board: `[]`.

```yaml
- id: rag-04-retriever
  steps:
    - |
      Swap the retriever to hybrid BM25 plus dense.
      Spec in docs/spec/retrieval.md.
    - &check "Correct, clean, beautiful?"
    - commit:
    - run: make bench > bench/out.txt
      timeout: 1800
    - commit: "bench: hybrid retriever"
- id: rag-04-review
  after: [rag-04-retriever]
  agent: codex
  steps:
    - "Read bench/out.txt. Audit the retriever swap against it. Close every gap."
    - *check
    - run: make test > test/out.txt 2>&1
      continue_on_error: true
    - "Read test/out.txt. Fix every failure."
    - run: make test
    - commit:
```

- `id` names the branch, the land ref, the transcript and the `after` links. It becomes a git ref, so write it lowercase ASCII with no space. Unique per board.
- `after` is a list of ids. Absent = ready at once.
- `agent` is a key of `agents`. Absent = `default_agent`. One row per model is how a second model audits the first.
- `steps` run in order in one session. The last step must be a commit.
- An anchor (`&check`) carries a repeated step, an alias (`*check`) points back at it.
- A prompt that starts with `{`, or one YAML reads as a bool or number (`yes`, `42`), must be quoted.

| Step | Form | Does |
|---|---|---|
| prompt | a string, or `prompt: <text>` | one agent turn, text as typed |
| run | `run: <text>` | `sh -c <text>` in the slot container, no agent |
| agent commit | `commit:` with no value | agent writes the message, host commits |
| literal commit | `commit: <text>` | host commits with that text |

- Steps share the worktree and nothing else. A `run` step writes a file, a later prompt reads it. Output a later row needs must sit on a tracked path: ignored files miss the commit.
- The agent session spans the row. `run` and literal commit steps leave it open.
- Prompt, run and agent commit steps take these fields. Each field is looked up on the step, then in `defaults`, then here.

| Field | Meaning | Default |
|---|---|---|
| `tries` | tries before the step fails | prompt and agent commit 3, run 1 |
| `wait` | seconds after a failed try, doubling | 1 |
| `wait_max` | cap of `wait` | 60 |
| `timeout` | seconds before a try is killed | none |
| `continue_on_error` | `run` alone: move on after spent tries | false |

`status` exits 1 and names the row on a duplicate id, an unknown id in `after`, a dependency loop, an unknown agent, or an id git refuses as a ref. A bad step names the row, the step number and the field. `serve` reports the same text once as `skip`, starts no row, and lets rows in flight finish until a fixed board reaches the trunk.

## Commands

`PROJECT` is the stem of the host file. The working directory means nothing.

```
scswarm init PROJECT       bare repo and image; safe while `serve` runs
scswarm serve PROJECT      foreground loop; run under systemd
scswarm status PROJECT     one line per row: id, state, branch, worker
scswarm log PROJECT ID     transcripts, one block per try or commit: merge session, then row session
scswarm reset PROJECT ID   delete swarm/<id>, every dependant branch, their land refs and marks
```

Exit codes: 0 ok, 1 input invalid, 2 git or podman failure. A git or podman failure inside a slot also stops `serve` with exit 2 and marks no row.

## Run under systemd

Copy the template unit [systemd/scswarm@.service](systemd/scswarm@.service) to `~/.config/systemd/user/`, then:

```
systemctl --user enable --now scswarm@PROJECT
```

## Operate

**Row shows `failed`.** Read `scswarm log PROJECT ID` for the failing step. A failed row waits for a new trunk tip, so it costs one attempt per push, not one per cycle. To rerun without a push, run `reset`.

**`serve` exits with `slot N stopped`.** Git or podman failed inside that slot. No row got a fail mark. Read the cause in the journal and fix the machine. Workers on other slots run on. systemd starts `serve` again, which restarts each slot container the first cycle it finds the slot idle, and the stopped row replays.

**Rerun a task.** Wait until `status` shows no worker for it, then run `scswarm reset PROJECT ID`. A worker running that task recreates the refs at its next commit. Stopping `serve` stops no worker.

**Upgrade.** Run `cargo install --path .`, then `systemctl --user restart scswarm@PROJECT`. Rows in flight finish on the old binary. New rows start on the new one.

**Stop.** `systemctl --user stop scswarm@PROJECT` ends `serve` alone: rows in flight finish, no new row starts. To end every worker too, run `systemctl --user kill --kill-whom=all scswarm@PROJECT`. Their rows replay from base on the next start.

**Change workers, mounts, devices or env.** If `workers` grows past `ARG WORKERS` of the Containerfile, raise that value, commit it on the trunk, and run `init` first. Then edit the host file. `serve` applies it on the next cycle, slot by slot as each goes idle.

**Change the image.** Commit the Containerfile on the trunk, run `init`. `serve` rebuilds each slot on the new image ID as it goes idle. An absent image skips every cycle with `skip cause=image ... is absent`.

**Credential expired.** Every row it touches fails after 3 tries. Fix the credential, restart `serve`, then `reset` each marked row.

**Task done.** Merge `swarm/<id>` into the trunk. The row reads `done` once its last commit is an ancestor of the trunk.

## Limits

- Reads the board from the trunk branch only. A row on any other branch stays invisible.
- A row's failure or crash costs its whole step list. The replay starts from base.
- `status` prints one line per row, so a task written as three rows reads as three lines.
- A task stacked on a rejected parent needs `reset`.
- One configured agent means one model both writes and audits the work.
- A step with no `timeout` runs without limit. No token budget: agent flags in `exec` carry any cap.
- The container restarts after every try and the first time a `serve` finds its slot idle. A server a step needs starts and stops inside one `run`.
- A podman fault inside `exec` exits 125 and counts as a failed try. A step command that exits 125 reads the same.
- The transcript keeps full stdout and stderr of every try. Redirect big output to a file.
- Slots share the CPU, so timings from a `run` step carry noise from other slots. Set `workers: 1`, or lock inside the script.
- The commit message is the agent reply, unchecked. Agents that wrap it in a code fence produce a commit subject of "```".
- A trunk change leaves running rows on their old base.
- Mounts use the host path inside the container, so a podman machine must share that path.
- A conflicting stack costs one merge session per trunk tip, then the row waits.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.
- `systemctl stop` leaves rows in flight running.
- Locks sit on `~/swarm`, which must be a local filesystem. NFS breaks them.
- A worker hung outside a step holds its slot until killed.

## Build

```
cargo build --release
cargo test
make lint
```

## License

MIT. See [LICENSE](LICENSE).
