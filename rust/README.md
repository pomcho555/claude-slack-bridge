# Rust port (in progress)

A Rust rewrite of the Python `v0` bridge. The goal: pass the **same**
language-neutral spec, [`../spec/scenarios.json`](../spec/scenarios.json), that
the Python reference (`../tests/test_spec.py`) passes.

## Status — skeleton, spec is RED

| layer | module | state |
|---|---|---|
| config (env) | `src/config.rs` | implemented |
| thread→session store (SQLite) | `src/store.rs` | implemented |
| `claude` runner (subprocess + JSON) | `src/claude_runner.rs` | implemented |
| outbound seams + helpers | `src/app.rs` (`Poster`, `Workers`, `post_result`, `run_new_job`, `run_reply`, `strip_mentions`, `is_allowed`) | implemented |
| transcript parsing | `src/stop_hook.rs` (`assistant_text`, `is_human_turn`, `answer_for_last_turn`, `final_message`) | implemented |
| **inbound routing** | `src/app.rs` (`handle_mention`, `handle_message`) | **stubbed** |
| **outbound push** | `src/notify.rs` (`push_result`) | **stubbed** |
| **stop-hook orchestration** | `src/stop_hook.rs` (`run`) | **stubbed** |
| Slack Bolt wiring / `main` | `src/bin/bridge.rs` | **stubbed** |

The stubs are `unimplemented!()` with a pointer to the Python source to port.
The port is done when `cargo test` is green.

## Run the spec

```bash
cd rust
cargo test            # runs every scenario from ../spec/scenarios.json
```

Each scenario prints `PASS`/`FAIL` with its name; the test fails listing every
red scenario. The fake `claude` CLI (`../tests/fake_claude.py`) is shared
verbatim — `tests/spec.rs` just points `CLAUDE_BIN` at it via the runner, the
same way the Python harness does.

## What is NOT in the shared spec (port these as native tests)

Per [`../spec/README.md`](../spec/README.md): the transcript-flush poll timing
(`stop_hook.rs::final_message`) and the runner's timeout / missing-binary
handling are language-specific. Replicate
`tests/test_stop_hook.py::test_waits_for_flush_then_posts` and
`tests/test_claude_runner.py` as Rust tests when implementing those paths.
