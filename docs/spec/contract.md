# Contract

What a project gives the swarm and what it gets back. Binding.

## `SWARM.yaml`

YAML sequence at project root. One entry per row. Tool reads, never writes.

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

- One row is one session: own steps, one branch, one land ref.
- `id`: key of the row. Names branch `swarm/<id>`, land ref, fail mark, transcript dir, `log`, `reset`, `status` and `after`. Unique per board.
- `id` becomes a ref, so it must pass `git check-ref-format`: no space, `..`, `~^:?*[`, control character, leading `-`, or `.lock` end. Write it lowercase ASCII: git refs are case-sensitive, the macOS default filesystem is not.
- `after`: list of ids. Absent = none.
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
- `serve` and `status` exit 1 and name the row by `id`, or by its place when `id` is absent, on any of:
  - empty `id`, or one git refuses as a ref
  - duplicate `id`
  - unknown id in `after`, or a loop of ids through `after`
  - unknown `agent`
  - empty `steps`, or a last step that is not a commit
  - a step with no kind key or two, blank text, unknown field, a field its kind does not take
  - `tries` or `timeout` of 0, or a resolved `wait` above the resolved `wait_max`
- Step errors name the step number and field: ``SWARM.yaml row `rag-04-retriever`: field `steps` step 4: field `timeout` must be seconds, 1 or more, or stay absent``.
- Row removed = task gone from swarm. Branch and land ref stay until `reset`.

## `<project>.yaml`

Host file, one per project, at `${XDG_CONFIG_HOME:-~/.config}/scswarm/<project>.yaml`. Project
name = file stem, ascii letters, digits, `.`, `_`, `-`.

```yaml
path: ~/alpha
trunk: blahblah
workers: 3
mounts:
  - ~/alpha/records:/records
  - ~/.cargo/registry:/cargo/registry
env:
  - ANTHROPIC_API_KEY
poll: 5
```

- `path`: root of the project checkout the bare repo fetches from. Absolute, or under `~/`. A path inside the checkout is not a repo and fetch refuses it.
- `trunk`: branch of that checkout every task branch starts from. Any name git takes as a ref.
- `workers`: slot count. One worktree, one container each.
- `mounts`: extra `host:container` binds, rw. Both sides absolute; host side takes `~/` for home. Build caches ride here.
- `env`: host environment variable names forwarded into every container. Credentials ride here. Value never reaches an argument list.
- `poll`: seconds between loop cycles, 1 or more. Absent = 5.
- These fields cannot sit in `scswarm.yaml`: `trunk` names the branch that file is read from, and the rest changes with the machine.
- Every field reloads on the next cycle.
- `serve` converges slots each cycle, at start too: creates each missing slot below `workers`, drops each slot at or above it, rebuilds each slot whose container differs in `mounts` or image ID. A busy slot changes after its row ends. `init` writes no slot.
- `env` names apply from the next row on. No container bakes them.
- Absent or unparsable at start: exit 1. Unparsable later: `serve` reports `stale` once per cause, keeps the last good copy, keeps running.

## `scswarm.yaml`

Project root, read from the trunk branch.

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
  - "Resolve every conflict of the merge of {parent} into {id}."
  - run: make lint
  - commit:
