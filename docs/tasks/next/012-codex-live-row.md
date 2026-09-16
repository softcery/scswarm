# 012 — codex row on a live board

**What.** Run one codex row end to end, correct the two pointers 006 could not verify.

**Why.** 006 ships the codex config against a binary read, not a live stream. Quota on this workstation was spent when 006 was written: probe on 2026-09-16 returned `{"type":"error","message":"You've hit your usage limit. … try again at Sep 22nd, 2026 3:44 AM."}`, exit 1.

**Done when.** A board row with `agent` `codex` lands its task, and `scswarm log ID` shows one session plus a commit message with no event noise.

## Steps

1. Export `CODEX_ACCESS_TOKEN`, add it to the host file `env`, run `init`, start `serve`.
2. Capture one live stream: `codex exec --json "reply with the word ping" > codex.jsonl`.
3. Read the `item.completed` line. Tag key is `item_type` or `type`: set `reply.result.when.at` in `scswarm.yaml` to whichever it is, and correct the `turn.rs` test fixture to the captured lines.
4. Confirm `codex login status` inside a slot container exits zero after the `exec` prelude ran once, so the prelude skips the network on later turns.
5. Confirm `codex exec --json resume <id> <prompt>` continues the session: turn 2 of a row must answer a question about turn 1.
6. Hit the limit on purpose, or wait for one: the row must read `held` in `status`, keep its worktree and session, and resume when the limit lifts.

## Notes

- 013 and 006 land first: 013 sets the file format and the one-session row, 006 ships the codex agent.

- A wrong tag key fails the try under the 3-try cap and never lands a commit: 006 drops the whole-stdout fallback for an agent that sets `reply.result`.
- `login --with-access-token` accepting a refresh-free token is unverified. If it refuses, the fallback is `OPENAI_API_KEY` through the same `env` list, which costs API-key billing instead of plan quota.
