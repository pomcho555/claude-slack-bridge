# Rust port (in progress)

A Rust rewrite of the Python `v0` bridge. The goal: pass the **same**
language-neutral spec, [`../spec/scenarios.json`](../spec/scenarios.json), that
the Python reference (`../tests/test_spec.py`) passes.

## Status — spec is GREEN; Slack wiring remains

The behavior layer is fully ported and passes all 13 shared scenarios plus the
language-specific native tests (runner timeout / missing-binary / non-JSON, and
the stop-hook flush race). What's left is the Slack transport (`main`).

| layer | module | state |
|---|---|---|
| config (env) | `src/config.rs` | ✅ implemented |
| thread→session store (SQLite) | `src/store.rs` | ✅ implemented |
| `claude` runner (subprocess + JSON + timeout) | `src/claude_runner.rs` | ✅ implemented |
| outbound seams + helpers | `src/app.rs` (`Poster`, `Workers`, `post_result`, `run_new_job`, `run_reply`, `strip_mentions`, `is_allowed`) | ✅ implemented |
| inbound routing | `src/app.rs` (`handle_mention`, `handle_message`) | ✅ implemented |
| outbound push | `src/notify.rs` (`push_result`) | ✅ implemented |
| transcript parsing + flush-race poll | `src/stop_hook.rs` (`assistant_text`, `is_human_turn`, `answer_for_last_turn`, `final_message`) | ✅ implemented |
| stop-hook orchestration | `src/stop_hook.rs` (`run`) | ✅ implemented |
| **Slack Socket Mode wiring / `main`** | `src/bin/bridge.rs` | ⛔ **not started** (see below) |

### Remaining: Slack transport (a design decision)

The core is transport-agnostic — it writes through the `Poster` /
`SlackClient` seams. `main` still needs a real Socket Mode client wired to those
seams. Rust has no official Slack Bolt; the realistic options are
[`slack-morphism`](https://crates.io/crates/slack-morphism) (Socket Mode +
async, pulls in `tokio`/`hyper`) or a hand-rolled `apps.connections.open` +
WebSocket client. Picking one is the open design choice, deferred so it can be
made deliberately rather than by default.

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