```

- `image`: Containerfile path relative to root, or image name. Must hold every tool the agents and the project build need. `init` builds it from a throwaway worktree of the trunk ref, so untracked and ignored files stay out of the build context. An image name `init` pulls when the host lacks it. `serve` reads the image ID each cycle: a rebuild under the same name rolls to every slot as it goes idle. Image absent: `serve` reports `skip` and assigns nothing that cycle.
- `agents.*.exec`: command run inside the container. Prompt appended as last argument. Must print JSON with `session_id` and `result` on stdout, or plain text as `result`. With `resume` set, a reply with no `session_id` fails the try. Must return only after the turn's work ends: a harness that sends work to the background and returns leaves the next step over half-done work, and the restart after the try kills that work.
- `agents.*.resume`: arguments added from second turn on. `{session}` = id from first reply. Absent = every prompt opens a new session.
- `commit`: prompt of every agent commit step. Asks for the commit message; the host commits with the reply. Required, non-empty.
- `defaults`: policy per step kind, keys `prompt` and `run`, fields as on a step. `continue_on_error` sits under `run` alone. Checked at parse: an error names the field, for example `defaults.prompt.tries`.
- `merge`: steps run in the conflicted worktree when a parent merge conflicts. Same kinds and rules as board steps: last step a commit. `{parent}` = branch merged in, `{id}` = row id, filled into prompt steps alone, in one pass so filled text never reads as a template. Absent = no try, branch dropped. The first commit closes the merge.

## Git

- Remote: bare repo `~/swarm/<project>.git`. Project pushes the trunk branch.
- `init` writes the bare repo `origin` from the host file: url `path`, refspec `+refs/heads/<trunk>:refs/swarm/trunk`. `serve` writes it when the two fields change, and again after a failed fetch, so a hand-edited bare repo heals. `status` reads the last fetch.
- The tool reads the trunk tip at ref `refs/swarm/trunk` and holds no branch name. A failed fetch reports `skip` and costs one cycle.
- Branch per row: `swarm/<id>`. Base: `refs/swarm/trunk`, then a merge of every parent in `after` that is landed and unmerged. One such parent fast-forwards, so the branch equals `swarm/<parent>`.
- The stack merges on a detached head. The branch takes its name after the last merge, so a crash mid-merge leaves no branch.
- Merge conflict on that base: `merge` steps get one try in the conflicted worktree. The first commit step closes the merge. Merge commits get no ref. Transcript: `merge`.
- No `merge` block, a failed merge session, or a conflict left after it: the tool aborts the merge, names no branch, and reports the parent and the files. The row fails and waits for a new trunk tip.
- A `git merge` error with no `MERGE_HEAD` is git failing, not a conflict: no `merge` steps run.
- Worktree per slot: `~/swarm/slot/<project>/<n>`. Never inside the project checkout.
- A slot detaches its worktree when a row ends or fails, and again when `serve` opens it. A branch left checked out by a crash blocks every other slot.
- One commit per commit step, no trailer. The host stages with `add --all`: ignored files miss every commit. A commit step with no diff commits empty, so state still moves.
- Ref `refs/swarm/land/<id>` holds the last commit of the row. It is the sole record of finished work: no commit message carries state.
- Ready: not started, every `after` landed or done. Running: branch stands, no land ref. Landed: land ref set. Done: the land commit is an ancestor of the trunk.
- Failed: ref `refs/swarm/fail/<id>` holds the trunk tip the last attempt ran against, and no land ref stands. `serve` skips the row while that tip stands, so a broken row costs one attempt per trunk tip, not one per cycle. The mark sits outside `refs/heads/`, so it lists as no branch and pushes with none.
- A branch with no land ref holds a session cut short. `serve` reads it as running, hands it to a slot, and the slot deletes the branch and replays the row from base.
- A landed row clears its own mark. `reset <id>` clears branch, land ref and mark of the row and of every dependant.
- A try fails on non-zero exit, a signal, `timeout`, a blank agent commit reply, or no `session_id` from an agent with `resume`. A step whose tries are spent fails the row and marks it, except a `run` step with `continue_on_error`.
- A git or podman failure in a slot is no row failure. It writes no mark: `serve` exits 2, and the row replays on the next start.

## CLI

`PROJECT` = stem of the host file. The working directory carries no meaning.

```
scswarm init PROJECT       bare repo and image; safe while `serve` runs
scswarm serve PROJECT      foreground loop; run under systemd
scswarm status PROJECT     one line per row: id, state, branch, worker
                             state: blocked, ready, running, failed, landed, done
scswarm log PROJECT ID     transcript, one block per try or commit: prompt, command or commit
                             message, then reply, stdout, stderr, faults
