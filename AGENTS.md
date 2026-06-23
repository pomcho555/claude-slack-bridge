# AGENTS.md

Guidance for AI coding agents working in this repository. Humans: see
[README.md](README.md).

## What this is

A Slack ↔ Claude Code bridge, in Rust. A Slack mention starts a headless
`claude` job; the result is posted back in the thread, and replying in that
thread continues the same Claude session via `--resume`. Transport is Slack
Socket Mode via [`slack-morphism`](https://crates.io/crates/slack-morphism).

Single language, single crate at the repo root (there is no Python — it was
removed once the Rust port reached parity).

## Layout

```
src/
  config.rs         env/.env config (dotenvy)
  store.rs          SQLite thread_ts <-> session_id map (+ reverse lookup)
  claude_runner.rs  spawns `claude --print --output-format json`, parses it
  app.rs            inbound core: handle_mention/handle_message, Poster &
                    Workers traits, ThreadWorkers, post_result, run_* helpers
  notify.rs         push_result + the SlackClient trait (outbound seam)
  stop_hook.rs      transcript parsing + Stop-hook orchestration
  slack.rs          RealSlack: slack-morphism impl of Poster + SlackClient
  bin/
    bridge.rs       slack-claude-bridge   (the Socket Mode server, default-run)
    job.rs          slack-claude-job
    notify.rs       slack-claude-notify
    stop_hook.rs    slack-claude-stop-hook
    fake_claude.rs  test fixture: a fake `claude` CLI (see Testing)
tests/              integration tests (spec.rs, runner.rs, stop_hook_timing.rs)
spec/scenarios.json declarative behavioral contract (see below)
```

## Commands (all must pass before you call something done)

```bash
cargo fmt --all                       # format (CI gates on --check)
cargo clippy --all-targets --all-features -- -D warnings
cargo test                            # spec + native tests
cargo build --release
cargo audit                           # advisory scan (CI gate)
cargo deny check advisories bans licenses sources
```

CI mirrors these: `.github/workflows/e2e.yml` (fmt, clippy, build, test,
audit, release) and `.github/workflows/security.yml` (TruffleHog, Trivy SCA,
cargo-deny, Syft SBOM, dependency-review on PRs). Keep all green.

## The behavioral contract — read before changing behavior

`spec/scenarios.json` is the source of truth for observable behavior (inbound
Slack event → outbound Slack calls; Stop-hook → outbound). It is run by
`tests/spec.rs` against the real `ClaudeRunner` + SQLite store, with only Slack
faked. **If you change what the bridge does, update `scenarios.json` in the same
change** — do not weaken the harness to make a diff pass. See
[spec/README.md](spec/README.md) for the schema.

Timing-sensitive behavior that can't be expressed declaratively lives in native
tests: the transcript-flush race in `tests/stop_hook_timing.rs`, and the
runner's timeout / missing-binary / non-JSON handling in `tests/runner.rs`.

The fake `claude` CLI is the `fake_claude` bin (`src/bin/fake_claude.rs`); tests
locate it via `env!("CARGO_BIN_EXE_fake_claude")` and drive it with `FAKE_CLAUDE_*`
env vars. It is a shipped binary so plain `cargo test` works.

## Conventions & gotchas

- **Seams, not direct calls.** The core writes through the `Poster` /
  `SlackClient` traits and submits work through `Workers`. That is what makes it
  testable without live Slack. Add behavior in the core (app/notify/stop_hook),
  keep `slack.rs`/`bin/` as thin transport.
- **Threading.** `RealSlack` bridges sync → slack-morphism async with
  `tokio::runtime::Handle::block_on`, which must only run **off** a runtime
  thread. The bridge dispatches each event to the sync core on a plain OS thread;
  the one-shot CLIs call the core from `main`. Don't call the core (or `post`)
  from inside an async task.
- **One Slack thread = one Claude session.** Preserve the `thread_ts ↔
  session_id` invariant (`store.rs`, and `find_by_session` reuse in `notify.rs`).
- **`.env`** is loaded by `dotenvy` in `Config::load` from the working dir; real
  env vars win. Never commit secrets — `.env` is gitignored and must stay so.
- **Stop hook is opt-in** (`CLAUDE_SLACK_NOTIFY`) and must always exit 0.
- Match existing style; keep `cargo fmt` / `clippy -D warnings` clean.

## Security

- `ALLOWED_USERS` gates who can run Claude on the host. Don't regress the
  allow-list checks (`is_allowed`).
- Never log or commit tokens (`xoxb-`/`xapp-`). Don't set
  `CLAUDE_PERMISSION_MODE=bypassPermissions` as a default.
