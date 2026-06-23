//! Test fixture: a stand-in for the real `claude` CLI, used by the spec harness
//! (`tests/spec.rs`) and the native runner tests. Port of the former
//! `tests/fake_claude.py` — keeping the bridge's tests free of any Python.
//!
//! It mimics `claude --print --output-format json` closely enough to exercise
//! the whole bridge without a real model. Behaviour is driven entirely by env
//! vars so a single test can shape one invocation:
//!
//!   FAKE_CLAUDE_LOG      append-only JSONL of each invocation (resume/prompt/argv)
//!   FAKE_CLAUDE_SESSION  force the returned session_id
//!   FAKE_CLAUDE_RESULT   force the returned result text (default: "echo: <prompt>")
//!   FAKE_CLAUDE_ERROR    "1" -> is_error: true
//!   FAKE_CLAUDE_SLEEP    seconds to sleep before answering (to trigger timeouts)
//!   FAKE_CLAUDE_RAW      emit this raw string verbatim instead of JSON
//!
//! Tests locate this binary via `env!("CARGO_BIN_EXE_fake_claude")`.

use std::io::Write;

use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut resume: Option<String> = None;
    for i in 0..args.len() {
        if args[i] == "--resume" && i + 1 < args.len() {
            resume = Some(args[i + 1].clone());
        }
    }
    let prompt = args.last().cloned().unwrap_or_default();

    if let Ok(log) = std::env::var("FAKE_CLAUDE_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            let _ = writeln!(
                f,
                "{}",
                json!({"resume": resume, "prompt": prompt, "argv": args})
            );
        }
    }

    if let Ok(sleep) = std::env::var("FAKE_CLAUDE_SLEEP") {
        if let Ok(secs) = sleep.parse::<f64>() {
            if secs > 0.0 {
                std::thread::sleep(std::time::Duration::from_secs_f64(secs));
            }
        }
    }

    if let Ok(raw) = std::env::var("FAKE_CLAUDE_RAW") {
        print!("{raw}");
        return;
    }

    let session = std::env::var("FAKE_CLAUDE_SESSION")
        .ok()
        .or(resume)
        .unwrap_or_else(|| "sess-new".to_string());
    let result = std::env::var("FAKE_CLAUDE_RESULT").unwrap_or_else(|_| format!("echo: {prompt}"));
    let is_error = std::env::var("FAKE_CLAUDE_ERROR")
        .map(|v| v == "1")
        .unwrap_or(false);

    println!(
        "{}",
        json!({"session_id": session, "result": result, "is_error": is_error})
    );
}