scswarm reset PROJECT ID   delete swarm/<id>, every dependant branch, their land refs and marks
```

Exit codes: 0 ok, 1 input invalid (config, board, unknown id), 2 git or podman failure. A git or podman failure inside a slot exits `serve` 2.

## Container

Rootless podman, `--userns=keep-id`, one long-lived container per slot, named `scswarm-<project>-<n>`. It sleeps; every try of a prompt, `run` or agent commit step is one `podman exec`, then `podman restart --time=0`. The restart kills every process the try left, so a background job never reaches the next step or the commit. `serve` restarts each container a killed `serve` left when it attaches the slot, so a process left inside never reaches the next row. Mounts: slot worktree rw at same path, bare repo ro at same path, `mounts` rw. Every bind carries the `z` flag, so an SELinux host relabels it shared. Credentials arrive as `env` names. Network on. Agent runs with its own permission checks off.

Git inside the container is read-only: the index sits under the bare repo. `git status`, `git log`, `git diff` work. `git add`, `git mv`, `git commit`, `git checkout` fail with `Read-only file system`. Host stages every change with `add --all` at each commit step, so a plain `mv` lands. Project prompts and command files must not ask the agent for a git write.

Image: `image` names a Containerfile on the trunk, built as `localhost/scswarm/<project>:latest`, or an image podman resolves. An untagged name reads as `:latest`, the form podman reports.

## Limits

- Board read from the trunk branch only.
- A prompt that starts with `{` must be quoted: bare `- {x}` reads as a flow mapping and fails. A brace later in the line is safe.
- A prompt YAML reads as a bool or number, for example `- yes` or `- 42`, must be quoted.
- A row's failure or crash costs its whole step list: the replay starts from base, commits of earlier steps included.
- A replay appends to the same transcript, so step numbers restart inside `run.jsonl`.
- Output a later row needs must sit on a tracked path. The slot cleans with no `-x`: an ignored file misses the commit, and stays in the slot worktree for the next row on that slot.
- The restart after each try kills every process in the container. A server a step needs starts and stops inside one `run`.
- Slots share the host CPU. Timings from a `run` step carry noise from other slots: set `workers: 1`, or lock inside the script.
- `status` prints one line per row. A task written as three rows reads as three lines; the tool learns no id pattern, so it groups nothing.
- A slot due to drop or rebuild waits for its row to end. A long row holds the old slot past the change.
- While slots rebuild, rows run on old and new image at once.
- The image build context is the trunk ref. A build that reads an untracked or ignored file fails after that file leaves the ref.
- Containers carry the `mounts` they baked as a label. A release that changes that text rebuilds every slot as it goes idle.
- `init` pulls a named image only when the host lacks it. A new push under the same tag needs `podman pull`.
- A trunk change leaves running rows on their old base. A trunk without a row's landed commit reads that row as `landed` again, not `done`.
- Chained rows on a moved trunk take the newer base, so a chain can hold a trunk merge.
- Mounts use the host path inside the container, so the podman machine must share that path.
- `env` values come from the `serve` process. A refreshed credential needs a restart.
- A row that fails waits for a new trunk tip. A parent that lands while the trunk stands still leaves it waiting: move the trunk or run `reset`.
- A step with no `timeout` runs without limit. No cap counts tokens.
- A podman fault inside `exec` exits 125 and counts as a failed try. A step command that exits 125 reads the same: `sh -c` passes the code through.
- The transcript keeps full stdout and stderr of every try. Redirect big output to a file.
- A git or podman failure in one slot stops `serve`. Every row in flight replays from base.
- A kill between the failure and the mark write loses the mark. The row runs once more.
- A kill between the commit and the land ref write leaves a branch the next cycle replays.
- Stop `serve` before `reset`. A worker running that row recreates the refs at its next commit.
- One `serve` per project. Two share the slot worktrees and fight over the same branches.
- A conflicting stack costs one merge session per trunk tip, then the row waits.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.
- Slot pick is the lowest idle index, so a chained row can land on a cold build cache.
- No token budget. Agent flags in `exec` carry any cap.
