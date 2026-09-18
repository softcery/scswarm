# 0 — slot affinity

**What.** Among idle slots, `serve` prefers one whose worktree HEAD is an ancestor of the row's base, else the lowest idle index.

**Why.** A chained row's base holds its parent's commit. The slot that ran the parent keeps a warm `target/`. Lowest-index pick sends the row to a cold one.

**Done when.** Three-row chain on a two-slot board: rows 2 and 3 run on the slot that ran row 1 when it is idle. No stored state: `git worktree list --porcelain` prints HEAD, `git.rs` `parse_checkouts` reads it.

## Notes

- After `row-is-a-session`. Touches `pool::idle` and `serve::cycle` alone.
