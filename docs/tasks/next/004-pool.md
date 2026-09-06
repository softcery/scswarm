# 004 — worker pool, log, reset, service

**What.** `workers` slots in parallel, stacked branches, `log`, `reset`, systemd user unit.

**Why.** Independent chains run at once. Operator sees and undoes.

**Done when.** Three ready rows run on three slots at once; a row whose parent is landed and unmerged branches from the parent; `reset` drops a branch and every dependant; `log` tails a transcript; unit file starts `serve` on login and restarts on exit.

## Shape

- `serve.rs`: assignment over idle slots, base branch rule.
- `reset.rs`: dependant walk over the board, branch delete, worktree reset.
- `log.rs`: read transcript.
- `systemd/sercswarm.service`: user unit, `Restart=always`.

## Plan

1. Pool in `serve.rs`.
2. Base branch rule.
3. `reset`, `log`.
4. Unit file.

## Notes
