# 006 — harness adapters

**What.** Agent gains `command` and `reply` templates, stage turns become `Text` | `Command` | `Commit`. Any agent harness runs from `scswarm.ron` with no code change.

**Why.** Tool fits one harness today.

- `turn.rs:138` `parse()` reads one JSON object, keys `session_id` and `result`. opencode `--format json` prints JSONL events (`sessionID` per event, text in `part.text`), so session never parses and every turn opens a fresh session. Stage context dies between turns.
- `turn.rs:92` builds `sh -c '<exec> "$@"'` with the prompt last. opencode `run.ts` has no slash parsing: a command needs `--command <name>` with the message as arguments. So `/execute {task}` arrives as chat text and `.claude/commands/execute.md` never loads.
- `stage.rs:13` `COMMIT_PROMPT` and `config.rs:14` `COMMIT_TURN` put prompt text and a prompt convention in `src/`. Invariant `architecture.md:21` bars both.
- `stage.rs:117` takes the commit reply as prose. Subject = "```" on 29 of 34 stage commits across two boards; commit `483038d` carries agent-written `Swarm-Task`. See [issue-commit-message-unchecked.md](../../notes/issue-commit-message-unchecked.md).
- `turn.rs:71` reads exit code alone. Account usage limit exits 1, three tries in 6 s mark the row failed, row idle 4 h 55 min. See [issue-usage-limit-fails-row.md](../../notes/issue-usage-limit-fails-row.md).

**Done when.** One board runs rows with `agent` `claude` and `agent` `opencode` at once, each lands its task, and `scswarm log ID` shows one session per stage for both plus a commit message with no event noise. `grep -ri 'claude\|opencode\|codex' src/` returns nothing.

## Shape

`config.rs`, sole reader of the project file:

```
stages: [
  (turns: [
    Command("execute", "{task}"),
    Text("Correct, clean, beautiful?"),
    Commit("Reply with the commit message for the work in this session. Message only."),
  ]),
],
agents: {
  "claude": (
    exec: "claude --dangerously-skip-permissions -p --output-format json",
    resume: "--resume {session}",
    command: Text("/{name} {args}"),
    reply: (session: "/session_id", result: "/result"),
    commit: (args: "--json-schema {schema}", reply: "/structured_output"),
    unavailable: (path: "/api_error_status", value: "429"),
  ),
  "opencode": (
    exec: "opencode run --format json --auto",
    resume: "--session {session}",
    command: Argv("--command {name}"),
    reply: (session: "/sessionID", result: "/part/text"),
  ),
},
```

- `Turn` enum replaces the `{commit}` string slot. `Commit` carries its own prompt, so `src/` holds no prompt text.
- `Command::Text` puts name and args in the message. `Command::Argv` puts the name in argv, args stay the message. A harness with no command system uses `Text("Read .claude/commands/{name}.md, follow it. Arguments: {args}")`, so the model opens the file itself. Three placements cover every harness.
- `ReplyPath { session: Option<String>, result: String }` holds RFC 6901 JSON pointers. `serde_json::Value::pointer` reads them, no new dependency.
- `commit: Option<CommitReply { args: String, reply: String }>`. `{schema}` = tool-owned JSON schema `{ subject: string, body: string }`, rendered by `serde_json::to_string`, compact, no whitespace, so it stays one argv token through the whitespace split. `reply` points at the object. Absent: host takes `result` whole as message; harness with no structured output, stated limit.
- `unavailable: Option<Match { path: String, value: String }>`. Reply where the pointer reads a scalar whose text form equals `value` = environment failure, not row failure. Number `429` matches `"429"`. Absent: every non-zero exit is a row failure, as today.

Commit message, host-built from the object: subject, blank line, body, then host trailers. Boundary checks, failed check = failed try under the cap: subject one line, non-empty; no line of subject or body starts with `swarm-`, compared case-insensitive, since git matches trailer keys that way. Host owns that key space.

Unavailable, wait in place:

- `turn.rs`: reply matches `unavailable` = sleep `hold` seconds from the host file, same turn again. No try counted. Transcript entry holds the reply, event `hold` names the elapsed wait.
- Session, worktree, stage position stay. A limit in turn 4 of 5 costs nothing when it lifts.
- Other slots: `serve` keeps assigning. A new row hits the limit at its first turn and waits the same way. Cost: one call per slot per `hold`.
- `status`: a slot whose last transcript entry is a hold reads `held`, not `running`.
- Reset time in the reply is prose, "resets 7am (UTC)". Not parsed. Fixed interval.
- Aborting to the pool instead loses the stage in flight: worktree resets to the last stage commit, session dies. Rejected.

`turn.rs`:

- Rename struct `Turn` to `Call`; `config::Turn` takes the name. `stage.rs:95` updates.
- Argv order: `exec` tokens, command argv, resume argv, prompt. Prompt stays one argv token, so agent text never reaches the shell.
- `parse()`: read stdout line by line, skip lines that hold no JSON, apply the pointers. Session = first hit. Result = last hit. No hit, or `reply` absent, = whole stdout trimmed, as today.
- Template substitution reuses `turn.rs:99`: split on whitespace, replace the slot per token. `resume` uses it for `{session}`, `Command::Argv` for `{name}`.

`stage.rs`: renders `config::Turn` into prompt text plus command argv. `Text` substitutes `{task}`. `Command` renders through the agent's `command`. `Commit` sends its text, then the host commits with the reply, unchanged from `stage.rs:116`.

Checks in `config.rs`:

- Pointer must start with `/`. Error names `reply.session` or `reply.result`.
- `resume` present requires `reply.session`; without an id there is nothing to resume with.
- A `Command` turn in any stage requires every agent to define `command`. Board rows pick the agent per row, so any agent can run any stage.
- Every stage holds at least one `Commit`. Replaces the `COMMIT_TURN` check at `config.rs:170`.
- `commit.reply` and `unavailable.path` start with `/`. Error names the field.
- Host file `hold` must be present when any agent sets `unavailable`. Error names both fields.
- Rendered prompt trimmed, so `Text("/{name} {args}")` with empty args leaves no trailing space.

## Rules

- Tool learns no harness name and no dialect. Every dialect lives in `scswarm.ron`.
- Keep no back-compat. The `{commit}` string form and the plain `turns: ["text"]` form die with this change.
- No new dependency.

## Steps

1. `config.rs`: add `Turn`, `Command`, `ReplyPath`; extend `Agent`; drop `COMMIT_TURN`; write the five checks; update the module test to the new form.
2. `turn.rs`: rename `Turn` to `Call`, take command argv, rewrite `parse()` to the pointer rule.
3. `stage.rs`: render the three turn kinds, drop `COMMIT_PROMPT`. `Commit` turn adds `commit.args`, reads the object, builds the message, runs the boundary checks.
4. `turn.rs`, `host.rs`, `status.rs`, `event.rs`: `unavailable` match and wait, `hold` field, `held` line, `hold` event.
5. `scswarm.ron`: turns in enum form, `opencode` agent, mount `~/.local/share/opencode/auth.json:/home/dev/.local/share/opencode/auth.json`. Bind the file, never the directory: the directory holds session storage and two slots would write one index.
6. `Containerfile.swarm`: `npm install -g opencode-ai`.
7. Symlink `.opencode/commands` to `.claude/commands`. Formats match: frontmatter plus `$ARGUMENTS` body (`.claude/commands/execute.md:1-5`). Single source, no copies.
8. `docs/spec/contract.md`: rewrite the `agents.*` and `stages[].turns` bullets, host file `hold`, add the limits below. `docs/refs/architecture.md:33`: new argv order and parse rule.
9. Tests: `config.rs` parses the two-agent example and names the bad field; `turn.rs` parses a one-object reply and a five-event JSONL reply. Nothing else; `stage.rs` and `pool.rs` need real podman.
10. Verify: `cargo build`, `cargo test`, `make lint`, then a live board with one claude row and one opencode row.

## Limits

- Prompt rides as the last argv token. A harness that reads the prompt from stdin only does not fit. Add an `input` field when such a harness turns up.
- Harness with no session resume: leave `resume` absent, every turn opens a fresh session, stage context is lost between turns.
- Result must reach stdout. A harness that writes the final message to a file only does not fit.
- opencode wraps a space-holding argv in literal quotes (`run.ts`: `arg.includes(" ") ? '"' + … + '"'`), so the model sees the prompt quoted.
- opencode emits one `text` part per text block. Text before a tool call in the final turn drops out of a commit message, since result takes the last hit.
- Command frontmatter keys differ per harness (`allowed-tools` against `agent`). Each ignores the other's.
- Harness without structured output: commit message is prose, fence and all. Stated per agent by absent `commit`.
- Held turn probes every `hold` seconds per slot while the limit stands. Slot stays busy; nothing else can run under an account-wide limit, so no capacity lost.

## Notes

Unverified, check on first run:

- Claude CLI 2.1.263 `--json-schema`: field that carries the object in `--output-format json`. `/structured_output` above is a guess. Read one live reply before writing the pointer.
- 009 lands first: `{id}` and `{task}` are the slots `Text` substitutes. 010 lands first: strict reader guards the branch while the writer changes.

- codex accepts `codex exec --json … resume <id> <prompt>`, meaning `exec` flags before the `resume` sub-subcommand.
- opencode reasoning parts carry no `part.text`. `--thinking` is off by default, so expected absent; if present they pollute `result`.
- opencode `--auto` exists on the installed version. Pin the npm version in the image if it does not.
- opencode auth in the container: `auth.json` mount covers OAuth providers, `env` covers key-only providers. Neither tested headless here.
