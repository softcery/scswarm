# scswarm

Runs coding agents on a task board with no human at the keyboard. One binary, one workstation.

Each team pushes a track: an orphan branch whose `SWARM.yaml` names a `from` branch, a `to` branch and the rows to run. Workers pick every row whose dependencies are pushed, run it in its own git worktree and container, merge it into `to` and push `to` to your remote. You review `to` and merge it.

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

`PROJECT` below is any name you pick. It keys the host file, the bare repo and the containers.

1. Write the host file `~/.config/scswarm/PROJECT.yaml`:

   ```yaml
   remote: git@github.com:org/myproject.git
   tracks: track/
   workers: 2
   mounts:
     - ~/.cargo/registry:/home/dev/.cargo/registry
   env:
     - match: "*"
       names: [ANTHROPIC_API_KEY]
   ```

2. On `main`, commit `scswarm.yaml`, plus `Containerfile.swarm` when `image` names a file to build. Push. See [Configure](#configure).

3. Push a track branch holding `SWARM.yaml` alone:

   ```
   git switch --orphan track/laser
   git add SWARM.yaml && git commit -m "track: laser"
   git push origin track/laser
   ```

4. Build the bare repo:

   ```
   scswarm init PROJECT
   ```

5. Write one file per credential named in `env`, then run the loop:

   ```
   mkdir -m 700 -p ~/.config/scswarm/credentials
   printf '%s' "$KEY" > ~/.config/scswarm/credentials/ANTHROPIC_API_KEY
   scswarm serve PROJECT
   ```

6. Watch from another terminal, or from any clone. A clone first runs `git config --add remote.origin.fetch '+refs/swarm/*:refs/swarm/*'` and `git fetch`:

   ```
   scswarm status PROJECT
   scswarm status --tracks track/
   scswarm log PROJECT laser/<id>
   ```

7. Review `to`, for example `feat/laser`, and merge it into `from`.

## How it works

Job queue with a worker pool. Queue = the track branches. Workers = slots. State = git.

Every `poll` seconds, 5 by default, the loop:

1. Fetches every branch of `remote`. Pushes land and fail refs when they changed.
2. Reads each track branch under `tracks`. Makes and pushes each absent `to`.
3. Parses `scswarm.yaml` at each `to` tip, derives the state of every row: waiting, queued, running, failed, pushed, merged.
4. Starts one worker process, `scswarm work`, per queued row and free slot: highest `priority` first, then the track with fewest rows running.
5. Sleeps.

A slot is a git worktree at `~/swarm/slot/<project>/<n>` plus one long-lived container `scswarm-<project>-<n>`.

One row is one session of scripted steps: agent prompts, shell `run` steps, and commit steps. The worker builds the image at the `to` tip, branches `swarm/<track>/<id>` from that tip, and commits at each commit step. At the last one it writes ref `refs/swarm/land/<track>/<id>`, merges the row into the `to` tip with one merge commit and pushes `to` with no force. A branch with no land ref holds a session cut short: the next cycle replays the row from the `to` tip.

Workers outlive `serve`. A restart or an upgrade leaves rows in flight running; the new `serve` waits on their locks.

Rows chain. A row starts once every row in its `after` is pushed, from a `to` tip that holds them.

Design and reasoning: [docs/refs/architecture.md](docs/refs/architecture.md).

### Where state lives

Git only. No database, no state file. `serve` pushes land and fail refs to the remote each cycle they change, so any clone reads them.

| Fact | Where |
|---|---|
| row started | branch `swarm/<track>/<id>` exists in the bare repo |
| row finished | ref `refs/swarm/land/<track>/<id>` holds its last commit |
| row pushed | that commit is an ancestor of `to` |
| row merged | that commit is an ancestor of `from` |
| row failed | refs `refs/swarm/fail/<track>/<id>/track` and `/to` hold the track tip and `to` tip it failed against |

Kill the process at any point, restart, every row rebuilds from the repo.

### Git is read-only inside the container

The gitdir mounts read-only. `git status`, `git log`, `git diff` work. `git add`, `git mv`, `git commit`, `git checkout` fail with `Read-only file system`. The host stages every change with `add --all`, so a plain `mv` still lands. Do not ask the agent for a git write in your prompts.

## Configure

Host file, `scswarm.yaml`, track files, and a Containerfile when you build the image. Binding formats: [docs/spec/contract.md](docs/spec/contract.md).

### Host file: `~/.config/scswarm/<project>.yaml`

What this machine gives the swarm. Never committed.

| Field | Meaning |
|---|---|
| `remote` | git URL or path the bare repo fetches from and pushes to |
| `tracks` | branch name prefix of every track branch |
| `workers` | slot count |
| `mounts` | extra `host:container` binds, read-write. Build caches ride here |
| `devices` | host device paths passed as `--device`, for example `/dev/dri` for a GPU |
| `env` | ordered rules: the first whose `match` glob takes the track name lists the credentials forwarded into its containers, as environment variables |
| `poll` | seconds between loop cycles. Absent = 5 |

Every field reloads each cycle. `serve` drops slots at or above `workers`. The worker of each row makes its slot's container, and remakes it when the row's image, `mounts` or `devices` differ. A busy slot finishes its row first. `env` applies from the next row on, with no rebuild.

### Credentials: `~/.config/scswarm/credentials/<NAME>`

One file per name in `env`, contents = value. The directory must be mode `0700`. One trailing newline strips, so `echo` works. The worker reads each file at row start, so a new value reaches the next row with no restart. A name with no file reports `stale` and keeps the last good host file. Only these files carry credentials: the `serve` environment carries none.

The machine pushes `to` and `refs/swarm/*`. Give it a deploy key or token with write access, outside every container.

### `scswarm.yaml` in the code

What the agents run. Read at the `to` tip of each track.

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
  - "Resolve every conflict of the merge of row {id} into {parent}."
  - run: make lint
  - commit:
```

- `image` naming a path at the `to` tip builds for every row, cached by podman. Builds of one project run one at a time, so slots on one Containerfile share one image ID. Any other value names an image podman resolves.
- `exec` runs in the container with the prompt appended as the last argument. It must print JSON holding `session_id` and `result`, or plain text taken as `result`. With `resume` set, a reply with no `session_id` fails the try.
- `exec` must return only when the turn's work ends. The container restarts after every try, which kills anything left running. `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` stops Claude Code from sending a long command to the background and ending the turn with "the command is running in the background". Without it, the next step runs over half-done work. Measured on claude 2.1.280: switch off, turn ends at 13 s, output file never written; switch on, turn holds 209 s, file written. The switch leaves `nohup cmd &` from Bash open; the restart after the try kills it.
- `--disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification` keeps the session flat: no subagent, no skill, no message to another session. A subagent burns the turn budget and writes files the transcript never sees.
- `resume` is added from the second agent turn on, so one row is one session. Absent = every prompt opens a new session.
- `commit` is the prompt of every agent commit step: it asks for a message and the host commits with the reply.
- `defaults` sets policy per step kind, `prompt` and `run`. See the step fields below.
- `merge` is a step list run in the row session when the merge of the row into `to` conflicts. Its prompts take `{parent}`, the `to` branch, and `{id}`. It ends with a commit step; the first commit closes the merge. Absent = the row fails and waits for a new track tip or `to` tip.

### `Containerfile.swarm` in the project root

Needed when `image` names a path at the `to` tip. The image a slot runs for each row. It must hold the agent CLI, its settings, and every tool your build needs. The worker builds it from a throwaway worktree of the `to` tip, so untracked and ignored files stay out of the build context.

Start from [Containerfile.swarm](Containerfile.swarm) in this repo and change 3 things: the package list, `ARG PROJECT`, `ARG WORKERS`. The worker passes no build argument, so both values live in the file.

`ARG CLAUDE_CODE_VERSION` pins the agent CLI. The build cache keeps an unpinned install at the version of the first build, and the agent user cannot update a root install. To update the CLI, change this value and push it on each `to`.

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

### `SWARM.yaml` on a track branch

The track file. Root of an orphan branch named `<tracks><name>`, for example `track/laser`. Read and never written by the tool.

```yaml
from: main
to: feat/laser
priority: 10
rows:
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

- `from` is the branch the work belongs to. `at`, a commit sha on `from`, sets where `to` starts. Absent = the `from` tip. Both count only while `to` is absent.
- `to` is the output branch, other than `from`. Rows start at its tip and merge back into it. One track per `to`. You can push to `to` too: rows merge your commits in. New `from` commits never reach `to` on their own: merge `from` into `to` yourself.
- `priority` orders tracks when slots run short. Higher first. Absent = 0.
- `id` names the branch, the land ref, the transcript and the `after` links, as `<track>/<id>`. It becomes a git ref, so write it lowercase ASCII with no space and no `/`. Unique per track.
- `after` is a list of ids of the same track. Absent = queued at once. A row that needs another track's output names that track's `to` as `from`.
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

A track that fails a check skips; the others run. `status` prints the rest, then exits 1 and names the track, the row and the field: a duplicate id, an unknown id in `after`, a dependency loop, an unknown agent, an id git refuses as a ref, `to` equal to `from`, two tracks on one `to`, and while `to` is absent: an absent `from`, an `at` not on `from`, or a remote that refuses to make `to`. A row whose `id` fails its check is named by its place, for example `row 2`. A track branch whose name fails its check is named by its fault alone. A bad step names the row, the step number and the field. `serve` reports the same text once as `skip`, starts no row of that track, and lets rows in flight finish until a fixed track file arrives.

## Commands

`PROJECT` is the stem of the host file. `ROW` is `<track>/<id>`. The working directory means nothing, except to `status --tracks`.

```
scswarm init PROJECT            bare repo, origin, first fetch; a new bare repo also fetches the
                                  remote's land and fail refs; safe while `serve` runs
scswarm serve PROJECT           foreground loop; run under systemd or launchd
scswarm status PROJECT          one line per row of every track: row, state, branch, worker
scswarm status --tracks PREFIX  same, from the clone in the working directory
scswarm log PROJECT ROW         transcripts, one block per try or commit: row session, then merge
scswarm reset PROJECT ROW       delete swarm/<track>/<id>, every dependant branch, their land and
                                  fail refs, here, then on the remote
```

Exit codes: 0 ok, 1 input invalid, 2 git or podman failure. A git or podman failure inside a slot also stops `serve` with exit 2 and marks no row.

## Run under systemd

Copy the template unit [systemd/scswarm@.service](systemd/scswarm@.service) to `~/.config/systemd/user/`, then:

```
systemctl --user enable --now scswarm@PROJECT
```

## Run under launchd

macOS runs containers in a podman machine VM. Start it with `podman machine init --now` once, and `podman machine start` after each login.

1. Copy the template agent [launchd/com.softcery.scswarm.plist](launchd/com.softcery.scswarm.plist) to `~/Library/LaunchAgents/com.softcery.scswarm.PROJECT.plist`.
2. Fill in the project and your user name:

   ```
   F=~/Library/LaunchAgents/com.softcery.scswarm.PROJECT.plist
   plutil -replace Label -string com.softcery.scswarm.PROJECT "$F"
   plutil -replace ProgramArguments -json "[\"$HOME/.cargo/bin/scswarm\",\"serve\",\"PROJECT\"]" "$F"
   plutil -replace StandardOutPath -string "$HOME/Library/Logs/scswarm-PROJECT.log" "$F"
   plutil -replace StandardErrorPath -string "$HOME/Library/Logs/scswarm-PROJECT.log" "$F"
   plutil -lint "$F"
   ```

3. If podman sits outside `/opt/homebrew/bin` and `/usr/local/bin`, set `EnvironmentVariables.PATH` to hold it. launchd starts agents with `PATH=/usr/bin:/bin:/usr/sbin:/sbin`.
4. Load the agent:

   ```
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.softcery.scswarm.PROJECT.plist
   ```

## Operate

**Row shows `failed`.** Read `scswarm log PROJECT ROW` for the failing step. A failed row waits for a new track tip or `to` tip, so it costs one attempt per push, not one per cycle. To rerun without a push, run `reset`.

**`serve` exits with `slot N stopped`.** Git or podman failed inside that slot, or the remote was unreachable. No row got a fail mark. A push the remote turns down, for example branch protection on `to`, fails that row instead. Read the cause in the journal, or in `~/Library/Logs/scswarm-PROJECT.log` on macOS, and fix the machine. Workers on other slots run on. systemd or launchd starts `serve` again, the next worker on each slot restarts its container, and the stopped row replays.

**Rerun a row.** Wait until `status` shows no worker for it, then run `scswarm reset PROJECT ROW`. Its old commits stay on `to`: revert them there if you want them gone. A worker running that row recreates the refs at its next commit. Stopping `serve` stops no worker.

**Upgrade.** Run `cargo install --path .`, then `systemctl --user restart scswarm@PROJECT`, or `launchctl kickstart -k gui/$(id -u)/com.softcery.scswarm.PROJECT` on macOS. Rows in flight finish on the old binary. New rows start on the new one.

`serve` makes the remote's `refs/swarm/*` match the bare repo's, so its first cycle deletes every remote land or fail ref the bare repo lacks. Before the first `serve` against a remote other hosts or clones wrote to, compare the two:

```
git ls-remote origin 'refs/swarm/*'
git -C ~/swarm/PROJECT.git for-each-ref refs/swarm/
```

**Stop.** `systemctl --user stop scswarm@PROJECT` ends `serve` alone: rows in flight finish, no new row starts. To end every worker too, run `systemctl --user kill --kill-whom=all scswarm@PROJECT`. On macOS, `launchctl bootout gui/$(id -u)/com.softcery.scswarm.PROJECT` ends `serve` alone; then `pkill -f 'scswarm work PROJECT '` ends the workers. Their rows replay from the `to` tip on the next start.

**Change workers, mounts, devices or env.** If `workers` grows past `ARG WORKERS` of the Containerfile, raise that value and push it on each `to` first. Then edit the host file. `serve` applies it on the next cycle, slot by slot as each goes idle.

**Change the image.** Push the Containerfile on `to`. Each row of that track builds it and remakes its slot's container.

**Credential expired.** Every row it touches fails after 3 tries. Write the new value to its file, then `reset` each marked row.

**Track done.** Merge `to` into `from`. Each row reads `merged` once its last commit is an ancestor of `from`.

## Limits

- A row's failure or crash costs its whole step list. The replay starts from the `to` tip.
- `status` prints one line per row, so work written as three rows reads as three lines.
- `merged` misses squash and rebase merges: they write new commits.
- `reset` after a push leaves the old commits on `to`. The tool never rewrites `to`. Revert the row's merge commit with `git revert -m 1`.
- Land and fail refs reach the remote on the next `serve` cycle. While `serve` is down, a clone reads a row a live worker pushed as `queued`.
- A failed row retries on every push to its `to`, each pushed sibling row included.
- A push of `to` refused as behind merges the row again onto the new tip. A conflict runs the `merge` steps again.
- Builds of one project wait on each other, so a slot can wait on another slot's build.
- One configured agent means one model both writes and audits the work.
- A step with no `timeout` runs without limit. No token budget: agent flags in `exec` carry any cap.
- The container restarts after every try and before every row. A server a step needs starts and stops inside one `run`.
- A podman fault inside `exec` exits 125 and counts as a failed try. A step command that exits 125 reads the same.
- The transcript keeps full stdout and stderr of every try. Redirect big output to a file.
- Slots share the CPU, so timings from a `run` step carry noise from other slots. Set `workers: 1`, or lock inside the script.
- The commit message is the agent reply, unchecked. Agents that wrap it in a code fence produce a commit subject of "```".
- Built images carry no tag and stay until `podman image prune`.
- Mounts use the host path inside the container, so a podman machine must share that path.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.
- `systemctl stop` and `launchctl bootout` leave rows in flight running.
- macOS: `devices` must stay empty. Device nodes live in the podman machine VM.
- macOS: a Mac asleep pauses the VM. Steps in flight can hit `timeout` and spend tries.
- macOS: the podman machine does not start at login. Until it runs, `serve` stops with exit 2 every 10 s.
- Locks sit on `~/swarm`, which must be a local filesystem. NFS breaks them.
- A worker hung outside a step holds its slot until killed.
- A git or podman fault in one slot stops `serve` for every track.

## Contributing

Build, checks and design rules: [CONTRIBUTING.md](CONTRIBUTING.md). Security reports: [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
