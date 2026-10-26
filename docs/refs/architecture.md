# Architecture

Why the tool has this shape. Contract: [docs/spec/contract.md](../spec/contract.md).

## Purpose

Fully autonomous execution of every planned task, at any time, on one workstation, with no client machine in the loop. One swarm per unix user runs many repos, one project each. Many teams share one repo: each owns its tracks. Humans write tracks and merge `to` branches. Nothing else.

## Pattern

Job queue, worker pool, explicit outcome states, persisted state. Queue = track branches on the remote. Workers = processes, one `scswarm work` per row attempt, one slot each. `serve` schedules only. State = git, pushed so any clone reads it. Textbook shape, no framework.

## Split

| Side | Holds | Lives in |
|---|---|---|
| meta | what to run, order, base, output | track file, orphan branch |
| work | code, task files, Containerfile, `scswarm.yaml` | code branches |
| machine | projects, slots, devices, credentials | host file, VM |
| project | remote, track prefix, fetch, mounts, env | host file entry |

Push rights per branch give each team its own track file. No team edits another's.

## Invariants

- **Track is input only.** Tool reads each track file, writes no file in the project. Chokepoint: `board` module has no write path. Violation: three branches edit one file, merges conflict.
- **State derives from git.** Waiting, queued, failed, pushed, merged come from branch existence, the land ref, the fail refs and ancestry against `to` and `from`; running from the row lock. Chokepoint: `state` module reads git and locks only. Violation: a crash leaves a database and a repo that disagree.
- **Remote mirrors the kept refs.** Workers and `reset` write land refs and fail refs in the bare repo alone. Each project's feed, on change, and `reset` push `refs/swarm/*` with force and prune. A bare repo whose `origin` names another URL mirrors nothing: its refs belong to the old remote. Chokepoint: `git::Git::mirror_kept`. Violation: a fault between two pushes leaves a clone reading a row the host reads as pushed.
- **Host owns git.** Agents see gitdir read-only. Every commit runs on the host with the agent's message, and nothing in `src/` reads a commit message back. Every push runs on the host. Chokepoint: `git` module, sole caller of `git commit` and `git push`. Violation: agent rewrites history, pushes, or its text steers the loop.
- **`to` only grows.** Each row lands as one merge commit with the `to` tip first parent, pushed with no force. A push refused as behind fetches and merges onto the new tip. Chokepoint: `work::Attempt::deliver`, sole pusher of `to` besides its first push in `feed`. Violation: a row erases a human commit or another row.
- **Row is atomic.** One row is one session, one branch, one land ref at its last commit. A branch with no land ref holds a session cut short: the slot resets it and replays the row from the `to` tip. Chokepoint: `slot::Slot::checkout` resets the standing branch. Violation: half-edited worktree continues with lost context.
- **Spend is bounded.** Every exec'd step caps its tries and, with `timeout`, the time per try. A failed attempt marks the track tip and `to` tip it ran against, so the row waits for new input. Chokepoint: `step::Policy`, enforced in `call`; fail refs written in `work`, read in `state`. Violation: one broken agent burns budget every cycle forever.
- **Machine fault marks no row.** Spent tries, a failed image build, a conflict left after the `merge` steps and a push of `to` the remote turns down end the row as `step::Outcome::Spent`, which writes the fail refs. The worker reaches podman before the build, so a dead podman never reads as a failed build. A git or podman failure returns `Err`: the worker exits 2, `serve` reports `skip` for its project once and rests it, no mark. Workers on other slots run on. Chokepoint: `work::Attempt`. Violation: a broken slot marks every row it takes.
- **No process outlives its try.** The slot container restarts after every exec, and before every row. Chokepoint: `call::Call` and `slot::Slot::prepare`, sole callers of `podman::Container::restart`. Violation: a job the agent sent to the background, or one a dead worker left, writes into the next step or the next row on that slot.
- **Values live in config, not code.** Tries, waits, timeouts, poll and fetch come from step fields, `defaults` and the host file. Tool defaults sit in one place each: `step` for policy, `host` for poll, fetch, rest and transport bounds. Violation: a project needs a release to change a retry count.
- **Branches are refs, not names.** Each project entry names its remote and track prefix; every branch arrives as a remote-tracking ref, the layout of any clone. Every project text that becomes a ref passes `git::valid_ref` first. Chokepoint: refspec in `git::Git::set_origin`, gate in `host` and `board`. Violation: a project whose branches the tool cannot name cannot run, or a track row kills the loop.
- **Slot set converges to the host file.** `serve` drops slots above `workers` and frees idle slots. A slot takes rows of any project. The worker makes its slot's worktree from the row's bare repo, dropping a worktree of another repo with its ignored files. It remakes the container when the row's image, `mounts`, device nodes or bare repo differ. `env` rides in each job, so it changes no container. A busy slot keeps its row and changes once idle. Chokepoint: `pool::Pool::converge`, sole caller of `slot::remove`; `slot::Slot::prepare`, sole caller of `podman::Container::create`. Violation: a slot changes under a running row, or a container runs an image its row did not name.
- **Projects share nothing writable.** Slot worktrees move whole between repos, logs and builds sit under the project name, and the host file refuses one host path in the `mounts` of two projects. Chokepoint: `host::Host::check_mounts`; `slot::Slot::prepare` drops a foreign worktree. Violation: one client's agent plants a file another client's build reads.
- **A project fault stays in its project.** Each project has one feed thread for every network call: open, fetch, mirror, first push of `to`. The scheduler cycle reads local refs alone, so no remote holds it. A failed feed rests its own project; a worker that exits 2 rests its project, 60 s doubling, until a worker of it ends well. One feed at most per project: a project added back waits for its retired feed to end. Every call to a remote runs under transport bounds, so git fails and drops its own locks. Chokepoint: `feed::Feed`; `serve::Server::follow`, sole starter of feeds; `serve::Lane::ended`; bounds in `host`. Violation: one dead or slow remote stalls or restarts every client, or a killed git leaves a lock that blocks its repo.
- **Projects take turns.** Project first: fewest rows running, then served longest ago. Inside it: `priority`, rows running in the track, file order. Track files set `priority`, so it orders tracks inside one project alone. Served order lives in `serve` memory: a restart resets it and costs one round of turns. Chokepoint: `pool::schedule`. Violation: one client's `priority` or name order takes every slot.
- **Credential values stay off argv and out of `serve`.** `serve` checks each track's credential files every cycle and reads no value; a track that fails skips. The worker reads values at row start and passes them in the podman environment, names alone as `--env`. Chokepoint: `credential`, sole reader of the credential directory; `podman::Container::exec`, sole place a value enters a command. Violation: a secret shows in `ps`, a transcript, or every child of `serve`.
- **Container is the boundary.** Agent permission checks off, container mounts fixed. One rule for every agent. Chokepoint: `podman` module builds every `run` and `exec`. Violation: two sandbox semantics, one agent escapes.
- **One row, one slot.** The scheduler hands a row to a single slot and skips it while a worker holds it. `serve` takes the row lock before spawn and the worker inherits it as stdin, so it is held from spawn to exit. The worker holds the slot lock for its life, on the slot dir, never on the worktree it moves; `serve` holds the swarm lock on `~/swarm`, so no second scheduler runs. Each fetch and push holds the remote lock, so no two update one remote-tracking ref. Each image build holds the build lock: parallel builds miss the layer cache and give slots of one image different IDs. Chokepoint: `lock`, sole caller of `File::lock`; `pool` checks own workers, then probes both locks, before release, drop and assign, and takes the row lock at assign. `status` reads the same fact from the row locks. Violation: two containers commit one branch.
- **Rows outlive `serve`.** Each row attempt is its own process. `serve` holds no row a restart cannot rebuild: kernel locks mark what a live worker holds and drop on its death. Chokepoint: `pool::Pool::assign`, sole spawner of workers. Violation: a restart, an upgrade or a fault on one slot kills every row in flight.
- **Tool knows no project.** Ids, order, base and output come from track files; image, agents, `commit`, `defaults` and `merge` steps from the project file at the row's base; workers, devices and poll, and per project remote, track prefix, fetch, mounts and env, from the host file; values from the credential files. Chokepoint: `board` reads track files, `config` reads the project file, `step` settles every step list, `host` reads the host file. Violation: second project needs a code change.

