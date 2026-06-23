# Rust port (in progress)

A Rust rewrite of the Python `v0` bridge. The goal: pass the **same**
language-neutral spec, [`../spec/scenarios.json`](../spec/scenarios.json), that
the Python reference (`../tests/test_spec.py`) passes.

## Status — fully wired (spec GREEN, Socket Mode implemented)

The behavior layer is fully ported and passes all 13 shared scenarios plus the
language-specific native tests (runner timeout / missing-binary / non-JSON, and
the stop-hook flush race). The Slack transport is wired with
[`slack-morphism`](https://crates.io/crates/slack-morphism) (Socket Mode).

| layer | module | state |
|---|---|---|
| config (env) | `src/config.rs` | ✅ implemented |
| thread→session store (SQLite) | `src/store.rs` | ✅ implemented |
| `claude` runner (subprocess + JSON + timeout) | `src/claude_runner.rs` | ✅ implemented |
| outbound seams + helpers | `src/app.rs` (`Poster`, `Workers`, `ThreadWorkers`, `post_result`, `run_new_job`, `run_reply`, `strip_mentions`, `is_allowed`) | ✅ implemented |
| inbound routing | `src/app.rs` (`handle_mention`, `handle_message`) | ✅ implemented |
| outbound push | `src/notify.rs` (`push_result`) | ✅ implemented |
| transcript parsing + flush-race poll | `src/stop_hook.rs` (`assistant_text`, `is_human_turn`, `answer_for_last_turn`, `final_message`) | ✅ implemented |
| stop-hook orchestration | `src/stop_hook.rs` (`run`) | ✅ implemented |
| shared Slack impl (`Poster` + `SlackClient`) | `src/slack.rs` (`RealSlack`) | ✅ implemented (slack-morphism) |
| Socket Mode bridge | `src/bin/bridge.rs` (`slack-claude-bridge`) | ✅ implemented |
| outbound CLIs | `src/bin/{job,notify,stop_hook}.rs` | ✅ implemented |

### Binaries (parity with the Python console scripts)

| binary | mirrors | purpose |
|---|---|---|
| `slack-claude-bridge` | `app:main` | Socket Mode bridge (mention → job → result; reply → resume) |
| `slack-claude-job` | `job:run_job_main` | run a prompt locally, push result + seed thread |
| `slack-claude-notify` | `job:notify_main` | push text for an existing session id |
| `slack-claude-stop-hook` | `stop_hook:main` | Claude Code Stop-hook (opt-in via `CLAUDE_SLACK_NOTIFY`) |

### Transport design

slack-morphism runs the async (tokio) WebSocket receive loop. Each push event
is converted to the core's `Event` and dispatched to the synchronous core
(`handle_mention` / `handle_message`) on a **plain OS thread**, off the runtime,
so the blocking `claude` subprocess and outbound Slack calls never block a tokio
worker. Outbound calls (`RealSlack`, implementing the `Poster` seam) bridge back
to slack-morphism's async API via `tokio::runtime::Handle::block_on`, only ever
called from non-runtime threads. Long results use the `files_upload_v2` flow
(getUploadURLExternal → upload → completeUploadExternal).

To make jobs runnable on worker threads, `SessionStore` / `ClaudeRunner` /
`RealSlack` are cheap-cloneable shared handles (`Arc` inside) and `ThreadWorkers`
keeps one OS thread per Slack thread (sequential per thread, parallel across).

> Note: the core (everything under `src/`, minus `bin/`) is covered by the spec
> + native tests. The transport binary compiles and is wired but is exercised
> against live Slack manually, not in CI — there is no fake Socket Mode server.

### Run

```bash
cd rust
cp ../.env.example .env   # set SLACK_BOT_TOKEN, SLACK_APP_TOKEN, ALLOWED_USERS, CLAUDE_WORKDIR
cargo run --release
```

## Run the spec

```bash
cd rust
cargo test            # runs every scenario from ../spec/scenarios.json
```

Each scenario prints `PASS`/`FAIL` with its name; the test fails listing every
red scenario. The fake `claude` CLI (`../tests/fake_claude.py`) is shared
verbatim — `tests/spec.rs` just points `CLAUDE_BIN` at it via the runner, the
same way the Python harness does.

## What is NOT in the shared spec (covered by native tests)

Per [`../spec/README.md`](../spec/README.md): the transcript-flush poll timing
(`stop_hook.rs::final_message`) and the runner's timeout / missing-binary
handling are language-specific. These are covered natively:

- `tests/runner.rs` — missing binary, non-JSON output, `--resume` threading,
  subprocess timeout (mirrors `tests/test_claude_runner.py`).
- `tests/stop_hook_timing.rs` — the flush-race wait (mirrors
  `tests/test_stop_hook.py::test_waits_for_flush_then_posts`).

## Security CI

`deny.toml` configures the `cargo-deny` gate (advisories / licenses / bans /
sources) run by `.github/workflows/security.yml`, alongside Trivy (SCA), Syft
(SBOM), TruffleHog (secrets) and dependency-review. Dependabot
(`.github/dependabot.yml`) opens update PRs but is not itself a gate.
