//! Inbound bridge core: Slack event -> dispatch -> Claude -> Slack
//! (mirror of `app.py`). Slack Bolt wiring lives in the binary; this module is
//! the testable core the spec harness drives directly.

use crate::claude_runner::{ClaudeResult, ClaudeRunner};
use crate::config::Config;
use crate::store::SessionStore;

/// Slack hard-limits a single text block; keep margin and upload anything larger.
pub const MAX_TEXT: usize = 3500;

/// A normalized Slack event (the fields the bridge actually reads).
#[derive(Debug, Clone, Default)]
pub struct Event {
    pub kind: String, // "app_mention" | "message"
    pub user: Option<String>,
    pub bot_id: Option<String>,
    pub ts: Option<String>,
    pub thread_ts: Option<String>,
    pub channel: Option<String>,
    pub text: Option<String>,
    pub subtype: Option<String>,
}

/// The outbound Slack surface the bridge core writes through — the seam that
/// makes the flows testable without a live Slack.
pub trait Poster {
    fn post(&self, channel: &str, thread_ts: Option<&str>, text: &str);
    fn upload(&self, channel: &str, thread_ts: Option<&str>, filename: &str, title: &str, content: &str);
}

/// One sequential worker per Slack thread: a reply that arrives mid-job queues
/// behind it instead of resuming the session concurrently. Different threads
/// run in parallel.
pub trait Workers {
    fn submit<'a>(&self, key: &str, job: Box<dyn FnOnce() + 'a>);
}

/// Strip `<@U123>`-style mentions and surrounding whitespace.
pub fn strip_mentions(text: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with("<@") {
            if let Some(close) = text[i + 2..].find('>') {
                let inner = &text[i + 2..i + 2 + close];
                if !inner.is_empty()
                    && inner.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                {
                    i = i + 2 + close + 1;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}

pub fn is_allowed(config: &Config, user: Option<&str>) -> bool {
    if config.allowed_users.is_empty() {
        return true;
    }
    matches!(user, Some(u) if config.allowed_users.contains(u))
}

pub fn post_result<P: Poster>(poster: &P, channel: &str, thread_ts: &str, result: &ClaudeResult) {
    let prefix = if result.is_error { "❌ *Error*\n" } else { "✅ *Done*\n" };
    if result.text.chars().count() <= MAX_TEXT {
        poster.post(channel, Some(thread_ts), &format!("{prefix}{}", result.text));
        return;
    }
    poster.post(
        channel,
        Some(thread_ts),
        &format!("{prefix}_Result is long — see the attached file._"),
    );
    poster.upload(channel, Some(thread_ts), "claude-result.md", "Claude result", &result.text);
}

pub fn run_new_job<P: Poster>(
    poster: &P,
    store: &SessionStore,
    runner: &ClaudeRunner,
    channel: &str,
    thread_ts: &str,
    prompt: &str,
) {
    store.start(thread_ts, channel);
    let result = runner.run_new(prompt);
    store.finish(
        thread_ts,
        result.session_id.as_deref(),
        if result.is_error { "error" } else { "done" },
    );
    post_result(poster, channel, thread_ts, &result);
}

pub fn run_reply<P: Poster>(
    poster: &P,
    store: &SessionStore,
    runner: &ClaudeRunner,
    channel: &str,
    thread_ts: &str,
    prompt: &str,
) {
    match store.get(thread_ts) {
        Some(row) if row.session_id.as_deref().map(|s| !s.is_empty()).unwrap_or(false) => {
            let sid = row.session_id.unwrap();
            let result = runner.run_resume(&sid, prompt);
            store.finish(
                thread_ts,
                result.session_id.as_deref(),
                if result.is_error { "error" } else { "done" },
            );
            post_result(poster, channel, thread_ts, &result);
        }
        _ => poster.post(
            channel,
            Some(thread_ts),
            "⚠️ No Claude session for this thread yet — ignoring.",
        ),
    }
}

// --- routing (STUBBED — this is the port work) ----------------------------

/// Handle an `app_mention`: anchor the thread, start a new job or continue the
/// tracked session, posting the appropriate ack.
pub fn handle_mention<P: Poster, W: Workers>(
    event: &Event,
    poster: &P,
    config: &Config,
    store: &SessionStore,
    runner: &ClaudeRunner,
    workers: &W,
    bot_user_id: &str,
) {
    unimplemented!("handle_mention: port from app.py:handle_mention")
}

/// Handle a plain `message`: continue a tracked thread, ignoring chatter, bot
/// posts, top-level messages and untracked threads.
pub fn handle_message<P: Poster, W: Workers>(
    event: &Event,
    poster: &P,
    config: &Config,
    store: &SessionStore,
    runner: &ClaudeRunner,
    workers: &W,
    bot_user_id: &str,
) {
    unimplemented!("handle_message: port from app.py:handle_message")
}
