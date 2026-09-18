# Contract

What a project gives the swarm and what it gets back. Binding.

## `SWARM.yaml`

YAML sequence at project root. One entry per row. Tool reads, never writes.

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

- One row is one agent session: own turns, one commit, one branch.
- `id`: key of the row. Names branch `swarm/<id>`, land ref, fail mark, transcript dir, `log`, `reset`, `status` and `after`. Unique per board.
- `id` becomes a ref, so it must pass `git check-ref-format`: no space, `..`, `~^:?*[`, control character, leading `-`, or `.lock` end. Write it lowercase ASCII: git refs are case-sensitive, the macOS default filesystem is not.
- `after`: list of ids. Absent = none.
- `agent`: key in `scswarm.yaml` `agents`. Absent = `default_agent`.
- `turns`: prompts sent in order, one session. Text goes to the agent as typed: no substitution. Anchor at first use carries a repeated turn, alias points back.
- Host appends the `commit` turn of `scswarm.yaml` after the last turn. No row writes it.
- `serve` and `status` exit 1 and name the row by `id`, or by its place when `id` is absent, on any of:
  - empty `id`, or one git refuses as a ref
  - duplicate `id`
  - empty `turns`, or a turn of blank text
  - unknown id in `after`, or a loop of ids through `after`
  - unknown `agent`
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
```

- `path`: root of the project checkout the bare repo fetches from. Absolute, or under `~/`. A path inside the checkout is not a repo and fetch refuses it.
- `trunk`: branch of that checkout every task branch starts from. Any name git takes as a ref.
- `workers`: slot count. One worktree, one container each.
- `mounts`: extra `host:container` binds, rw. Both sides absolute; host side takes `~/` for home. Build caches ride here.
- `env`: host environment variable names forwarded into every container. Credentials ride here. Value never reaches an argument list.
- These fields cannot sit in `scswarm.yaml`: `trunk` names the branch that file is read from, and the rest changes with the machine.
- `path` and `trunk` reload on the next cycle. `workers`, `mounts` and `env` are immutable while `serve` runs: containers bake them, so `init` replaces them.
- Reality differing from those three fields is drift. `serve` verifies at start and after every reload, then exits 2 naming the field. `serve` writes no container, ever.
- Absent or unparsable at start: exit 1. Unparsable later: `serve` reports `stale`, keeps the last good copy, keeps running.

## `scswarm.yaml`

Project root, read from the trunk branch.

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

- `image`: Containerfile path relative to root, or image name. Must hold every tool the agents and the project build need. `init` builds it from a throwaway worktree of the trunk ref, so untracked and ignored files stay out of the build context.
- `agents.*.exec`: command run inside the container. Prompt appended as last argument. Must print JSON with `session_id` and `result` on stdout, or plain text as `result`.
- `agents.*.resume`: arguments added from second turn on. `{session}` = id from first reply.
- `commit`: last turn of every session. Asks for the commit message; the host commits with the reply. Required, non-empty.
- `merge`: turns run in the conflicted worktree when a parent merge conflicts. `{parent}` = branch merged in, `{id}` = row id, substituted in one pass so filled text never reads as a template. Absent = no try, branch dropped. The `commit` turn closes this session too.

## Git

- Remote: bare repo `~/swarm/<project>.git`. Project pushes the trunk branch.
- `init` writes the bare repo `origin` from the host file: url `path`, refspec `+refs/heads/<trunk>:refs/swarm/trunk`. `serve` writes it when the two fields change, and again after a failed fetch, so a hand-edited bare repo heals. `status` reads the last fetch.
- The tool reads the trunk tip at ref `refs/swarm/trunk` and holds no branch name. A failed fetch reports `skip` and costs one cycle.
- Branch per row: `swarm/<id>`. Base: `refs/swarm/trunk`, then a merge of every parent in `after` that is landed and unmerged. One such parent fast-forwards, so the branch equals `swarm/<parent>`.
- The stack merges on a detached head. The branch takes its name after the last merge, so a crash mid-merge leaves no branch.
- Merge conflict on that base: `merge` turns get one try in the conflicted worktree, host commits the merge with the agent's message. That commit gets no ref. Transcript: `merge`.
- No `merge` block, a failed merge session, or a conflict left after it: the tool aborts the merge, names no branch, and reports the parent and the files. The row starts over next cycle.
- Worktree per slot: `~/swarm/slot/<project>/<n>`. Never inside the project checkout.
- A slot detaches its worktree when a row ends or fails, and again when `serve` opens it. A branch left checked out by a crash blocks every other slot.
- One commit per row, message from the agent, no trailer. A row with no diff commits empty, so state still moves.
- Ref `refs/swarm/land/<id>` holds that commit. It is the sole record of finished work: no commit message carries state.
- Ready: not started, every `after` landed or done. Running: branch stands, no land ref. Landed: land ref set. Done: the land commit is an ancestor of the trunk.
- Failed: ref `refs/swarm/fail/<id>` holds the trunk tip the last attempt ran against, and no land ref stands. `serve` skips the row while that tip stands, so a broken row costs one attempt per trunk tip, not one per cycle. The mark sits outside `refs/heads/`, so it lists as no branch and pushes with none.
- A branch with no land ref holds a session cut short. `serve` reads it as running, hands it to a slot, and the slot deletes the branch and replays the row from base.
- A landed row clears its own mark. `reset <id>` clears branch, land ref and mark of the row and of every dependant.
- Turn tries: 3 per turn, waits 1s and 2s between. Third non-zero exit fails the row and marks it. Same cap on a blank commit message.

## CLI

`PROJECT` = stem of the host file. The working directory carries no meaning.

```
scswarm init PROJECT       bare repo, image, slot worktrees and containers
scswarm serve PROJECT      foreground loop; run under systemd
scswarm status PROJECT     one line per row: id, state, branch, worker
                             state: blocked, ready, running, failed, landed, done
