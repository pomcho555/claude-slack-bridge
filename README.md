<p align="center">
  <img src="assets/icon.png" alt="Slack ↔ Claude Code bridge" width="160">
</p>

# Slack ↔ Claude Code bridge

[![e2e](https://github.com/pomcho555/claude-slack-bridge/actions/workflows/e2e.yml/badge.svg)](https://github.com/pomcho555/claude-slack-bridge/actions/workflows/e2e.yml)
[![security](https://github.com/pomcho555/claude-slack-bridge/actions/workflows/security.yml/badge.svg)](https://github.com/pomcho555/claude-slack-bridge/actions/workflows/security.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/pomcho555/claude-slack-bridge/badge)](https://securityscorecards.dev/viewer/?uri=github.com/pomcho555/claude-slack-bridge)

> ## ⚠️ USE AT YOUR OWN RISK
>
> **This software is provided "AS IS", without warranty of any kind. The
> author(s) accept NO LIABILITY and take NO RESPONSIBILITY WHATSOEVER for any
> damage, data loss, security breach, unauthorized access, financial cost, or
> any other consequence arising from its use.**
>
> This tool runs Claude Code with tool access on your machine and handles live
> Slack tokens — **anyone allowed to trigger the bot can execute code on your
> host.** Securing your deployment (tokens, `ALLOWED_USERS`, permission mode,
> network exposure) is **entirely your own responsibility.** You assume all
> risk by using it.

Run long Claude Code jobs from Slack and reply to them from your phone.

A mention starts a job; the bot runs `claude` headless on your machine and posts
the result back in the thread. **Replying in that thread continues the same
Claude session** (`--resume`), so you can keep the conversation going from your
iPhone while you're AFK.

```
iPhone (Slack) ──Socket Mode──► bridge (this app, on your home server) ──► claude CLI
        ▲                                                                      │
        └───────────────── result / reply posted to the thread ◄──────────────┘
```

One Slack thread = one Claude session. No public URL needed (Socket Mode uses an
outbound connection), so it runs fine from a home server.

Implemented in Rust (Socket Mode via [`slack-morphism`](https://crates.io/crates/slack-morphism)).

## Setup

### 1. Create the Slack app

Go to <https://api.slack.com/apps> → **Create New App** → **From an app manifest**,
pick your workspace, and paste:

```yaml
display_information:
  name: Claude Code Bridge
features:
  bot_user:
    display_name: claude
    always_online: true
oauth_config:
  scopes:
    bot:
      - app_mentions:read
      - channels:history
      - groups:history
      - im:history
      - chat:write
      - files:write
settings:
  event_subscriptions:
    bot_events:
      - app_mention
      - message.channels
      - message.groups
      - message.im
  socket_mode_enabled: true
```

Then:

- **Install App** → copy the **Bot User OAuth Token** (`xoxb-…`) → `SLACK_BOT_TOKEN`.
- **Basic Information → App-Level Tokens** → generate one with the
  `connections:write` scope → copy (`xapp-…`) → `SLACK_APP_TOKEN`.
- Invite the bot to a channel: `/invite @claude`.

### 2. Configure

```bash
cp .env.example .env
# edit .env: paste the two tokens, set ALLOWED_USERS to your Slack user ID,
# and point CLAUDE_WORKDIR at the repo you want Claude to work in.
```

`.env` is loaded automatically from the working directory (real env vars win).

> ⚠️ Anyone allowed to trigger the bot can run Claude Code on your machine with
> the configured permission mode. Set `ALLOWED_USERS`.

### 3. Build & run

```bash
cargo build --release
cargo run --release            # runs the bridge (default binary)
```

Leave it running on your home server while jobs execute.

## Usage

In a channel the bot is in:

```
@claude refactor the auth module and run the tests
```

It replies `🛠 Started…`, runs the job (minutes or hours), then posts `✅ Done`
with the result. To continue, just reply in that thread:

```
the tests still fail on login — look at session expiry
```

That resumes the same Claude session with full context. Long results are
attached as a file.

## Outbound: get pinged on Slack when a *local* job finishes

Often you start a long job from your terminal and just want to be notified — and
able to reply — when it finishes hours later. These binaries post a result to
Slack **and** seed the thread with the Claude session, so replying in that thread
(mention the bot) continues the same session via `--resume`.

Set a default target channel first (the bot must be a member):

```bash
# in .env
SLACK_NOTIFY_CHANNEL=C0123ABCD     # or pass --channel per call
```

### `slack-claude-job` — run a job and push its result

```bash
cargo run --release --bin slack-claude-job -- "refactor the auth module and run the tests"
```

Runs the prompt headless (same runner the bridge uses) and, when done, posts the
result to Slack and seeds the thread. Options: `--channel`, `--workdir`, `--title`.

### `slack-claude-notify` — push text for an existing session

```bash
echo "done — all green" | cargo run --release --bin slack-claude-notify -- --session <session-id>
```

Text comes from `--text`, `--file`, or stdin.

### Auto-notify any session with a Stop hook

To get the same notification from an *ordinary* `claude` run, wire
`slack-claude-stop-hook` into Claude Code's **Stop** hook. It reads the session
id + transcript from the hook payload, extracts this turn's final message
(polling past the transcript-flush race), and pushes it to Slack.

It is **opt-in**: it does nothing unless `CLAUDE_SLACK_NOTIFY` is truthy, so
ordinary interactive sessions stay quiet. After `cargo build --release`, add to
`~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "cd /path/to/slack-test && ./target/release/slack-claude-stop-hook"
          }
        ]
      }
    ]
  }
}
```

Then run the jobs you care about with the flag set:

```bash
CLAUDE_SLACK_NOTIFY=1 claude -p "run the full migration and verify"
```

The hook always exits 0 — a failure there never breaks your Claude session. It
must run from the bridge directory (so it loads the same `.env` and `bridge.db`).

## Handoff model — read this

The terminal `claude` (the interactive TUI) and Slack are **separate OS
processes**, and the bridge connects them as a *handoff*, not a live two-way
mirror:

- The Stop hook pushes a **snapshot** of the last assistant message at the end
  of a turn — it does not stream the live terminal conversation.
- A Slack reply runs `claude --resume` as a **fresh headless process**; it
  cannot inject input into a running TUI, so it never shows up in the terminal.
- The interactive TUI holds its conversation in memory and never re-reads the
  transcript, so it cannot see what the Slack side appended.

**Rule: once a job has been handed off to Slack, continue only in that Slack
thread — stop typing in the terminal session.** Driving the same Claude session
from both forks it (two heads writing one transcript); the bridge cannot prevent
this across processes. Repeated Stop-hook pushes for the *same* session are
collapsed into the one existing thread, but a real fork (terminal + Slack both
active) is still a fork.

## How it works

The core is transport-agnostic — it writes through the `Poster` / `SlackClient`
seams — which is what makes it testable without a live Slack.

- **`config.rs`** — loads config from the environment (`.env` via `dotenvy`).
- **`store.rs`** — SQLite map of `thread_ts → session_id` (plus a reverse lookup)
  so a thread survives restarts and reconnects to its Claude session.
- **`claude_runner.rs`** — invokes `claude --print --output-format json` (adds
  `--resume <id>` to continue), parses the session id and result; enforces a
  subprocess timeout.
- **`app.rs`** — the inbound core: `handle_mention` / `handle_message` routing,
  `post_result`, and `ThreadWorkers` (one OS thread per Slack thread, so a reply
  mid-job queues behind it instead of resuming concurrently).
- **`notify.rs`** — `push_result`: posts a result and seeds `thread_ts →
  session_id`; reuses the existing thread for a session via `find_by_session`.
- **`stop_hook.rs`** — transcript parsing + the Stop-hook orchestration.
- **`slack.rs`** — `RealSlack`: the slack-morphism-backed implementation of the
  `Poster` and `SlackClient` seams.
- **`bin/`** — the four binaries (`slack-claude-bridge`, `-job`, `-notify`,
  `-stop-hook`) plus the `fake_claude` test fixture.

slack-morphism runs the async (tokio) Socket Mode receive loop; each event is
dispatched to the synchronous core on a plain OS thread (off the runtime), so the
blocking `claude` subprocess and outbound Slack calls never block a tokio worker.

## Testing

```bash
cargo test
```

The suite is **end-to-end against the real seams** — a real `ClaudeRunner`
subprocess (driven by the `fake_claude` test bin, a stand-in for the `claude`
CLI) and the real SQLite store — with only Slack faked. It is driven by
[`spec/scenarios.json`](spec/scenarios.json), a declarative behavioral contract
run by [`tests/spec.rs`](tests/spec.rs); native tests cover the language-specific
paths the spec excludes (runner timeout / missing-binary / non-JSON in
[`tests/runner.rs`](tests/runner.rs), the transcript-flush race in
[`tests/stop_hook_timing.rs`](tests/stop_hook_timing.rs)).

CI also gates on `cargo fmt --check`, `cargo clippy -D warnings`, and
`cargo audit`; a separate [security workflow](.github/workflows/security.yml)
runs cargo-deny, Trivy (SCA), Syft (SBOM), and TruffleHog.

## Notes & limitations

- Per-thread worker threads are kept for the process lifetime — fine for personal
  use; for many threads you'd want to reap idle ones.
- Set `CLAUDE_PERMISSION_MODE=bypassPermissions` only if you fully trust the
  allowed users — it lets Claude run tools without prompting.
- `CLAUDE_TIMEOUT` (default 4h) kills runaway jobs.
