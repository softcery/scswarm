# Status columns: start time and context fill

**What.** `scswarm status` adds two columns: `started`, when the last attempt of the row began, and `context`, tokens in the agent context against the window, at the last finished turn.

**Why.**
- `status` reads git refs and row locks alone (`src/status.rs` lines 46-90). An operator who wants to know how long a row runs, or how close its session is to the window limit, reads `run.jsonl` by hand.
- No start time exists. The first transcript line lands only after step 1 ends (`src/call.rs` lines 109-131). A replay appends to the same `run.jsonl`, so step numbers restart in one file (`docs/spec/contract.md` line 277).
- The agent reply keeps `session_id` and `result` alone (`src/call.rs` lines 261-274). The rest of stdout drops on a passed try.
- The agent is opaque: `exec` must print `session_id` and `result` (`docs/spec/contract.md` line 167). Claude, Codex and Gemini CLI report usage in three different shapes. A parser for any of them puts agent layout in the tool.
- Measured on Claude Code `-p --output-format json`, 2026-10-27, one turn of 3 API calls (`num_turns: 3`):

| Field | Value | Meaning |
|---|---|---|
| `usage.iterations` | 1 entry: input 32, cache read 13823, cache creation 576 | last API call alone |
| `usage` top level | input 34, cache read 20073, cache creation 8149 | sum of all 3 calls |
| `modelUsage.<model>.contextWindow` | 1000000 | window of the model |

  Context fill = sum of the 3 input fields of the last `iterations` entry = 14431. The top-level sum, 28256, counts the shared prefix once per call.

**Done when.** `make e2e` runs a row whose stub agent replies with `context`. `scswarm status` prints a `started` time for that row and its `context` as `used/window`. A waiting row prints `-` in both.

## Design

### Contract: two optional reply fields

- `agents.*.exec` can add one top-level object to its JSON reply: `context: { used, window }`. `used`: tokens in context at the last API call of the turn. `window`: tokens the model takes.
- `context` absent, null, or missing a whole number in either field: reads as absent. No fault, no failed try.
- The tool reads no agent-native shape. Each project maps its agent output to these 2 fields.
- A pipe in `exec` cannot do the mapping. `Turn::argv` appends resume flags and prompt after the full `exec` text (`src/call.rs` lines 139-155), so the prompt reaches the last command of the pipe. The mapping lives in a wrapper script in the project image.

Wrapper for Claude Code, in this repo's image as `/usr/local/bin/claude-swarm`:

```sh
#!/bin/sh
out=$(claude "$@")
code=$?
if [ "$code" -ne 0 ]; then
  printf '%s\n' "$out"
  exit "$code"
fi
printf '%s\n' "$out" | jq -c '.usage.iterations[-1]? as $last | . + {context: {
  used: ($last.input_tokens + $last.cache_read_input_tokens + $last.cache_creation_input_tokens),
  window: ([.modelUsage[]?.contextWindow] | max)
}}'
```

`scswarm.yaml` of this repo: `exec` names `claude-swarm` in place of `claude`. `Containerfile.swarm` copies the script and installs `jq`.

### Transcript

`transcript::Entry` today is one struct of 11 optional fields (`src/transcript.rs` lines 17-42). Readers infer the line kind from the fields present. A third kind makes that guess worse, so `Entry` turns into a tagged type:

```rust
struct Entry { at: u64, #[serde(flatten)] kind: Kind }

#[serde(tag = "kind", rename_all = "lowercase")]
enum Kind {
    Start { base: String },
    Try { step: usize, #[serde(flatten)] sent: Sent, exit, session, result, context, stdout, stderr, fault },
    Commit { step: usize, commit: String, message: String },
}

enum Sent { Prompt(String), Run(String) }

struct Context { used: u64, window: u64 }
```

- Fields of `Try` other than `step` and `sent` stay `Option`, as today.
- `Start.base`: the `to` tip the attempt starts from (`Job::base`).
- `Context`: one type for the reply, the transcript and `status`. Both numbers or no `context`.

Start line:

- `Attempt::attempt` appends `Start` to `run.jsonl` as its first act, before credentials and image (`src/work.rs` lines 150-153). An image build is part of the attempt, so its time counts.
- One line per attempt. A replay writes a new one.

Reply:

- `call::Reply` (`src/call.rs` lines 64-67) gains `context: Option<Context>`.
- `call::parse` reads `context` next to `session_id`. `Turn::kept` writes it to the entry. A failed try keeps none: its step runs again.

### Reader

New `status::Usage` per row, read from the host transcripts of the row:

- `started`: `at` of the last `Start` in `run.jsonl`.
- `context`: the `Try` with the highest `at` across `run.jsonl` and `merge.jsonl` that holds `context`. The merge steps run in the row session (`docs/refs/architecture.md` line 70), so its fill continues the row's.
- No transcript dir, no such line, or a line that fails to parse: `-`.
- `status --tracks` reads a clone: no host transcripts, both columns `-`.

### Columns

- Header: `project row state branch worker started context`.
- `started`: UTC, `YYYY-MM-DDTHH:MMZ`, for example `2026-10-27T14:03Z`. Conversion from Unix seconds by the civil-from-days algorithm (Howard Hinnant), in `status`. No new dependency.
- `context`: `<used>k/<window>k`, thousands of tokens rounded down, for example `14k/1000k`.

### Log

`log::block` (`src/log.rs` lines 56-83) matches on `Kind` in place of testing fields. `Start` renders `run start base <sha>`. A `Try` with `context` adds `context <used>/<window>` to its header.

## Steps

1. `transcript`: `Entry` as `at` plus tagged `Kind`; `Sent`, `Context`.
2. `call`: writers build `Kind::Try`; `Context` in `Reply`, read in `parse`, kept in `Turn::kept`. `script::Session::commit` builds `Kind::Commit`.
3. `work`: `Kind::Start` at the head of `Attempt::attempt`.
4. `status`: `Usage` reader, two columns, UTC formatter.
5. `log`: start line and context in the block header.
6. `tests/e2e.rs`: stub agent prints `context`; assert both columns.
7. This repo: `claude-swarm` wrapper, `jq` in `Containerfile.swarm`, `exec` in `scswarm.yaml`.
8. Docs: contract reply fields, `status` header and columns, `run.jsonl` start line; README example wrapper.

## Notes

- `context` shows the last finished turn. A turn in progress shows the fill of the turn before, or `-` on the first turn. A live value needs agent-native session files inside the container: agent layout, out of scope.
- `context.used` counts input of the last API call. The reply of that call adds its output tokens to the next turn. Unverified against Claude Code `/context`.
- `context.window` takes the max over `modelUsage`. A turn that calls a small helper model and the main model reports the main window; a turn on the helper model alone reports its window.
- The transcript lives on the host. RFC 0020 (branch `rfc/transcript-refs`) publishes transcripts to clones. Its `Entry::public` drops `stdout` and `stderr`; on the tagged type it touches `Kind::Try` alone. The RFC that lands second rebases its `Entry` edits. With RFC 0020 built, `status --tracks` can read both columns from the log ref.
- UTC alone: local time needs a time zone database. Unix `TZ` rules are not in `std`.
- A `run.jsonl` from before this change has no `kind` field. With no compatibility kept, `log` prints such lines raw (`src/log.rs` lines 57-59) and `status` prints `-`.
- Width: the table in one measured run reached 140 columns. The two new columns add about 30.
- Unverified: Codex and Gemini CLI reply shapes. Each needs its own wrapper; this RFC ships the Claude one alone.