scswarm log PROJECT ID     transcript, one block per turn: prompt, then reply or failure
scswarm reset PROJECT ID   delete swarm/<id>, every dependant branch, their land refs and marks
```

Exit codes: 0 ok, 1 input invalid (config, board, unknown id), 2 git or podman failure. Drift between the host file and the slot set exits 2: the file parses and the machine is intact, and the fix is `init`.

## Container

Rootless podman, `--userns=keep-id`, one long-lived container per slot, named `scswarm-<project>-<n>`. It sleeps; every turn is one `podman exec`. Mounts: slot worktree rw at same path, bare repo ro at same path, `mounts` rw. Every bind carries the `z` flag, so an SELinux host relabels it shared. Credentials arrive as `env` names. Network on. Agent runs with its own permission checks off.

Git inside the container is read-only: the index sits under the bare repo. `git status`, `git log`, `git diff` work. `git add`, `git mv`, `git commit`, `git checkout` fail with `Read-only file system`. Host stages every change with `add --all` at the row commit, so a plain `mv` lands. Project prompts and command files must not ask the agent for a git write.

Image: `image` names a Containerfile on the trunk, built as `localhost/scswarm/<project>:latest`, or an image podman resolves. An untagged name reads as `:latest`, the form podman reports.

## Limits

- Board read from the trunk branch only.
- A turn that starts with `{` must be quoted: bare `- {commit}` reads as a flow mapping and fails. A brace later in the line is safe.
- A row's failure or crash costs its whole turn list: the replay starts from base.
- A replay appends to the same transcript, so turn numbers restart inside `run.jsonl`.
- `status` prints one line per row. A task written as three rows reads as three lines; the tool learns no id pattern, so it groups nothing.
- `init` replaces every slot container and drops every slot at or above `workers`. Stop `serve` first.
- The image build context is the trunk ref. A build that reads an untracked or ignored file fails after that file leaves the ref.
- Containers carry the `mounts` and `env` they baked as labels. A release that changes that text reads as drift until `init` runs again.
- A trunk change leaves running rows on their old base. A trunk without a row's landed commit reads that row as `landed` again, not `done`.
- Chained rows on a moved trunk take the newer base, so a chain can hold a trunk merge.
- Mounts use the host path inside the container, so the podman machine must share that path.
- `env` values come from the `serve` process. A refreshed credential needs a restart.
- A row that fails waits for a new trunk tip. A parent that lands while the trunk stands still leaves it waiting: move the trunk or run `reset`.
- One agent call can still run without limit. The cap counts tries, not time or tokens.
- A kill between the failure and the mark write loses the mark. The row runs once more.
- A kill between the commit and the land ref write leaves a branch the next cycle replays.
- Stop `serve` before `reset`. A worker running that row recreates the refs at its next commit.
- One `serve` per project. Two share the slot worktrees and fight over the same branches.
- A conflicting stack costs one merge session per trunk tip, then the row waits.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.
- Slot pick is the lowest idle index, so a chained row can land on a cold build cache.
- No per-turn budget. Agent flags in `exec` carry any cap.
