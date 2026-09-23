# 007 — step checkpoints

**What.** A row cut short resumes after its last commit step, not from base. Commit steps are the checkpoints; a ref per commit step records how far the row got.

**Why.** A crash or restart repays the whole step list.

- `slot.rs:77` `checkout()` deletes a standing branch with no land ref and replays the row from base. Contract limit: "A row's failure or crash costs its whole step list."
- alpha `parse-18-cache`, 2026-09-07, stage model: 4 of 5 turns ran, 25 files, +561 / -73, no commit. Restart at `21:42:23Z` dropped the tree. Only transcripts survived.
- Since 014, work before a commit step already sits in a commit. The tool throws it away on replay.

**Done when.** Row with steps prompt, agent commit, `run`, prompt, agent commit. Kill `serve` during step 4. Restart: branch stands at the step 2 commit, the row runs steps 3 to 5 alone, lands with two commits over base. `scswarm log` shows steps 1 and 2 once.

## Shape

State, git only:

- Host writes ref `refs/swarm/step/<id>/<n>` at the commit of commit step `n`, right after the commit. No commit message carries state.
- Resume point of a cut-short row: highest `n` whose ref commit is the branch tip. Tip past the ref, or no ref: replay from base, as today.
- Land clears the row's step refs. `reset` clears them with the branch.
- Failed row: replay from base on the new trunk tip. A failed attempt ran against a base the new tip replaces.

Flow:

- `slot.rs` `checkout()`: branch stands, resume point `n` found: `checkout --force` the branch, clean, hand `n` back. Else: today's path.
- `script.rs` `play()`: start at step `n + 1`. Agent session opens fresh: session id lives in the container, not in git.
- `pool.rs`: passes the resume point from `checkout` to `play`.

## Rules

- Resume only when steps `1..=n` of the row are unchanged since the checkpoint. Open: the tool holds no copy of old steps. Options: fingerprint of steps `1..=n` in the ref name, `refs/swarm/step/<id>/<n>-<fingerprint>`; or drop resume on any trunk move, since the board lives on the trunk.
- Checkpoint text is tool text: no prompt, no agent text in refs.
- Merge session: no checkpoint. A conflicted stack replays in full.
- No new dependency.

## Steps

1. `state.rs`: `step_ref(id, n)`, resume point read.
2. `git.rs`: list refs under a prefix, delete a prefix.
3. `slot.rs`: resume path in `checkout()`.
4. `script.rs`, `pool.rs`: start index, ref write after each commit step.
5. `reset.rs`: clear step refs.
6. `docs/spec/contract.md`: Git section gains the step ref; limit on a replay changes. `docs/refs/architecture.md`: "Row is atomic" becomes "Commit step is atomic".
7. Tests: `state.rs` resume point from a ref list. Nothing else.
8. Verify: `cargo test`, `make lint`, the "Done when" kill test on a live slot.

## Limits

- Work after the last commit step is lost on a cut. Bounded by how often the board commits.
- Resumed row opens a fresh agent session: prompts after the checkpoint lose the context of prompts before it. Board can re-read files the earlier steps wrote.
- Container rebuild by `init` loses every agent session; resume already opens fresh ones, so nothing extra is lost.
