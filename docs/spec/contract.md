# Contract

What a project gives the swarm and what it gets back. Binding.

## Track file

Each track is a branch named `<tracks><name>`, for example `track/laser`. Its root holds `SWARM.yaml`. The branch is orphan: it shares no history with code. Tool reads, never writes.

```yaml
from: main
at: a1b2c3d
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

- Track name = branch name minus the host file `tracks` prefix: `laser`. It holds no `/`.
- `from`: branch the work belongs to. Must stand on the remote while `to` is absent. Absent later: no row reads `merged`.
- `at`: commit sha, 4 to 40 hex digits, on `from`. `to` starts here. Absent = the `from` tip when `to` is made. Checked against `from` only while `to` is absent.
- `to`: output branch. Rows start at its tip and merge back into it. Other than `from`, outside the `tracks` prefix. One track per `to`.
- `priority`: integer. Higher runs first. Absent = 0.
- `rows`: one entry per row. Absent = none.
- One row is one session: own steps, one branch, one land ref.
- `id`: key of the row inside the track. `<track>/<id>` names branch `swarm/<track>/<id>`, land ref, fail refs, transcript dir, `log`, `reset` and `status`. Unique per track.
- `id` becomes a ref, so it must pass `git check-ref-format` and hold no `/`: no space, `..`, `~^:?*[`, control character, leading `-`, or `.lock` end. Write it lowercase ASCII: git refs are case-sensitive, the macOS default filesystem is not.
- `after`: list of ids of the same track. Absent = none.
- `agent`: key in `scswarm.yaml` `agents`. Absent = `default_agent`. Runs every prompt and agent commit of the row.
- `steps`: run in order, one session. Last step must be a commit step. Anchor at first use carries a repeated step, alias points back.

Step kinds:

| Step | Form | Runs |
|---|---|---|
| prompt | string, or `prompt: <text>` | one agent turn in the row session, text as typed |
| run | `run: <text>` | `sh -c <text>` in the slot container, worktree as working dir |
| agent commit | `commit:` with no value | `commit` prompt of `scswarm.yaml` in the row session; host commits with the reply |
| literal commit | `commit: <text>` | host commits with the text; no agent call |

- The agent session spans the whole row. `run` and literal commit steps leave it open.
- Steps join through the worktree alone: a `run` step writes files, a later step reads them. No output passes into a prompt.
- `run` text goes to the shell as typed. No substitution.
- Policy fields, on prompt, run and agent commit steps:

| Field | Meaning | Default |
|---|---|---|
| `tries` | tries before the step counts as failed | prompt and agent commit 3, run 1 |
| `wait` | seconds after a failed try, doubling per try | 1 |
| `wait_max` | cap of `wait` | 60 |
| `timeout` | seconds before a try is killed; counts as a failed try | absent = no cap |
| `continue_on_error` | `run` alone: spent tries move to the next step | false |

- Lookup: step field, then `defaults.<kind>` of `scswarm.yaml`, then the default above. Agent commit reads `defaults.prompt`.
- A literal commit takes no other field.
- Each case below skips the track. `status` prints the other tracks, then exits 1 and names the track and the field, and the row by `id`, or by its place when `id` is absent or fails its check. A track name that fails its check is named by its fault alone. `serve` reports the same text as `skip`, once per cause, assigns no row of that track, and lets rows in flight finish; the next valid track tip resumes it:
  - track name with `/`, or one git refuses as a ref
  - `from` or `to` git refuses as a branch, `to` equal to `from`, `to` under the `tracks` prefix
  - `at` not a hex sha
  - while `to` is absent: `from` absent on the remote, `at` not on `from`, or the remote turning down the push that makes `to`
  - 2 tracks name one `to`: both skip
  - empty `id`, an `id` with `/`, or one git refuses as a ref
  - duplicate `id`
  - unknown id in `after`, or a loop of ids through `after`
  - unknown `agent`
  - empty `steps`, or a last step that is not a commit
  - a step with no kind key or two, blank text, unknown field, a field its kind does not take
  - `tries` or `timeout` of 0, or a resolved `wait` above the resolved `wait_max`
- Step errors name the step number and field: ``track `laser` SWARM.yaml row `rag-04-retriever`: field `steps` step 4: field `timeout` must be seconds, 1 or more, or stay absent``.
- Row removed = row gone from swarm. Its commits stay on `to`; branch and land ref stay until `reset`.

## `<project>.yaml`

Host file, one per project, at `${XDG_CONFIG_HOME:-~/.config}/scswarm/<project>.yaml`. Project
name = file stem, ascii letters, digits, `.`, `_`, `-`.

```yaml
remote: git@github.com:org/alpha.git
tracks: track/
workers: 3
mounts:
  - ~/alpha/records:/records
  - ~/.cargo/registry:/cargo/registry
devices:
  - /dev/dri
env:
  - match: ai-*
    names: [AI_TEAM_CLAUDE_TOKEN]
  - match: "*"
    names: [ANTHROPIC_API_KEY]
poll: 5
```

- `remote`: git URL or repo path the bare repo fetches from and pushes to. A leading `~/` reads as home. Push needs a deploy key or token on the host, never in a container.
- `tracks`: branch name prefix of every track branch. Any text that makes a branch name git takes.
- `workers`: slot count. One worktree, one container each.
- `mounts`: extra `host:container` binds, rw. Both sides absolute; host side takes `~/` for home. Build caches ride here.
- `devices`: host device paths, absolute. A folder stands for each device node under it, symlinks skipped. Each node passes as `--device` at the same path. A GPU rides here. A bind in `mounts` cannot carry a device: the `z` relabel of a device node fails.
- `env`: ordered rules. `match` is a glob on track names, `*` for any run of characters. The first rule that matches names the credentials forwarded into each container of that track's rows, as environment variables. No match: none. Value never reaches an argument list.
- `poll`: seconds between loop cycles, 1 or more. Absent = 5.
- These fields cannot sit in a repo: `remote` and `tracks` say where repo text comes from, `env` keeps one team's key from another team's track, and the rest changes with the machine.
- Every field reloads on the next cycle.
- `serve` converges slots each cycle, at start too: drops each idle slot at or above `workers`, and frees the branch of each idle slot below it once per `serve` life. A busy slot drops after its row ends. The worker makes the worktree of its slot and the container. It remakes the container when it differs from the row in `mounts`, device nodes or image ID. A GPU node renumbered at boot remakes it at the next row. `init` writes no slot.
- `env` names and credential values apply from the next row on. No container bakes them.
- Absent or unparsable at start: exit 1. Unparsable later: `serve` reports `stale` once per cause, keeps the last good copy, keeps running. A credential check that fails counts as unparsable.

## Credentials

Directory `${XDG_CONFIG_HOME:-~/.config}/scswarm/credentials/`, shared by every project of the machine. One file per credential: file name = name in `env`, contents = value.

- The directory must grant group and others nothing: mode `0700`.
- One trailing `\n` strips. Other bytes pass as is.
- Sole source: the `serve` environment carries no credential.
- Each host file load checks the directory mode and one regular file per name in any `env` rule. It reads no value. A host file with no `env` names needs no directory.
- The worker reads the values of its row's names at row start. A file gone since the check: exit 2, no fail mark.
- Errors name the directory or the credential, never the value.

## `scswarm.yaml`

Code tree, read at the row's base: the `to` tip, or the commit `to` starts at.

```yaml
image: Containerfile.swarm
default_agent: claude
agents:
  claude:
    exec: CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1 claude --model opus -p --output-format json
    resume: --resume {session}
commit: "Reply with the commit message for the work in this session. Message only."
defaults:
  prompt: { tries: 3, timeout: 3600 }
  run: { tries: 1, timeout: 7200 }
merge:
  - "Resolve every conflict of the merge of row {id} into {parent}."
  - run: make lint
  - commit:
```

- `image`: Containerfile path relative to root, or image name. Must hold every tool the agents and the project build need. The worker builds it for every row from a throwaway worktree of the base, so untracked and ignored files stay out of the build context. The image takes no tag: the slot container runs it by ID. An image name the worker pulls when the host lacks it. A failed build or pull fails the row. Agents and image travel together, so a track cannot name an agent its image lacks.
- `agents.*.exec`: command run inside the container. Prompt appended as last argument. Must print JSON with `session_id` and `result` on stdout, or plain text as `result`. With `resume` set, a reply with no `session_id` fails the try. Must return only after the turn's work ends: a harness that sends work to the background and returns leaves the next step over half-done work, and the restart after the try kills that work.
- `agents.*.resume`: arguments added from second turn on. `{session}` = id from first reply. Absent = every prompt opens a new session.
- `commit`: prompt of every agent commit step. Asks for the commit message; the host commits with the reply. Required, non-empty.
- `defaults`: policy per step kind, keys `prompt` and `run`, fields as on a step. `continue_on_error` sits under `run` alone. Checked at parse: an error names the field, for example `defaults.prompt.tries`.
- `merge`: steps run in the row session, in the conflicted worktree, when the merge of the row into the `to` tip conflicts. Same kinds and rules as row steps: last step a commit. `{parent}` = the `to` branch, `{id}` = row id, filled into prompt steps alone, in one pass so filled text never reads as a template. Absent = no try, the row fails. The first commit closes the merge.

## Git

- Remote: `remote` of the host file. Humans push `from` branches, track branches, and commits on `to`. The tool pushes `to`, land refs and fail refs.
- Workers and `reset` write land refs and fail refs in the bare repo alone. `serve`, each cycle they changed since its last push, and `reset`, when done, push `refs/swarm/*` with force and prune: the remote then holds exactly the bare repo's set.
- Bare repo `~/swarm/<project>.git`, `origin` = `remote`, refspec `+refs/heads/*:refs/remotes/origin/*`: the layout of any clone. `init` writes it; `serve` writes it when `remote` changes, and again after a failed fetch, so a hand-edited bare repo heals. A failed fetch reports `skip` and costs one cycle. `status` reads the last fetch.
- `init` on a new bare repo fetches the land refs and fail refs of the remote once. Without that fetch, the first push of `refs/swarm/*` deletes them on the remote.
- `serve` makes each absent `to` at `at`, or at the `from` tip, and pushes it.

| Ref | Holds | Where |
|---|---|---|
| `refs/remotes/origin/*` | fetched remote branches | bare repo |
| `swarm/<track>/<id>` | row in progress, then its last commit | bare repo |
| `<to>` | track output | remote, pushed with no force |
| `refs/swarm/land/<track>/<id>` | last commit of the row session | bare repo, mirrored to remote |
| `refs/swarm/fail/<track>/<id>/track` | track tip of the failed attempt | bare repo, mirrored to remote |
| `refs/swarm/fail/<track>/<id>/to` | `to` tip of the failed attempt | bare repo, mirrored to remote |

- Worker, per row: build the image at the `to` tip, make `swarm/<track>/<id>` at that tip, run the steps, write the land ref. Then merge the row into the `to` tip on no branch, always with a merge commit, so `to` stays first parent and each row is one merge on `to`. Push that commit to `to` with no force. A push the remote refuses as behind fetches, merges onto the new tip, pushes again. A push the remote turns down, for example branch protection, fails the row. Then drop the fail refs.
- A land ref outside `to`, for example after a force-push dropped it, reads as queued: the next worker merges it into `to` again with no new session.
- Merge conflict: `merge` steps get one try in the conflicted worktree, in the row session. The first commit step closes the merge. Merge commits get no ref. Transcript: `merge`.
- No `merge` block, a failed merge session, or a conflict left after it: the tool aborts the merge and reports the files. The row fails.
- A `git merge` error with no `MERGE_HEAD` is git failing, not a conflict: no `merge` steps run.
- Worktree per slot: `~/swarm/slot/<project>/<n>`. Never inside a checkout.
- A slot detaches its worktree when a row ends or fails, and again when `serve` attaches it. A branch left checked out by a crash blocks every other slot.
- One commit per commit step, no trailer. The host stages with `add --all`: ignored files miss every commit. A commit step with no diff commits empty, so state still moves.
- The land ref is the sole record of finished work: no commit message carries state.

| State | Fact |
|---|---|
| waiting | a row in `after` is not pushed or merged |
| queued | can start: no worker holds it, every `after` pushed or merged |
| running | a worker holds the row lock |
| failed | fail refs stand, and equal the track tip and the `to` tip |
| pushed | land commit is an ancestor of `to` |
| merged | land commit is an ancestor of `from` |

- Precedence: merged, pushed, running, failed, then queued or waiting.
- A failed row retries when the track tip or the `to` tip moves, so a broken row costs one attempt per tip pair, not one per cycle. Fail refs sit outside `refs/heads/`, so they list as no branch.
- A branch with no land ref holds a session cut short, unless a worker holds the row lock. `serve` reads it as queued and the worker replays the row from the `to` tip.
- A pushed row clears its own fail refs. `reset <track>/<id>` clears branch, land ref and fail refs of the row and of every dependant in its track, here, then on the remote.
- A try fails on non-zero exit, a signal, `timeout`, a blank agent commit reply, or no `session_id` from an agent with `resume`. A step whose tries are spent fails the row and marks it, except a `run` step with `continue_on_error`.
- A git or podman failure in a slot is no row failure. It writes no mark: the worker exits 2, `serve` exits 2, and the row replays on the next start. Workers on other slots run on. A failed image build or pull is a row failure: the worker reaches podman before it builds, so a podman that answers nothing stays a machine fault.
- Locks: flock on 5 standing directories, no file written. The kernel drops a lock when its holder dies, so a crash leaves nothing to clean.

| Lock | Directory | Holder | `serve` while held |
|---|---|---|---|
| slot | `~/swarm/slot/<project>/<n>` | worker, whole life | no release, drop or assign |
| row | `~/swarm/log/<project>/<track>/<id>/` | `serve` takes it before spawn, worker inherits it as stdin and holds it to exit | no assign; the row reads `running`. Held from spawn, so no restart of `serve` reads the row free while its worker lives |
| project | `~/swarm/<project>.git` | `serve`, whole life | a second `serve` exits 2 |
| remote | `~/swarm/<project>.git/refs` | each fetch and push, for its length | fetch waits |
| build | `~/swarm/build/<project>` | worker, for its image build | none: workers alone take it. Builds run one at a time, so a second build of one Containerfile hits the cache and gets the same image ID |

## CLI

`PROJECT` = stem of the host file. The working directory carries meaning for `status --tracks` alone. `ROW` = `<track>/<id>`.

```
scswarm init PROJECT            bare repo, origin, first fetch; safe while `serve` runs
scswarm serve PROJECT           foreground loop; run under systemd or launchd
scswarm status PROJECT          one line per row of every track: row, state, branch, worker
                                  state: waiting, queued, running, failed, pushed, merged
scswarm status --tracks PREFIX  same, read from the clone in the working directory; that
                                  clone fetches +refs/swarm/*:refs/swarm/*. Running reads as queued
scswarm log PROJECT ROW         transcript, one block per try or commit: prompt, command or commit
                                  message, then reply, stdout, stderr, faults
scswarm reset PROJECT ROW       delete swarm/<track>/<id>, every dependant branch, their land refs
                                  and fail refs, here, then on the remote
scswarm work PROJECT --slot N --track NAME --row ID --track-tip SHA --base SHA
             [--env NAME]... [--mount HOST:GUEST]... [--device PATH]...
                                internal, hidden from --help: one row attempt; `serve` spawns one
                                  per assignment. Reads the track file at the track tip, config at
                                  the base, branches from the base, forwards each NAME
```

Exit codes: 0 ok, 1 input invalid (host file, track file, config, unknown row), 2 git or podman failure. `serve` exits 1 only on a host file invalid at start: an invalid track or config skips that track. `work` exits 0 once it pushes the row or writes the fail refs, 2 on a git or podman failure. A worker exit other than 0 exits `serve` 2.

`work` argv and stdin bind across versions: a `serve` started before `cargo install` spawns the new binary. Stdin holds the row lock: `work` refuses a stdin that is not that directory. A change to either needs a release note.

`serve` spawns each worker with stdout and stderr shared, so worker lines reach the same journal, also after `serve` dies. A worker whose `serve` died runs to its end; the process manager reaps it. `systemctl stop` and `restart` end `serve` alone under the shipped unit, `KillMode=process`: rows in flight finish, no new row starts. The shipped launchd agent does the same through `AbandonProcessGroup`, for `launchctl kickstart -k` and `bootout`.

Scheduling: each idle slot takes a queued row. Highest `priority` first; among tracks of equal priority, the track with fewest rows running, so tracks take turns; file order inside a track. No preemption.

## Container

Rootless podman, `--userns=keep-id`, one long-lived container per slot, named `scswarm-<project>-<n>`. It sleeps; every try of a prompt, `run` or agent commit step is one `podman exec`, then `podman restart --time=0`. The restart kills every process the try left, so a background job never reaches the next step or the commit. The worker restarts the container before each row, so a process a dead worker left inside never reaches the next row. Mounts: slot worktree rw at same path, bare repo ro at same path, `mounts` rw. Every bind carries the `z` flag, so an SELinux host relabels it shared. `devices` pass as `--device`, with no relabel. Credentials arrive as `--env=NAME`, values in the podman environment. Network on. Agent runs with its own permission checks off.

Git inside the container is read-only: the index sits under the bare repo. `git status`, `git log`, `git diff` work. `git add`, `git mv`, `git commit`, `git checkout` fail with `Read-only file system`. Host stages every change with `add --all` at each commit step, so a plain `mv` lands. Project prompts and command files must not ask the agent for a git write.

Image: `image` names a Containerfile at the base, built with no tag and run by ID, or an image podman resolves. An untagged name reads as `:latest`, the form podman reports.

## Limits

- A prompt that starts with `{` must be quoted: bare `- {x}` reads as a flow mapping and fails. A brace later in the line is safe.
- A prompt YAML reads as a bool or number, for example `- yes` or `- 42`, must be quoted.
- A row's failure or crash costs its whole step list: the replay starts from the `to` tip, commits of earlier steps included.
- A replay appends to the same transcript, so step numbers restart inside `run.jsonl`.
- Output a later row needs must sit on a tracked path. The slot cleans with no `-x`: an ignored file misses the commit, and stays in the slot worktree for the next row on that slot.
- The restart after each try kills every process in the container. A server a step needs starts and stops inside one `run`.
- Slots share the host CPU. Timings from a `run` step carry noise from other slots: set `workers: 1`, or lock inside the script.
- `status` prints one line per row. A task written as three rows reads as three lines; the tool learns no id pattern, so it groups nothing.
- A slot due to drop waits for its row to end. A long row holds the old slot past the change.
- Rows of tracks with different images run on different images at once. A slot that switches image remakes its container.
- The worker builds the image for every row. A Containerfile with `COPY . .` rebuilds for every row. Builds of one project wait on each other, so a slot can wait on another slot's build. Built images carry no tag and stay until `podman image prune`.
- The image build context is the base. A build that reads an untracked or ignored file fails after that file leaves the base.
- Containers carry the `mounts` and device nodes they baked as labels. A release that changes that text remakes every container at its next row.
- A pulled image name is not pulled again. A new push under the same tag needs `podman pull`.
- `merged` misses squash and rebase merges: they write new commits, so the land commit is never an ancestor of `from`.
- `reset` after a push leaves the old commits on `to`. The tool never rewrites `to`. A human reverts the row's merge commit there, `git revert -m 1`.
- `to` takes no `from` commit made after `to` starts. A human merges `from` into `to`.
- Land refs and fail refs reach the remote on the next `serve` cycle. While `serve` is down, a clone reads a row a live worker pushed as queued.
- A push refused as behind merges again onto the new tip from scratch: a conflict runs the `merge` steps again.
- Repair merges a row dropped from `to` by a force-push again. To drop a row, delete it from the track file, or revert it on `to`.
- A row that needs another track's output names that track's `to` as `from`. `after` never crosses tracks.
- A track with several last rows is valid: `to` joins them all.
- A failed row retries on every move of `to`, so each pushed row of its track costs it one more attempt.
- A merge session after a crash or a repair opens a new agent session: the row session lived in the dead worker.
- An urgent track waits for a free slot, up to one row's length.
- A 429 usage limit fails rows of every track that shares the credential. Per-team `env` limits it to one team.
- A refreshed credential reaches the next row. Rows in flight keep the old value.
- A step with no `timeout` runs without limit. No cap counts tokens.
- A podman fault inside `exec` exits 125 and counts as a failed try. A step command that exits 125 reads the same: `sh -c` passes the code through.
- The transcript keeps full stdout and stderr of every try. Redirect big output to a file.
- A git or podman failure in one slot stops `serve` for every track. That row replays from base; rows on other slots run on. A remote out of reach is such a failure; a remote that turns a push down fails the row.
- A kill between the failure and the fail ref write loses the mark. The row runs once more.
- A kill between the commit and the land ref write leaves a branch the next cycle replays. A kill after the land ref write leaves a row the next cycle merges into `to`.
- `reset` a row only while no worker runs it. A running worker recreates the refs at its next commit, and stopping `serve` stops no worker.
- `systemctl stop` leaves rows in flight running. Hard stop: `systemctl --user kill --kill-whom=all scswarm@<project>`.
- `launchctl bootout` leaves rows in flight running. Hard stop: `pkill -f 'scswarm work <project> '`. launchd has no command that kills an abandoned process group.
- macOS: `devices` must stay empty. Device nodes live in the podman machine VM, so the host check fails each entry.
- macOS: `mounts` host sides must sit under a podman machine share: `/Users`, `/private`, `/var/folders` by default.
- macOS: a Mac asleep pauses the VM. Steps in flight can hit `timeout` and spend tries.
- macOS: the podman machine does not start at login. Until it runs, workers exit 2 and launchd starts `serve` again every 10 s.
- systemd logs a left-over process line per running worker at each restart. Expected under `KillMode=process`.
- Ctrl-C in a terminal reaches the whole process group: `serve` and its workers stop.
- Rows started before an upgrade finish on the old binary. Branch, land ref and fail ref names must read the same in both versions.
- A worker takes the slot lock after spawn. A new `serve` started before then can release that slot or run another row on it first; the worker waits for the lock, then runs. A slot dropped in that wait fails the worker with exit 2, and the row replays.
- Locks need a local filesystem. `~/swarm` on NFS breaks flock.
- A new bare repo leaves the slot worktrees of the old one dangling, and `serve` exits 2 on them. Delete `~/swarm/slot/<project>` with the bare repo.
- A hung worker holds its slot until killed. Step `timeout` bounds a try; a hang outside a step, for example in git, has no bound.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.
- Slot pick is the lowest idle index, so a chained row can land on a cold build cache.
- No token budget. Agent flags in `exec` carry any cap.
- GitHub accepts `refs/swarm/*` from a write deploy key: create, fetch in a fresh clone, delete, all checked 2026-10-22, private repo on a personal account, ssh. The refs stay out of the web UI branch list. Unverified: organization repos, GitHub Enterprise, branch rulesets that target all refs, and the delete of a ref the remote lacks, which git 2.39 over a local path accepts with exit 0.
