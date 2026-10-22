# Harness adapters per agent CLI

**What.** Agent gains `command` and `reply` templates, a prompt step becomes text or command. Any agent harness runs from `scswarm.yaml` with no code change.

**Why.** Tool fits one harness today.

- `call.rs:255` `parse()` reads one JSON object, keys `session_id` and `result`. opencode `--format json` prints JSONL events (`sessionID` per event, text in `part.text`), so session never parses and every turn opens a fresh session. Row context dies between steps.
- `call.rs:133` `Turn::argv()` builds `sh -c '<exec> "$@"'` with the prompt last. opencode `run.ts` has no slash parsing: a command needs `--command <name>` with the message as arguments. So `/execute {task}` arrives as chat text and `.claude/commands/execute.md` never loads.
- `script.rs` takes the agent commit reply as prose; `call.rs:187` `Message::reply()` rejects a blank one alone. Subject = "```" on 29 of 34 stage commits across two boards; commit `483038d` carries agent-written `Swarm-Task`. See [#7](https://github.com/softcery/scswarm/issues/7).
- `call.rs:238` `judge()` reads exit code alone. Account usage limit exits 1, three tries in 6 s mark the row failed, row idle 4 h 55 min. See [#6](https://github.com/softcery/scswarm/issues/6).

**Done when.** One board runs rows with `agent` `claude` and `agent` `opencode` at once, each lands its task, and `scswarm log ID` shows one session per row for both plus a commit message with no event noise. `scswarm.yaml` carries a third agent, `codex`, and `cargo test` parses a captured codex JSONL reply to one session and one message. `grep -ri 'claude\|opencode\|codex' src/` returns nothing. A live codex row belongs to [RFC 0012](0012-codex-live-row.md): account quota on this workstation resets 2026-09-22.

## Shape

`config.rs`, sole reader of the project file:

```yaml
agents:
  claude:
    exec: claude --dangerously-skip-permissions -p --output-format json
    resume: "--resume {session}"
    command:
      text: "/{name} {args}"
    reply:
      session:
        at: /session_id
      result:
        at: /result
    commit:
      args: "--json-schema {schema}"
      reply: /structured_output
    unavailable:
      at: /api_error_status
      text:
        is: "429"
  opencode:
    exec: opencode run --format json --auto
    resume: "--session {session}"
    command:
      argv: "--command {name}"
    reply:
      session:
        at: /sessionID
      result:
        when:
          at: /part/type
          text:
            is: text
        at: /part/text
  codex:
    exec: >-
      codex login status >/dev/null 2>&1 ||
      printenv CODEX_ACCESS_TOKEN | codex login --with-access-token >/dev/null;
      codex exec --json --dangerously-bypass-approvals-and-sandbox
    resume: "resume {session}"
    command:
      text: "Read .claude/commands/{name}.md, follow it. Arguments: {args}"
    reply:
      session:
        at: /thread_id
      result:
        when:
          at: /item/item_type
          text:
            is: agent_message
        at: /item/text
    unavailable:
      at: /message
      text:
        has: usage limit
```

- Prompt step is text or `command`, a new step key beside `prompt`. The agent commit step sends config field `commit`, so `src/` holds no prompt text.
- `command.text` puts name and args in the message. `command.argv` puts the name in argv, args stay the message. A harness with no command system uses `text: "Read .claude/commands/{name}.md, follow it. Arguments: {args}"`, so the model opens the file itself. Three placements cover every harness.
- `Match { at, text }`, `text` is `is` or `has`. `at` is an RFC 6901 pointer, `serde_json::Value::pointer` reads it, no new dependency. `is` compares the scalar's text form whole, `has` asks substring. Claude `429` is a number, opencode `text` and codex `agent_message` are exact strings, codex states its limit in prose with a reset date, so both forms carry work.
- `Field { when: Option<Match>, at: String }`. A line counts when `when` is absent or matches; `at` reads the value out of that line. One picker, one reader, three uses: `result`, `unavailable`, and the per-line problem [RFC 0011](0011-status-context.md) names.
- `reply: (session: Field, result: Field)`. Session takes the first counting line, result the last. Pick rule stays in code until a harness needs another.
- `commit: Option<CommitReply { args: String, reply: String }>`. `{schema}` = tool-owned JSON schema `{ subject: string, body: string }`, rendered by `serde_json::to_string`, compact, no whitespace, so it stays one argv token through the whitespace split. `reply` points at the object. Absent: host takes `result` whole as message; harness with no structured output, stated limit.
- `unavailable: Option<Match>`, tested on non-zero exits alone. A matching line is an environment failure, not a step failure. Both limits verified non-zero: codex exit 1 on 2026-09-16, claude exit 1 in [#6](https://github.com/softcery/scswarm/issues/6). Testing successful replies too would let an agent that writes the words "usage limit" into a commit message hold its slot forever. Absent: every non-zero exit is a row failure, as today.

Commit message, host-built from the object: subject, blank line, body. Boundary check, failed check = failed try under the cap: subject one line, non-empty. The host writes no trailer, so the `swarm-` key space needs no guard.

Unavailable, wait in place:

- `call.rs`: reply matches `unavailable` = sleep `hold` seconds from the host file, same step again. No try counted. Transcript entry holds the reply, event `hold` names the elapsed wait.
- Session, worktree, step position stay. A limit in step 4 of 5 costs nothing when it lifts.
- Other slots: `serve` keeps assigning. A new row hits the limit at its first agent step and waits the same way. Cost: one call per slot per `hold`.
- `status`: a slot whose last transcript entry is a hold reads `held`, not `running`.
- Reset time in the reply is prose, "resets 7am (UTC)". Not parsed. Fixed interval.
- Aborting to the pool instead loses the row in flight: replay from base, session dies. Rejected.

`call.rs`:

- Argv order: `exec` tokens, command argv, resume argv, prompt. Prompt stays one argv token, so agent text never reaches the shell.
- `parse()`: read stdout line by line, skip lines that hold no JSON, apply the fields. Session = first counting line, result = last.
- `reply` absent = plain-text harness: result is the whole stdout trimmed, as today. `reply.result` set and no line counts = failed try under the cap, never a fallback. A wrong pointer otherwise passes every commit check of this task: line 1 of a codex stream is `{"type":"thread.started","thread_id":"…"}`, one line, non-empty, no fence, so a JSON object lands as a commit subject.
- Template substitution reuses `Turn::argv()`: split on whitespace, replace the slot per token. `resume` uses it for `{session}`, `Command::Argv` for `{name}`.

`script.rs`: renders a prompt step into prompt text plus command argv. A text step goes as written. A command step renders through the agent's `command`. The agent commit step sends the config text, then the host commits with the reply.

Checks in `config.rs`:

- Every `at` starts with `/`. Error names the field: `reply.session.at`, `reply.result.at`, `reply.result.when.at`, `unavailable.at`.
- `Is` and `Has` text non-empty. Error names the field.
- `resume` present requires `reply.session`; without an id there is nothing to resume with.
- A command step on any row requires every agent to define `command`. Rows pick the agent per row, so any agent can run any row.
- `commit.reply` starts with `/`. Error names the field.
- Host file `hold` must be present when any agent sets `unavailable`. Error names both fields.
- Rendered prompt trimmed, so `text: "/{name} {args}"` with empty args leaves no trailing space.

## Rules

- Tool learns no harness name and no dialect. Every dialect lives in `scswarm.yaml`.
- Keep no back-compat. The `{commit}` turn form is gone already.
- No new dependency.

## Steps

1. `config.rs`: add `Match`, `Text`, `Field`, `Command`; extend `Agent`; write the checks; update the module test. `step.rs`: prompt step gains the `command` form beside plain text.
2. `call.rs`: take command argv, rewrite `parse()` to the pointer rule.
3. `script.rs`: render both prompt forms. The agent commit step adds `commit.args`, reads the object, builds the message. `call.rs` `Fault::Blank` grows into the boundary check.
4. `call.rs`, `host.rs`, `status.rs`, `event.rs`: `unavailable` match and wait, `hold` field, `held` line, `hold` event.
5. `scswarm.yaml`: `opencode` and `codex` agents, mount `~/.local/share/opencode/auth.json:/home/dev/.local/share/opencode/auth.json`. Bind the file, never the directory: the directory holds session storage and two slots would write one index. Codex takes no mount: host file `env` gains `CODEX_ACCESS_TOKEN`, and the `exec` prelude logs in inside the container, one container per slot, so no file is shared.
6. `Containerfile.swarm`: `npm install -g opencode-ai @openai/codex`.
7. Symlink `.opencode/commands` to `.claude/commands`. Formats match: frontmatter plus `$ARGUMENTS` body (`.claude/commands/execute.md:1-5`). Single source, no copies.
8. `docs/spec/contract.md`: rewrite the `agents.*` bullets and the prompt row of the step table, host file `hold`, name `CODEX_ACCESS_TOKEN` beside `ANTHROPIC_API_KEY` in the `env` example, add the limits below. `docs/refs/architecture.md` section Step: new argv order and parse rule.
9. Tests: `config.rs` parses the three-agent example and names the bad field; `call.rs` parses a one-object reply, an opencode JSONL reply, and a codex JSONL reply whose stream holds a `reasoning` item and a `command_execution` item after the `agent_message`, so the `when` filter is what picks the message. Nothing else; `script.rs` and `pool.rs` need real podman.
10. Verify: `cargo build`, `cargo test`, `make lint`, then a live board with one claude row and one opencode row.

## Limits

- Prompt rides as the last argv token. A harness that reads the prompt from stdin only does not fit. Add an `input` field when such a harness turns up.
- Harness with no session resume: leave `resume` absent, every agent step opens a fresh session, row context is lost between steps.
- Result must reach stdout. A harness that writes the final message to a file only does not fit.
- opencode wraps a space-holding argv in literal quotes (`run.ts`: `arg.includes(" ") ? '"' + … + '"'`), so the model sees the prompt quoted.
- opencode emits one `text` part per text block. Text before a tool call in the final turn drops out of a commit message, since result takes the last hit.
- Command frontmatter keys differ per harness (`allowed-tools` against `agent`). Each ignores the other's.
- Harness without structured output: commit message is prose, fence and all. Stated per agent by absent `commit`.
- Codex sets no `commit`. Its flag is `--output-schema <FILE>`, a path, and no argv slot carries a host-written file. Its commit message is prose under the boundary checks, so a fenced reply costs tries.
- Codex access token measured on 2026-09-16: `iat` to `exp` = 240 h, `auth_mode` `chatgpt`. `login --with-access-token` stores no refresh token, and container `env` bakes at `init`, so a token must be replaced and `serve` restarted inside 10 days or every codex row fails.
- Codex `exec` prelude calls `codex login status` per turn and the network only when that check fails.
- Held turn probes every `hold` seconds per slot while the limit stands. Slot stays busy; nothing else can run under an account-wide limit, so no capacity lost.

## Notes

Unverified, check on first run:

- Claude CLI 2.1.263 `--json-schema`: field that carries the object in `--output-format json`. `/structured_output` above is a guess. Read one live reply before writing the pointer.

- codex 0.154.0, read from the binary and one live run on 2026-09-16: events tag on `type`, values `thread.started`, `turn.started`, `turn.completed`, `turn.failed`, `item.started`, `item.updated`, `item.completed`, `error`. Session id is `thread_id` on `thread.started`. Item kinds number 18, `agent_message` among `reasoning`, `command_execution`, `file_change`, `web_search`, `mcp_tool_call`.
- codex item tag key is `item_type` or `type`: unverified, quota spent. [RFC 0012](0012-codex-live-row.md) reads one live stream and corrects `reply.result.when.at` if wrong. The failed-try rule above keeps a wrong key loud.
- `codex exec resume --help` takes `[SESSION_ID] [PROMPT]`, so `resume {session}` lands between the `exec` flags and the prompt, matching the argv order above.
- codex has no slash commands in `exec`: custom prompts are a TUI feature. `Command::Text` names the file, and the worktree is the working directory, so the relative path resolves. No symlink, unlike opencode.
- opencode reasoning parts carry no `part.text`. `--thinking` is off by default, so expected absent; if present they pollute `result`.
- opencode `--auto` exists on the installed version. Pin the npm version in the image if it does not.
- opencode auth in the container: `auth.json` mount covers OAuth providers, `env` covers key-only providers. Neither tested headless here.

