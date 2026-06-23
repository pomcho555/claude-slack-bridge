# Contributing

Thanks for your interest in the Slack ↔ Claude Code bridge. This document covers
building from source, the internal architecture, and how to run the tests. For
installation and usage, see the [README](README.md).

## Build from source

```bash
git clone https://github.com/pomcho555/claude-slack-bridge
cd claude-slack-bridge
cargo build --release
```

The binaries land in `target/release/`. To run the bridge straight from the
checkout without installing:

```bash
cargo run --release                                  # the bridge (default binary)
cargo run --release --bin slack-claude-job -- "…"    # a one-off job
```

Configuration is identical to an installed build: a `.env` in the working
directory (see [`.env.example`](.env.example)).

A [`Makefile`](Makefile) wraps the common tasks — run `make help` to list them.
Useful shortcuts: `make check` (format + lint + test, the fast pre-commit gate)
and `make ci` (everything CI runs).

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

## CI

CI gates on `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo audit`.
A separate [security workflow](.github/workflows/security.yml) runs cargo-deny,
Trivy (SCA), Syft (SBOM), and TruffleHog. Please run `cargo fmt` and
`cargo clippy` locally before opening a pull request.
