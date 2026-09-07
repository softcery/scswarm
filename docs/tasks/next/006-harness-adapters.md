# 006 — harness adapters

**What.** Agent gains `command` and `reply` templates, stage turns become `Text` | `Command` | `Commit`. Any agent harness runs from `sercswarm.ron` with no code change.

**Why.** Tool fits one harness today.

- `turn.rs:138` `parse()` reads one JSON object, keys `session_id` and `result`. opencode `--format json` prints JSONL events (`sessionID` per event, text in `part.text`), so session never parses and every turn opens a fresh session. Stage context dies between turns.
- `turn.rs:92` builds `sh -c '<exec> "$@"'` with the prompt last. opencode `run.ts` has no slash parsing: a command needs `--command <name>` with the message as arguments. So `/execute {task}` arrives as chat text and `.claude/commands/execute.md` never loads.
- `stage.rs:13` `COMMIT_PROMPT` and `config.rs:14` `COMMIT_TURN` put prompt text and a prompt convention in `src/`. Invariant `architecture.md:21` bars both.

**Done when.** One board runs rows with `agent` `claude` and `agent` `opencode` at once, each lands its task, and `sercswarm log ID` shows one session per stage for both plus a commit message with no event noise. `grep -ri 'claude\|opencode\|codex' src/` returns nothing.

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
- Rendered prompt trimmed, so `Text("/{name} {args}")` with empty args leaves no trailing space.

## Rules

- Tool learns no harness name and no dialect. Every dialect lives in `sercswarm.ron`.
- Keep no back-compat. The `{commit}` string form and the plain `turns: ["text"]` form die with this change.
- No new dependency.

## Steps

1. `config.rs`: add `Turn`, `Command`, `ReplyPath`; extend `Agent`; drop `COMMIT_TURN`; write the five checks; update the module test to the new form.
2. `turn.rs`: rename `Turn` to `Call`, take command argv, rewrite `parse()` to the pointer rule.
3. `stage.rs`: render the three turn kinds, drop `COMMIT_PROMPT`.
4. `sercswarm.ron`: turns in enum form, `opencode` agent, mount `~/.local/share/opencode/auth.json:/home/dev/.local/share/opencode/auth.json`. Bind the file, never the directory: the directory holds session storage and two slots would write one index.
5. `Containerfile.swarm`: `npm install -g opencode-ai`.
6. Symlink `.opencode/commands` to `.claude/commands`. Formats match: frontmatter plus `$ARGUMENTS` body (`.claude/commands/execute.md:1-5`). Single source, no copies.
7. `docs/spec/contract.md`: rewrite the `agents.*` and `stages[].turns` bullets, add the limits below. `docs/refs/architecture.md:33`: new argv order and parse rule.
8. Tests: `config.rs` parses the two-agent example and names the bad field; `turn.rs` parses a one-object reply and a five-event JSONL reply. Nothing else; `stage.rs` and `pool.rs` need real podman.
9. Verify: `cargo build`, `cargo test`, `make lint`, then a live board with one claude row and one opencode row.

## Limits

- Prompt rides as the last argv token. A harness that reads the prompt from stdin only does not fit. Add an `input` field when such a harness turns up.
- Harness with no session resume: leave `resume` absent, every turn opens a fresh session, stage context is lost between turns.
- Result must reach stdout. A harness that writes the final message to a file only does not fit.
- opencode wraps a space-holding argv in literal quotes (`run.ts`: `arg.includes(" ") ? '"' + … + '"'`), so the model sees the prompt quoted.
- opencode emits one `text` part per text block. Text before a tool call in the final turn drops out of a commit message, since result takes the last hit.
- Command frontmatter keys differ per harness (`allowed-tools` against `agent`). Each ignores the other's.

## Notes

Unverified, check on first run:

- codex accepts `codex exec --json … resume <id> <prompt>`, meaning `exec` flags before the `resume` sub-subcommand.
- opencode reasoning parts carry no `part.text`. `--thinking` is off by default, so expected absent; if present they pollute `result`.
- opencode `--auto` exists on the installed version. Pin the npm version in the image if it does not.
- opencode auth in the container: `auth.json` mount covers OAuth providers, `env` covers key-only providers. Neither tested headless here.