## Loop

1. Reap ended workers: exit 0 ends its project's rest, exit 1 reports, any other end rests its project.
2. Each project, one thread each, local refs alone: read each track branch under `tracks`, check its credential files, skip it while its `to` is absent, read the project file at each `to` tip, settle rows, derive states. A track that fails a check skips; a project whose feed is down, or at rest, skips; the others run.
3. Converge slots: drop each idle slot at or above `workers`, free the branch of each idle slot once per `serve` life.
4. For each idle slot, pick a queued row: project with fewest rows running, then served longest ago; inside it priority, rows running in the track, file order. Spawn `scswarm work` with the project, the track tip and the `to` tip.
5. Worker: build the image at the `to` tip, move the worktree to the project's bare repo, make or restart the container, make `swarm/<track>/<id>` at the `to` tip, run the steps, write the land ref. Merge the row into the `to` tip on no branch, resolving a conflict with the `merge` steps in the row session, push to `to` with no force; refused as behind: fetch, merge onto the new tip, push again; turned down: the row fails. Drop the fail refs. Or write the fail refs. Detach the worktree, exit.
6. Sleep `poll`, reread the host file, repeat. An unreadable host file keeps the last good copy. Rows in flight run on.

## Feed

One thread per project, started on first sight in the host file, ended when its entry goes. Each `poll`, unless at rest:

1. Make the bare repo on first sight.
2. Check that `origin` names `remote`, then write the refspec. Another URL: skip, no network call.
3. Fetch once `fetch` seconds passed. A failed fetch rewrites `origin` at the next try.
4. Push the kept refs when they changed.
5. After a fetch, make and push each absent `to`.

