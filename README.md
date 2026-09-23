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

4. Build the slot set:

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

Every 5 seconds the loop:

1. Fetches the trunk branch of the checkout at `path` into ref `refs/swarm/trunk`.
2. Parses `SWARM.yaml` and `scswarm.yaml` at that ref.
3. Derives the state of every row from git: blocked, ready, running, failed, landed, done.
4. Gives each ready row a free slot.
5. Sleeps.

A slot is a git worktree at `~/swarm/slot/<project>/<n>` plus one long-lived container `scswarm-<project>-<n>`.

One row is one agent session with scripted turns. The host appends the `commit` turn, asks for a message, commits every change and writes ref `refs/swarm/land/<id>` at that commit. A branch with no land ref holds a session cut short: the next cycle drops the branch and replays the row from base.

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
| `env` | host environment variable names forwarded into every container. Credentials ride here |

`path` and `trunk` reload each cycle. `workers`, `mounts` and `env` bake into the containers: change one, run `init` again. `serve` exits 2 when reality and the file disagree.

### `scswarm.yaml` in the project root

What the agents run. Read from the trunk ref.

```yaml
image: Containerfile.swarm
default_agent: claude
agents:
  claude:
    exec: claude --model opus --dangerously-skip-permissions --disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification -p --output-format json
    resume: --resume {session}
commit: "Reply with the commit message for the work in this session. Message only."
merge:
  - "Resolve every conflict of the merge of {parent} into {id}."
```

- `image` naming a path on the trunk ref builds at `init`. Any other value names an image podman resolves.
- `exec` runs in the container with the prompt appended as the last argument. It must print JSON holding `session_id` and `result`, or plain text taken as `result`.
- `--disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification` keeps the session flat: no subagent, no skill, no message to another session. A subagent burns the turn budget and writes files the transcript never sees.
- `resume` is added from the second turn on, so one row is one session.
- `commit` closes every session: it asks for a message and the host commits with the reply.
- `merge` runs in a conflicted worktree when a parent merge fails. It takes `{parent}` and `{id}`. Absent = the branch drops and the row starts over.

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

- `id` names the branch, the land ref, the transcript and the `after` links. It becomes a git ref, so write it lowercase ASCII with no space. Unique per board.
- `after` is a list of ids. Absent = ready at once.
- `agent` is a key of `agents`. Absent = `default_agent`. One row per model is how a second model audits the first.
- `turns` are the prompts of the session, sent as typed. The host appends the `commit` turn.
- An anchor (`&check`) carries a repeated turn, an alias (`*check`) points back at it.
- A turn that starts with `{` must be quoted. YAML reads a bare `- {x}` as a mapping.

`serve` and `status` exit 1 and name the row on a duplicate id, an unknown id in `after`, a dependency loop, an unknown agent, an empty turn list, or an id git refuses as a ref.

## Commands

`PROJECT` is the stem of the host file. The working directory means nothing.

```
scswarm init PROJECT       bare repo, image, slot worktrees and containers
scswarm serve PROJECT      foreground loop; run under systemd
scswarm status PROJECT     one line per row: id, state, branch, worker
scswarm log PROJECT ID     transcripts, one block per turn: merge session, then row session
scswarm reset PROJECT ID   delete swarm/<id>, every dependant branch, their land refs and marks
```

Exit codes: 0 ok, 1 input invalid, 2 git or podman failure. Drift between the host file and the slot set exits 2; the fix is `init`.

## Run under systemd

Copy the template unit [systemd/scswarm@.service](systemd/scswarm@.service) to `~/.config/systemd/user/`, then:

```
systemctl --user enable --now scswarm@PROJECT
```

## Operate

**Row shows `failed`.** Read `scswarm log PROJECT ID` for the failing turn. A failed row waits for a new trunk tip, so it costs one attempt per push, not one per cycle. To rerun without a push, run `reset`.

**Rerun a task.** Stop `serve`, run `scswarm reset PROJECT ID`, start `serve`. A worker holding that task recreates the refs at its next commit, so stop it first.

**Change workers, mounts or env.** Stop `serve`, edit the host file, run `init`, start `serve`. `init` replaces every slot container and drops the slots at or above `workers`.

**Credential expired.** Every row it touches fails after 3 tries. Fix the credential, restart `serve`, then `reset` each marked row.

**Task done.** Merge `swarm/<id>` into the trunk. The row reads `done` once its last commit is an ancestor of the trunk.

## Limits

- Reads the board from the trunk branch only. A row on any other branch stays invisible.
- A row's failure or crash costs its whole turn list. The replay starts from base.
- `status` prints one line per row, so a task written as three rows reads as three lines.
- One `serve` per project. Two share the slot worktrees and fight over the same branches. Nothing locks them.
- A task stacked on a rejected parent needs `reset`.
- One configured agent means one model both writes and audits the work.
- Turn tries cap at 3, counted in tries. A failed try waits 1 s, doubling to 60 s. No token or time budget: agent flags in `exec` carry any cap.
- The commit message is the agent reply, unchecked. Agents that wrap it in a code fence produce a commit subject of "```".
- A trunk change leaves running rows on their old base.
- Mounts use the host path inside the container, so a podman machine must share that path.
- A conflicting stack costs one merge session per trunk tip, then the row waits.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.

## Build

```
cargo build --release
cargo test
make lint
```
