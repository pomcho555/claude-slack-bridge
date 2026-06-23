# Behavioral spec (`scenarios.json`)

`scenarios.json` is a **declarative, data-driven** description of how the bridge
must behave: given an input (a Slack event, or a finished Claude session), what
Slack must see. The implementation is validated against this contract — every
scenario is run through a harness and the same output is asserted.

The harness is [`tests/spec.rs`](../tests/spec.rs). The schema is deliberately
language-neutral, so the same `scenarios.json` could validate a port in another
language via an equivalent harness.

## The fake `claude`

The `fake_claude` test binary (`src/bin/fake_claude.rs`) stands in for the real
CLI; the harness points `CLAUDE_BIN` at it (via `CARGO_BIN_EXE_fake_claude`) and
spawns it as a subprocess. It is driven by env vars: `FAKE_CLAUDE_SESSION`,
`FAKE_CLAUDE_RESULT`, `FAKE_CLAUDE_ERROR`, `FAKE_CLAUDE_SLEEP`, `FAKE_CLAUDE_RAW`,
and `FAKE_CLAUDE_LOG` (append-only JSONL recording each invocation's `resume`
id, `prompt`, and full `argv`).

## Schema

### `inbound[]` — Slack event → bridge → Slack

| field | meaning |
|---|---|
| `config.allowed_users` | allow-list; empty = everyone allowed |
| `claude` | `{session, result}` or `{result_len}` (forces fake output); `{error:true}` |
| `seed[]` | pre-existing `{thread_ts, channel, session_id}` thread→session rows |
| `events[]` | `{type: app_mention\|message, ...}` — the rest is the Slack event verbatim (`user`/`bot_id`, `ts`, `thread_ts?`, `channel`, `text`) |
| `expect.posts[]` | ordered, exact-count match of outbound messages: `{channel?, thread_ts?, text_contains?, text_not_contains?}` |
| `expect.post_any_contains[]` | substrings that must appear in *some* post |
| `expect.uploads[]` | `{filename?, min_content_len?}` |
| `expect.claude_invocations[]` | per-invocation `{resume, prompt_contains?}` read from `FAKE_CLAUDE_LOG` |
| `expect.store` | `{thread_ts: {session_id}}` final thread→session mapping |
| `expect.no_claude` | `true` ⇒ the fake CLI must never be invoked |

### `stop_hook[]` — finished local session → Slack

| field | meaning |
|---|---|
| `notify_channel` | `SLACK_NOTIFY_CHANNEL` |
| `hook` | `{session_id, notify_flag}` (`notify_flag` = `CLAUDE_SLACK_NOTIFY`; `"0"` ⇒ stay silent) |
| `seed[]` | pre-existing thread→session rows (for the one-session-one-thread case) |
| `transcript[]` | mini-DSL, one entry per JSONL line (see below) |
| `expect.posts[]` / `expect.uploads[]` | same matchers as inbound |

Transcript DSL → real transcript JSONL shapes:

| entry | becomes |
|---|---|
| `{"u": "text"}` | a human user turn |
| `{"tr": true}` | a `tool_result` user entry (plumbing, not a human turn) |
| `{"think": true}` | assistant `thinking` block (no text) |
| `{"tool": "Bash"}` | assistant `tool_use` block (no text) |
| `{"a": "text"}` | assistant `text` block |
| `{"a_len": N}` | assistant `text` block of length N |

## Harness checklist

1. Point `CLAUDE_BIN` at the `fake_claude` binary; honor `FAKE_CLAUDE_*`.
2. Load `scenarios.json`; for each `inbound` scenario seed the store, feed the
   events, capture outbound Slack calls + the invocation log, assert `expect`.
3. For each `stop_hook` scenario, build the transcript from the DSL, set the
   env, run the Stop-hook entry point, assert `expect`.

## Out of scope (covered by native tests)

Timing/race behavior — the transcript-flush poll in the Stop hook — is verified
imperatively in [`tests/stop_hook_timing.rs`](../tests/stop_hook_timing.rs)
rather than declaratively, since simulating a delayed flush is harness-specific.
Likewise the runner's timeout / missing-binary / non-JSON handling in
[`tests/runner.rs`](../tests/runner.rs).
