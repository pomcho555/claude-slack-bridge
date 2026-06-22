# Slack ↔ Claude Code bridge

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

> ⚠️ Anyone allowed to trigger the bot can run Claude Code on your machine with
> the configured permission mode. Set `ALLOWED_USERS`.

### 3. Run

```bash
uv sync
uv run slack-claude-bridge
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

The flow above is Slack-first (a mention starts the job). But often you start a
long job from your terminal and just want to be notified — and able to reply —
when it finishes hours later. Two outbound entry points cover that. Both post
the result to Slack **and** seed the thread with the Claude session, so replying
in that thread (mention the bot) continues the same session via `--resume`.

Set a default target channel first (the bot must be a member):

```bash
# in .env
SLACK_NOTIFY_CHANNEL=C0123ABCD     # or pass --channel per call
```

### `slack-claude-job` — run a job and push its result

```bash
uv run slack-claude-job "refactor the auth module and run the tests"
```

Runs the prompt headless (same runner the bridge uses) and, when done, posts
the result to Slack and seeds the thread. Options: `--channel`, `--workdir`,
`--title`.

### `slack-claude-notify` — push text for an existing session

Lower-level: post arbitrary text for a session id you already have (text from
`--text`, `--file`, or stdin):

```bash
echo "done — all green" | uv run slack-claude-notify --session <session-id>
```

### Auto-notify any session with a Stop hook

To get the same notification from an *ordinary* `claude` run (without going
through `slack-claude-job`), wire `slack-claude-stop-hook` into Claude Code's
**Stop** hook. It reads the session id + transcript from the hook payload,
extracts the final message, and pushes it to Slack.

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
            "command": "cd /path/to/slack-test && .venv/bin/slack-claude-stop-hook"
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
must run from the bridge directory (so it loads the same `.env` and `bridge.db`
the bridge uses).

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

So the intended flow is one-directional in time:

```
terminal: start long job ──(finishes, Stop hook fires once)──► Slack: ✅ one thread
                                                                        │ reply = --resume
                                                                        ▼
                                                              Slack: continue (from here on)
```

**Rule: once a job has been handed off to Slack, continue only in that Slack
thread — stop typing in the terminal session.** Driving the same Claude session
from both the terminal and Slack forks it (two heads writing one transcript);
the bridge cannot prevent this across processes. Repeated Stop-hook pushes for
the *same* session are now collapsed into the one existing thread, but a real
fork (terminal + Slack both active) is still a fork.

## How it works

- **`config.py`** — loads `.env` via `python-dotenv`.
- **`store.py`** — SQLite map of `thread_ts → session_id` so a thread survives
  restarts and reconnects to its Claude session.
- **`claude_runner.py`** — invokes `claude --print --output-format json` (adds
  `--resume <id>` to continue), parses the session id and result.
- **`app.py`** — Slack Bolt (Socket Mode) handlers. Each thread gets a
  single-worker executor, so a reply that arrives mid-job queues behind it
  instead of resuming the session concurrently.
- **`notify.py`** — outbound counterpart: posts a result to Slack and seeds
  `thread_ts → session_id` so a reply resumes it. If the session is already
  mapped to a thread (a `store.find_by_session` reverse lookup), it posts into
  that thread instead of a new root, so one session stays one thread.
- **`job.py`** — the `slack-claude-job` / `slack-claude-notify` CLIs.
- **`stop_hook.py`** — the `slack-claude-stop-hook` Stop-hook entry point.

## Notes & limitations

- Per-thread executors are kept for the process lifetime — fine for personal
  use; for many threads you'd want to reap idle ones.
- Set `CLAUDE_PERMISSION_MODE=bypassPermissions` only if you fully trust the
  allowed users — it lets Claude run tools without prompting.
- `CLAUDE_TIMEOUT` (default 4h) kills runaway jobs.
