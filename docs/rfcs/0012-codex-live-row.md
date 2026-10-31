# Run a codex row on a live board

**What.** Run one codex row end to end, correct the two pointers [RFC 0009](0009-harness-adapters.md) could not verify.

**Why.** [RFC 0009](0009-harness-adapters.md) ships the codex config against a binary read, not a live stream. Quota on this workstation was spent when [RFC 0009](0009-harness-adapters.md) was written: probe on 2026-09-16 returned `{"type":"error","message":"You've hit your usage limit. … try again at Sep 22nd, 2026 3:44 AM."}`, exit 1.

**Done when.** A board row with `agent` `codex` lands its task, and `scswarm log ID` shows one session plus a commit message with no event noise.

## Steps

1. Export `CODEX_ACCESS_TOKEN`, add it to the host file `env`, run `init`, start `serve`.
2. Capture one live stream: `codex exec --json "reply with the word ping" > codex.jsonl`.
3. Read the `item.completed` line. Tag key is `item_type` or `type`: set `reply.result.when.at` in `scswarm.yaml` to whichever it is, and correct the `call.rs` test fixture to the captured lines.
4. Confirm `codex login status` inside a slot container exits zero after the `exec` prelude ran once, so the prelude skips the network on later turns.
5. Confirm `codex exec --json resume <id> <prompt>` continues the session: turn 2 of a row must answer a question about turn 1.
6. Hit the limit on purpose, or wait for one: the row must read `held` in `status`, keep its worktree and session, and resume when the limit lifts.

## Notes

- [RFC 0009](0009-harness-adapters.md) lands first: it ships the codex agent. Current code already sets the file format, the one-session row and the step list.

- A wrong tag key fails the try under the prompt `tries` cap, default 3, and never lands a commit: [RFC 0009](0009-harness-adapters.md) drops the whole-stdout fallback for an agent that sets `reply.result`.
- `login --with-access-token` accepting a refresh-free token is unverified. If it refuses, the fallback is `OPENAI_API_KEY` through the same `env` list, which costs API-key billing instead of plan quota.