A failed step reports `skip` once per cause and rests the project: `fetch` seconds, doubling, up to 15 min. The scheduler assigns no row of a project whose feed is down.

## Step

Prompt and agent commit: `podman exec <slot> sh -c '<exec> "$@"' scswarm [<resume>] <prompt>`. Prompt, resume and session id ride as argv, so agent text never reaches the shell. Stdout parsed for `session_id` and `result`. The session id carries across every step of the row and into its `merge` steps.

Run: `podman exec <slot> sh -c <text>`. Track text, trusted as the team's own; no substitution, since row ids take `;`, `$` and `|`.

Literal commit: host commit, no exec.

Each try: capture with deadline, judge, record, restart container. The restart runs when capture breaks too. Fails on non-zero exit, signal, `timeout`, blank commit reply, or no session id from an agent with `resume`. Next try after `wait`, doubling to `wait_max`. Each try and each commit appends one line to `~/swarm/log/<project>/<track>/<id>/run.jsonl`, or `merge.jsonl` for the merge into `to`. Chokepoint: `transcript::Transcript`.

Steps join through the worktree alone: no step output reaches a prompt, so prompts stay as typed.

## Known limits

- One model executes and audits when only one agent is configured.
- A row's failure or crash costs its whole step list. The replay starts from the `to` tip.
- The restart after each try kills servers too. A step that needs one starts and stops it inside one `run`.
- A git or podman failure in one slot rests its project. That row replays once the rest ends. Podman broken for every slot rests every project in turn, the rest doubling until a worker ends well.
- A dead remote holds its own feed thread up to the transport bounds: 30 s to connect over ssh, about 60 s on a stalled transfer. Other projects schedule on.
- The scheduler can read refs while a fetch writes them. A cycle can see a new track tip with the old `to`; the next cycle reads both.
- A worker that exits 1 reports `skip` and rests nothing. A row that fails its input every time spawns a worker every cycle until its track or `to` moves.
- A project switch of `remote` on a standing bare repo skips the project until the bare repo is deleted.
- The ssh bound replaces any `core.sshCommand` in git config. Per-repo keys go in `~/.ssh/config`.
- Podman exit 125 from `exec` counts as a failed try. A step command that exits 125 reads the same.
- `status` prints one line per row. The tool learns no id pattern, so a task written as three rows groups into nothing.
- `reset` races a worker running that row. Stopping `serve` stops no worker.
- `systemctl stop` and `launchctl bootout` leave rows in flight running. Hard stop: `systemctl --user kill --kill-whom=all`, or `pkill -f 'scswarm work '` under launchd.
- macOS runs containers in the podman machine VM. `devices` stays empty, `mounts` sit under a VM share, a Mac asleep pauses rows.
- Rows started before an upgrade finish on the old binary. `work` argv, branch and ref names must read the same in both versions.
- A worker takes the slot lock after spawn. A new `serve` can release or use that slot first; the worker waits, or exits 2 if the slot drops.
- Locks need a local filesystem. `~/swarm` on NFS breaks flock.
- A worker hung outside a step holds its slot until killed.
- A slot due to drop waits for its row to end. Tracks on different images or projects remake containers as slots switch between them. A switch of project drops the worktree, so the next build starts cold; build caches ride on per-project `mounts`.
- One swarm per unix user. Shell access to that user reaches every project: bare repos, transcripts, credential files.
- `devices` and the network reach every project's containers.
- Worker count times cargo jobs must fit the core count; set `CARGO_BUILD_JOBS` in the image.
- Credential expiry fails every row it touches after 3 tries. Fix the credential, then `reset` each marked row or push to its track or `to`.
- `merged` misses squash and rebase merges.
- A failed row retries on every push to its `to`, its siblings' included.
- A merge session after a crash or a repair opens a new agent session.
- Kept refs reach the remote on the next `serve` cycle. While `serve` is down, a clone reads a pushed row as queued.
- `to` takes no `from` commit made after it starts. A human merges `from` into `to`.

## Smells

- Any path, dir name, prompt or branch name from a project in `src/`.
- Project text used as a ref name without `git::valid_ref`.
- A container made outside `slot::Slot::prepare`, or a slot dropped outside `pool::Pool::converge`.
- A file written under a project checkout by the tool.
- `git commit` or `git push` outside the `git` module.
- A push of `to` with force.
- A push of `refs/swarm/*` outside `git::Git::mirror_kept`.
- A merge into `to` that fast-forwards or leaves the row first parent.
- A `podman` argument outside the `podman` module.
- A credential value in an argument, an event line or a `Debug` derive.
- A writable path two projects share.
- State kept in memory that a restart cannot rebuild from git and locks.
- A row run as a thread inside `serve`.
- A commit message read for anything but the commit itself.
- A retry count, wait or timeout constant outside `step` and `host`.
- Step output passed into a prompt.
- A fail ref written for a git or podman failure.
