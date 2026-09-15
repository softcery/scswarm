# 011 — context in status, turn usage in log

**What.** `status` gains column `context`: tokens in the live agent session over the agent window, per running row. `log` prints the usage of each try: tokens and cost.

**Why.** `status` shows `running` and nothing of session size. The transcript keeps no usage, so a per-turn breakdown needs the harness session files inside the slot containers.

- `turn.rs:73` records `session` and `result` on a successful try. Reply `usage` drops.
- Claude Code 2.1.263, `-p --output-format json`, haiku, 2 API calls: top-level `usage` sums the calls. `input_tokens` 18 = 10 + 8, `cache_creation_input_tokens` 17609 = 17220 + 389. That is turn usage, not context fill.
- Same reply: last call prompt = 8 + 17220 + 389 = 17617 tokens, found only in `usage.iterations[0]`. Field has no docs; a pointer cannot pick the last element of a longer array.
- Window sits at `modelUsage.<model id>.contextWindow`. Key varies per model; no pointer reaches it. `claude-opus-5` reads 1000000.
- Same CLI, `--output-format stream-json --verbose`, 3 API calls: stdout holds 6 `assistant` events, each with its call's `/message/usage`. Last one: 8 + 17528 + 142 = 17678. `output_tokens` in those events reads 4, 1, 1: partial.
- Same mode: the `result` event alone carries top-level `usage` and `total_cost_usd`.
- alpha ui-00 to ui-05 and beta api-01-transport, 23 stage sessions: peak fill 57k to 588k, no compaction. Read from Claude Code session files inside the slot containers; `~/.claude` has no mount, so `init` drops them with the container.
- Failed tries spend too: alpha ui-01 stage 1 turn 1, first try, `total_cost_usd` 20.74 before a 429.

**Done when.** Live board, one claude row mid-stage:

- `scswarm status` `context` cell equals input + cache_read + cache_creation of the last `assistant` event of that stage's latest try, over `window`.
- `scswarm log ID --stage N` header of each try carries the `usage` fields and `total_cost_usd` of its `result` event, failed tries included.
- `grep -ri 'claude\|opencode\|codex' src/` returns nothing.

## Shape

`config.rs`:

```
agents: {
  "claude": (
    exec: "claude --dangerously-skip-permissions -p --output-format stream-json --verbose",
    resume: "--resume {session}",
    reply: (
      session: "/session_id",
      result: "/result",
      context: [
        "/message/usage/input_tokens",
        "/message/usage/cache_read_input_tokens",
        "/message/usage/cache_creation_input_tokens",
      ],
      usage: [
        ("out", "/usage/output_tokens"),
        ("read", "/usage/cache_read_input_tokens"),
        ("write", "/usage/cache_creation_input_tokens"),
        ("cost", "/total_cost_usd"),
      ],
    ),
    window: 1000000,
  ),
},
```

- `reply.context: Vec<String>`, RFC 6901 pointers, default empty. A line counts when at least one pointer reads an unsigned integer. Its value = sum of the pointers that read one. Last counting line wins, same rule as `result`.
- `reply.usage: Vec<(String, String)>`, name and pointer, default empty. Each pointer reads a JSON number; last hit wins. Names are display text only, so the tool learns no metric.
- Context sums; usage does not. Fill has no single field in any reply seen; each usage figure has one.
- `window: Option<u64>`: token window of the model `exec` names. Absent: cell shows used tokens alone.
- Example `context` pointers read input fields only. Input size is fixed when a call starts; stream `output_tokens` is partial.

`transcript.rs`, new, sole reader and writer of transcript files:

- `Entry` moves here from `turn.rs`, gains `context: Option<Context>`, `Context { used: u64, window: Option<u64> }`, and `usage: Vec<(String, Number)>`, skipped when empty. Window rides in the entry, so readers resolve no agent and a later config change leaves old entries true.
- `append(path, &Entry)`: body of `Turn::record`.
- `read(path) -> Result<Vec<Line>>`, `Line` = `Entry` or raw text, so `log` keeps printing a line that parses as no entry. Absent file keeps the `log.rs:71` rule: `Invalid`.
- `context(path) -> Result<Option<Context>>`: last entry that carries `context`. Absent file = `None`.

`turn.rs`:

- `parse()` runs on every try, failed ones included. `Reply` gains `context: Option<u64>` and `usage: Vec<(String, Number)>`.
- `record` calls `transcript::append`.

`status.rs`:

- Column 6, `context`. Row in `Running { stage }` with a slot holding its branch: `transcript::context` of stage `stage`. Rendered used over window, each rounded to the nearest thousand with `k`, below 1000 raw: `412k/1000k`, or `412k` with no window. Every other row: `-`.
- Row with no slot has no live session: a restart reruns the stage in a fresh one.
- Doc comment `status.rs:14`: every cell but `context` derives from git.

`log.rs`:

- Reads through `transcript::read`. Try header appends `context=<used>/<window>` and one `<name>=<value>` per `usage` pair, in config order, raw numbers. Shape matches `event` lines.

## Rules

- Tool learns no harness field name and no metric name. Pointers, names and window live in `scswarm.ron`.
- `context` and `usage` are display only. `pool`, `stage` and `state` never read them, so state still derives from git.
- Transcript files open in `transcript` alone.
- No new dependency.

## Steps

1. `transcript.rs`: move `Entry` and the append body out of `turn.rs`, the read body out of `log.rs`. Add `Context`, `usage`, `context()`.
2. `config.rs`: `reply.context`, `reply.usage`, agent `window`. Checks, each error names the field: pointer starts with `/`; `usage` name non-empty, no whitespace, no `=`; `window` above 0.
3. `turn.rs`: parse on every try, `Reply.context`, `Reply.usage`, sum rule, record through `transcript`.
4. `status.rs`: column 6, cell rule, doc comment.
5. `log.rs`: read through `transcript`, header fields.
6. `scswarm.ron`: claude `exec` to `stream-json --verbose`, `reply.context`, `reply.usage`, `window`.
7. `docs/spec/contract.md`: `agents.*` bullets for `reply.context`, `reply.usage` and `window`; CLI `status` line adds `context`, `log` line adds the header fields; limits below. `docs/refs/architecture.md`: Known limits gains the stale-value limit.
8. Test: `turn.rs` parse of a JSONL reply with two `assistant` events and one `result` event: context = second event sum, usage = `result` fields. Extends the 006 JSONL fixture.
9. Verify: `cargo build`, `cargo test`, `make lint`, live board as Done when.

## Limits

- Context comes from the last finished try. The try in flight is not counted.
- After a restart, the cell shows the dead session's last value until the new session's first try ends. Both sessions append to one transcript file.
- Merge sessions (stage 0) run on a detached head. No slot holds the branch, so the cell stays `-`. `log` still shows their usage.
- `window` is set by hand per agent. A model change in `exec` without a `window` change gives a wrong ratio.
- `total_cost_usd` is the harness figure. Claude Code reports `costBasis` `list` in 4 of 4 alpha failed-try replies: list price, not the bill of a subscription account.
- A try that dies before the `result` event records no usage. Tokens spent before a kill or a crash stay uncounted.
- Failed-try entries store full stdout: under stream-json, every event.
- Harness with no usage on stdout: leave `reply.context` and `reply.usage` empty; cell shows `-`, header shows no fields.

## Notes

- 006 lands first: JSONL `parse()` and `reply` pointers.
- 006 `unavailable` must match per line under stream-json. `api_error_status` sits on the `result` event only.
- `session_id` sits on every stream event, so 006 first-hit rule holds.
