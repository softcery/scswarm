# scswarm

Runs coding agents on a task board with no human at the keyboard. One binary, one workstation.

You list tasks in `SWARM.md` and commit. Workers pick every task whose dependencies are met, run it in its own git worktree and container, and leave branch `swarm/<id>`. You review and merge.

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

1. Write the host file `~/.config/scswarm/PROJECT.ron`:

   ```
   (
     path: "~/myproject",
     trunk: "main",
     workers: 2,
     mounts: ["~/.cargo/registry:/home/dev/.cargo/registry"],
     env: ["ANTHROPIC_API_KEY"],
   )
   ```

2. In the project checkout, write `scswarm.ron`, `Containerfile.swarm` and `SWARM.md`. See [Configure](#configure).

3. Commit all three on the trunk branch. The swarm reads them from there, never from your working tree.

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
2. Parses `SWARM.md` and `scswarm.ron` at that ref.
3. Derives the state of every row from git: blocked, ready, running, failed, landed, done.
4. Gives each ready row a free slot.
5. Sleeps.

A slot is a git worktree at `~/swarm/slot/<project>/<n>` plus one long-lived container `scswarm-<project>-<n>`.

A task runs as stages. A stage is one agent session with scripted turns. The last turn of a stage asks for a commit message; the host commits every change with trailers `Swarm-Task` and `Swarm-Stage`. Restart resets the worktree to the last stage commit and reruns that stage in a fresh session.

Rows stack. A task whose parent landed but is not merged branches off that parent, so a chain builds while you sleep.

Design and reasoning: [docs/refs/architecture.md](docs/refs/architecture.md).

### Where state lives

Git only. No database, no state file.

| Fact | Where |
|---|---|
| task started | branch `swarm/<id>` exists |
| stage reached | trailer `Swarm-Stage: n/total` on the branch tip |
| task landed | last stage committed |
| task done | that commit is an ancestor of the trunk |
| task failed | ref `refs/swarm/fail/<id>` holds the trunk tip it failed against |

Kill the process at any point, restart, every row rebuilds from the repo.

### Git is read-only inside the container

The gitdir mounts read-only. `git status`, `git log`, `git diff` work. `git add`, `git mv`, `git commit`, `git checkout` fail with `Read-only file system`. The host stages every change with `add --all`, so a plain `mv` still lands. Do not ask the agent for a git write in your prompts.

## Configure

Four files. Binding formats: [docs/spec/contract.md](docs/spec/contract.md).

### Host file: `~/.config/scswarm/<project>.ron`

What this machine gives the swarm. Never committed.

| Field | Meaning |
|---|---|
| `path` | project checkout the bare repo fetches from |
| `trunk` | branch every task branch starts from |
| `workers` | slot count |
| `mounts` | extra `host:container` binds, read-write. Build caches ride here |
| `env` | host environment variable names forwarded into every container. Credentials ride here |

`path` and `trunk` reload each cycle. `workers`, `mounts` and `env` bake into the containers: change one, run `init` again. `serve` exits 2 when reality and the file disagree.

### `scswarm.ron` in the project root

What the agents run. Read from the trunk ref.

```
(
  image: "Containerfile.swarm",
  default_agent: "claude",
  agents: {
    "claude": (
      exec: "claude --model opus --dangerously-skip-permissions --disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification -p --output-format json",
      resume: "--resume {session}",
    ),
  },
  stages: [
    (turns: ["/execute {id}", "Correct, clean, beautiful?", "{commit}"]),
    (turns: ["/audit {id}", "Close every gap you found.", "{commit}"]),
  ],
  merge: (turns: ["Resolve every conflict of the merge of {parent} into {id}.", "{commit}"]),
)
```

- `exec` runs in the container with the prompt appended as the last argument. It must print JSON holding `session_id` and `result`, or plain text taken as `result`.
- `--disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification` keeps the session flat: no subagent, no skill, no message to another session. A subagent burns the turn budget and writes files the stage never sees in its transcript.
- `resume` is added from the second turn on, so one stage is one session.
- Turn text takes `{id}`, `{task}`, `{parent}`. `{commit}` is the built-in turn: it asks for a message and the host commits.
- `stages[].agent` overrides the row agent for that stage. Use it to make a second model audit the first.
- `merge` runs in a conflicted worktree when a parent merge fails. Absent = the branch drops and the row starts over.

### `Containerfile.swarm` in the project root

The image every slot runs. It must hold the agent CLI, its settings, and every tool your build needs. `init` builds it from a throwaway worktree of the trunk ref, so untracked and ignored files stay out of the build context.

Start from [Containerfile.swarm](Containerfile.swarm) in this repo and change 3 things: the package list, `ARG PROJECT`, `ARG WORKERS`. `init` passes no build argument, so both values live in the file.

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

### `SWARM.md` in the project root

The board. Markdown table, 4 columns, read and never written by the tool.

```
| id | after | agent | task |
| --- | --- | --- | --- |
| parse-10-repair | | | Repair the parser. Spec in docs/spec/parser.md. |
| trace-06-decode | | | |
| parse-15-postures | parse-10-repair, trace-06-decode | codex | Postures. |
```

- `id` names the branch, the trailer, the transcript and the `after` links. It becomes a git ref, so write it lowercase ASCII with no space. Unique per board.
- `after` is a comma list of ids. Empty = ready at once.
- `agent` is a key of `agents`. Empty = `default_agent`.
- `task` is free text for the agent, substituted as `{task}`. Empty = the turns get the id alone. No `|` inside: the cell ends there.

`serve` and `status` exit 1 and name the row on a duplicate id, an unknown id in `after`, a dependency loop, an unknown agent, or a row of other than 4 cells.

## Commands

`PROJECT` is the stem of the host file. The working directory means nothing.

```
scswarm init PROJECT                bare repo, image, slot worktrees and containers
scswarm serve PROJECT               foreground loop; run under systemd
scswarm status PROJECT              one line per row: id, state, branch, stage, worker
scswarm log PROJECT ID [--stage N]  transcript, one block per turn
scswarm reset PROJECT ID            delete swarm/<id>, every dependant branch, their fail marks
```

Exit codes: 0 ok, 1 input invalid, 2 git or podman failure. Drift between the host file and the slot set exits 2; the fix is `init`.

## Run under systemd

Copy the template unit [systemd/scswarm@.service](systemd/scswarm@.service) to `~/.config/systemd/user/`, then:

```
systemctl --user enable --now scswarm@PROJECT
```

## Operate

**Row shows `failed`.** Read `scswarm log PROJECT ID` for the failing turn. A failed row waits for a new trunk tip, so it costs one attempt per push, not one per cycle. To rerun without a push, run `reset`.

**Rerun a task.** Stop `serve`, run `scswarm reset PROJECT ID`, start `serve`. A worker holding that task recreates the ref at its next commit, so stop it first.

**Change workers, mounts or env.** Stop `serve`, edit the host file, run `init`, start `serve`. `init` replaces every slot container and drops the slots at or above `workers`.

**Credential expired.** Every row it touches fails after 3 tries. Fix the credential, restart `serve`, then `reset` each marked row.

**Task done.** Merge `swarm/<id>` into the trunk. The row reads `done` once its last commit is an ancestor of the trunk.

## Limits

- Reads the board from the trunk branch only. A row on any other branch stays invisible.
- One `serve` per project. Two share the slot worktrees and fight over the same branches. Nothing locks them.
- A task stacked on a rejected parent needs `reset`.
- One configured agent means one model both writes and audits the work.
- Turn tries cap at 3, counted in tries. No token or time budget: agent flags in `exec` carry any cap.
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
