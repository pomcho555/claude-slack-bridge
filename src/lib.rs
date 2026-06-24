//! Rust port of the Slack <-> Claude Code bridge.
//!
//! The behavioral contract lives in `../spec/scenarios.json` and is exercised
//! by `tests/spec.rs`, which runs the SAME scenarios as the Python reference
//! (`tests/test_spec.py`) through an equivalent harness.
//!
//! Status: SKELETON. The data plumbing (config / store / claude_runner) is
//! implemented; the behavior layer (app handlers, notify, stop-hook
//! orchestration) is stubbed with `unimplemented!()`, so the spec suite runs
//! every scenario RED. Filling in those stubs until the suite is green is the
//! port.

// Skeleton stage: stubbed functions leave args/imports unused. Drop this as the
// behavior layer gets implemented.
#![allow(dead_code, unused_variables, unused_imports)]

pub mod app;
pub mod claude_runner;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod notify;
pub mod slack;
pub mod stop_hook;
pub mod store;
