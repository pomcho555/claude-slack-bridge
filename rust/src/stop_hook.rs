//! Claude Code *Stop* hook: when a session finishes, push its final message to
//! Slack and seed the thread (mirror of `stop_hook.py`).
//!
//! Opt-in via `CLAUDE_SLACK_NOTIFY`. The transcript parsing here is shared
//! contract; the flush-race poll timing is language-specific (see spec/README).

use std::fs;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::Config;
use crate::notify::{push_result, SlackClient};
use crate::store::SessionStore;

const FLUSH_WAIT: Duration = Duration::from_secs(5);
const FLUSH_POLL: Duration = Duration::from_millis(250);

/// Concatenated text blocks of one assistant transcript entry ("" if none).
pub fn assistant_text(obj: &Value) -> String {
    if obj.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return String::new();
    }
    match obj.get("message").and_then(|m| m.get("content")) {
        Some(Value::Array(blocks)) => {
            let parts: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty())
                .collect();
            parts.join("\n").trim().to_string()
        }
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// True for a genuine user prompt, False for tool_result plumbing.
pub fn is_human_turn(obj: &Value) -> bool {
    if obj.get("type").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    match obj.get("message").and_then(|m| m.get("content")) {
        Some(Value::Array(blocks)) => !blocks
            .iter()
            .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_result")),
        _ => true, // string content == a typed human message
    }
}

/// Final assistant text produced AFTER the most recent human turn, or None if
/// this turn's answer isn't on disk yet (lets the caller detect the flush race).
pub fn answer_for_last_turn(transcript_path: &str) -> Option<String> {
    let content = fs::read_to_string(transcript_path).ok()?;
    let objs: Vec<Value> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();

    let last_human = objs.iter().rposition(is_human_turn)?;

    let mut answer: Option<String> = None;
    for obj in &objs[last_human + 1..] {
        let text = assistant_text(obj);
        if !text.is_empty() {
            answer = Some(text); // keep the last non-empty text block of this turn
        }
    }
    answer
}

/// Read the finished turn's answer, polling past the transcript-flush race.
pub fn final_message(transcript_path: &str) -> Option<String> {
    let deadline = Instant::now() + FLUSH_WAIT;
    loop {
        let answer = answer_for_last_turn(transcript_path);
        if answer.is_some() || Instant::now() >= deadline {
            return answer;
        }
        sleep(FLUSH_POLL);
    }
}

/// Orchestrate the hook: gate on opt-in, extract the final message, push to
/// Slack and seed the thread. `payload` is the Stop-hook JSON (session_id +
/// transcript_path).
///
/// STUBBED — port from `stop_hook.py:main` (the env/stdin reading lives in the
/// binary; this takes the resolved inputs so it is testable).
pub fn run<C: SlackClient>(
    client: &C,
    store: &SessionStore,
    notify_enabled: bool,
    channel: Option<&str>,
    payload: &str,
) {
    if !notify_enabled {
        return; // opt-in only — stay quiet for ordinary sessions
    }

    let data: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let session_id = data.get("session_id").and_then(|v| v.as_str());
    let transcript = data.get("transcript_path").and_then(|v| v.as_str());

    let text = transcript
        .and_then(final_message)
        .unwrap_or_else(|| "(Claude session finished, but no final message was found.)".to_string());

    let channel = match channel {
        Some(c) if !c.is_empty() => c,
        _ => return, // no SLACK_NOTIFY_CHANNEL configured; skip the push
    };

    push_result(client, store, channel, session_id, &text, false, Some("✅ *Claude session done*"));
}
