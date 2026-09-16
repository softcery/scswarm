# 010 — trailer reader strict

**What.** `git::trailer_commit()` errors on a commit that holds one `Swarm-*` key twice. No skip to an older commit.

**Why.** `git.rs:289` `parse_trailer()` joins repeated values with `\x1e` and compares the joined string to the row id. Two `Swarm-Task` lines read as "no match", `find_map` walks to the older commit. alpha 2026-09-11, commit `483038d`: `Swarm-Task` and `Swarm-Stage: 3/3` twice, `state::derive()` read the stage 2 commit, stage 3 ran again, 6 min 57 s and one extra commit. See [issue-trailer-duplicate.md](../../notes/issue-trailer-duplicate.md).

State reader tolerated corrupt state. A reader of state must reject malformed input, whatever the writer does.

**Done when.** A branch whose tip holds `Swarm-Task` twice makes `scswarm status` exit 2 with the commit sha and key `Swarm-Task`, and `cargo test` holds a case for it.

## Shape

`git.rs`:

- `parse_trailer(line, key, value, want)` returns `Result<Option<Trailer>>`. Split each field on `\x1e`. Count above 1 = `Error::Failed`: "commit `<sha>`: trailer `<key>` must appear once, found <n>".
- `trailer_commit()`: loop with `?` in place of `find_map`.

`state.rs`: unchanged. Error rises through `derive()`, `serve` and `status` report it.

Recovery: `reset <id>` drops the branch. Contract Git section states it.

## Rules

- No repair. Reader names the commit, operator decides.
- Matching key with empty `want` stays a skip, not an error. 007 checkpoint commits carry `Swarm-Task` and no `Swarm-Stage`; the stage reader must walk past them.
- Stage commit writer stays as is; 006 gives it structured input so agent text carries no `Swarm-` line.

## Steps

1. `git.rs`: split, count, error, loop.
2. Test: `parse_trailer` on a line with two values names the key. Nothing else; `trailer_commit` needs a repo.
3. `docs/spec/contract.md` Git section: duplicate trailer = error, `reset` clears.
4. `make lint`.

## Notes

- 013 deletes both trailers, so it deletes this plan too. Land this one only if 013 slips: the reader it hardens guards the binary running today.

- 007 extends `trailer_commit()` to a want list. Land 010 first, 007 keeps the count check per key.
