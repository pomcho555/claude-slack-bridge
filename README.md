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

## Why this

- **Drive Claude Code from your phone** — start, monitor, and continue long jobs from Slack while you're AFK.
- **Threads are sessions** — replying in a thread resumes the same Claude session (`--resume`) with full context.
- **No public URL** — Socket Mode dials out, so it runs safely from a home server behind NAT.
- **Single static binary** — one `cargo install`, no runtime, no daemon manager; pure Rust on tokio.
- **Allow-list gated** — only `ALLOWED_USERS` can trigger jobs on your host, with a configurable permission mode.
- **Notify *any* terminal job** — a Stop hook pings Slack when an ordinary local `claude` run finishes.
- **Hardened CI** — fmt/clippy/audit plus cargo-deny, Trivy, Syft (SBOM), and TruffleHog on every change.

## Install

```sh
cargo install slack-claude-bridge
```

This installs four binaries on your `PATH`:

| Binary | What it does |
| --- | --- |
| `slack-claude-bridge` | the bridge — listens on Slack and runs jobs (main entry point) |
| `slack-claude-job` | run a local job and push its result to Slack |
| `slack-claude-notify` | push text into a Slack thread for an existing session |
| `slack-claude-stop-hook` | Claude Code Stop hook → auto-notify any session |

**Prerequisites:** the [`claude`](https://docs.claude.com/en/docs/claude-code)
CLI on your `PATH`, and a Rust toolchain to run `cargo install` (see
[rustup.rs](https://rustup.rs)).

Prefer to build from source? See [CONTRIBUTING.md](CONTRIBUTING.md).

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

Create a working directory for the bridge and add a `.env` there
([`.env.example`](.env.example) lists every option):

```bash
# .env
SLACK_BOT_TOKEN=xoxb-…          # from "Install App"
SLACK_APP_TOKEN=xapp-…          # the connections:write app-level token
ALLOWED_USERS=U0123ABCD         # your Slack user ID(s), comma-separated
CLAUDE_WORKDIR=/path/to/repo    # the repo you want Claude to work in
```

`.env` is loaded automatically from the working directory (real env vars win).

> ⚠️ Anyone allowed to trigger the bot can run Claude Code on your machine with
> the configured permission mode. Set `ALLOWED_USERS`.

### 3. Run

From that directory:

```bash
slack-claude-bridge
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
slack-claude-job "refactor the auth module and run the tests"
```

Runs the prompt headless (same runner the bridge uses) and, when done, posts the
result to Slack and seeds the thread. Options: `--channel`, `--workdir`, `--title`.

### `slack-claude-notify` — push text for an existing session

```bash
echo "done — all green" | slack-claude-notify --session <session-id>
```

Text comes from `--text`, `--file`, or stdin.

### Auto-notify any session with a Stop hook

To get the same notification from an *ordinary* `claude` run, wire
`slack-claude-stop-hook` into Claude Code's **Stop** hook. It reads the session
id + transcript from the hook payload, extracts this turn's final message
(polling past the transcript-flush race), and pushes it to Slack.

It is **opt-in**: it does nothing unless `CLAUDE_SLACK_NOTIFY` is truthy, so
ordinary interactive sessions stay quiet. Add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "cd /path/to/bridge-dir && slack-claude-stop-hook"
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

## Notes & limitations

- Per-thread worker threads are kept for the process lifetime — fine for personal
  use; for many threads you'd want to reap idle ones.
- Set `CLAUDE_PERMISSION_MODE=bypassPermissions` only if you fully trust the
  allowed users — it lets Claude run tools without prompting.
- `CLAUDE_TIMEOUT` (default 4h) kills runaway jobs.

## Contributing

Bug reports, architecture notes, and the development/test workflow live in
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE).
