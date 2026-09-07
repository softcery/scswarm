# Contract

What a project gives the swarm and what it gets back. Binding.

## `SWARM.md`

Markdown table at project root. Header row, separator row, one row per task. Tool reads, never writes.

```
| task | after | agent |
| docs/tasks/next/trace-06-decode.md | trace-05-record | |
| docs/tasks/next/parse-15-postures.md | parse-10-repair, trace-06-decode | codex |
```

- `task`: any text. Substituted as `{task}` in turns. Id = file stem when text is a path, else the text.
- `after`: comma list of ids. Empty = none.
- `agent`: key in `sercswarm.ron` `agents`. Empty = `default_agent`.
- Unknown id in `after`, duplicate id, unknown agent: `serve` and `status` exit non-zero, name the row.
- Row removed = task gone from swarm. Branch stays until `reset`.

## `sercswarm.ron`

Project root. RON, `IMPLICIT_SOME`.

```
(
  workers: 3,
  image: "Containerfile.swarm",
  mounts: ["~/alpha/records:/records", "~/.cargo/registry:/cargo/registry"],
  env: ["ANTHROPIC_API_KEY"],
  default_agent: "claude",
  agents: {
    "claude": (
      exec: "claude --model opus --effort high --dangerously-skip-permissions --disallowedTools Agent Skill SendMessage ListAgents RemoteTrigger PushNotification -p --output-format json",
      resume: "--resume {session}",
    ),
  },
  stages: [
    (turns: ["/execute {task}", "Correct, clean, beautiful?", "Mark the impl into the task file.", "{commit}"]),
    (turns: ["/audit {task}", "Audited thoroughly, checked everything, no gaps? If any, close them.", "What's the plan to resolve all audit issues and make the implementation correct, clean, and beautiful - no compromises?", "Write the audit and the plan into that same original task file.", "{commit}"]),
    (turns: ["/execute {task}", "Correct, clean, beautiful?", "Mark the impl into the task file.", "{commit}"]),
  ],
  merge: (turns: ["Resolve every conflict of the merge of {parent} into {task}.", "{commit}"]),
)
```

- `workers`: slot count. One worktree, one container each.
- `image`: Containerfile path relative to root, or image name. Must hold every tool the agents and the project build need.
- `mounts`: extra `host:container` binds, rw. Both sides absolute; host side takes `~/` for home. Build caches ride here.
- `env`: host environment variable names forwarded into every container. Credentials ride here. Value never reaches an argument list.
- `agents.*.exec`: command run inside the container. Prompt appended as last argument. Must print JSON with `session_id` and `result` on stdout, or plain text as `result`.
- `agents.*.resume`: arguments added from second turn on. `{session}` = id from first reply.
- `stages[].turns`: sent in order, one session per stage. `{task}` substituted. `{commit}` = built-in turn: asks for a commit message, host commits with the reply.
- `stages[].agent`: optional, overrides row and default for that stage.
- `merge`: optional stage, same fields. Runs in the conflicted worktree when a parent merge conflicts. `{parent}` = branch merged in, `{task}` substituted as in a stage. Absent = no try, branch dropped.

## Git

- Remote: bare repo `~/swarm/<project>.git`. Project pushes `main`.
- `init` points the bare repo `origin` at the checkout it ran on, refspec `+refs/heads/main:refs/heads/main`. `serve` fetches `main` each cycle. `status` reads the last fetch.
- Branch per task: `swarm/<id>`. Base: `main`, then a merge of every parent in `after` that is landed and unmerged. One such parent fast-forwards, so the branch equals `swarm/<parent>`.
- The stack merges on a detached head. The branch takes its name after the last merge, so a crash mid-merge leaves no branch and the next cycle starts over.
- Merge conflict on that base: `merge` turns get one try in the conflicted worktree, host commits the merge with the agent's message and trailer `Swarm-Stage: 0/<total>`. Transcript: stage 0.
- No `merge` block, a failed merge session, or a conflict left after it: the tool aborts the merge, names no branch, and reports the parent and the files. The row starts over next cycle.
- Worktree per slot: `~/swarm/slot/<project>/<n>`. Never inside the project checkout.
- A slot detaches its worktree when a task ends or fails, and again when `serve` opens it. A branch left checked out by a crash blocks every other slot.
- Commit per stage, message from agent, trailers `Swarm-Task: <id>` and `Swarm-Stage: <n>/<total>`. A stage with no diff commits empty, so state still moves.
- Stage restart resets the worktree to the last stage commit and drops untracked files. Ignored files stay, so build caches survive.
- Landed: last stage committed. Done: that commit is ancestor of `main`. Ready: not started, every `after` done or landed.
- Failed: ref `refs/swarm-fail/<id>` holds the `main` tip the last attempt ran against. `serve` skips the row while that tip stands, so a broken row costs one attempt per `main` tip, not one per cycle. The mark sits outside `refs/heads/`, so it lists as no branch and pushes with none.
- A landed task clears its own mark. `reset <id>` clears the mark of the row and of every dependant, branch or no branch.
- Turn tries: 3 per turn, waits 1s and 2s between. Third non-zero exit fails the stage and marks the row. Same cap on a blank commit message.

## CLI

```
sercswarm init [--root DIR]   bare repo, image, slot worktrees and containers
sercswarm serve               foreground loop; run under systemd
sercswarm status              one line per row: id, state, branch, stage, worker
                              state: blocked, ready, running, failed, landed, done
sercswarm log ID [--stage N]  transcript, one block per turn: prompt, then reply or failure
sercswarm reset ID            delete swarm/<id>, every dependant branch, their fail marks
```

Exit codes: 0 ok, 1 input invalid (config, board, unknown id), 2 git or podman failure.

## Container

Rootless podman, `--userns=keep-id`, one long-lived container per slot, named `sercswarm-<project>-<n>`. It sleeps; every turn is one `podman exec`. Mounts: slot worktree rw at same path, bare repo ro at same path, `mounts` rw. Every bind carries the `z` flag, so an SELinux host relabels it shared. Credentials arrive as `env` names. Network on. Agent runs with its own permission checks off.

Image: `image` names a Containerfile in the checkout, built as `localhost/sercswarm/<project>`, or an image podman resolves.

## Limits

- Board read from `main` only.
- `init` replaces every slot container. Stop `serve` first.
- Mounts use the host path inside the container, so the podman machine must share that path.
- `env` values come from the `serve` process. A refreshed credential needs a restart.
- A row that fails waits for a new `main` tip. A parent that lands while `main` stands still leaves it waiting: move `main` or run `reset`.
- One agent call can still run without limit. The cap counts tries, not time or tokens.
- A kill between the failure and the mark write loses the mark. The row runs once more.
- Stop `serve` before `reset`. A worker running that task recreates the ref at its next commit.
- One `serve` per project. Two share the slot worktrees and fight over the same branches.
- A conflicting stack costs one merge session per `main` tip, then the row waits.
- The merge check reads `MERGE_HEAD`. An agent that commits conflict markers passes it.
- No per-turn budget. Agent flags in `exec` carry any cap.
