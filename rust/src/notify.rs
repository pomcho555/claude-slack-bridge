//! Outbound counterpart to `app.rs`: post a locally-started job's result into
//! Slack and seed the thread (mirror of `notify.py`).

use crate::store::SessionStore;

/// Slack hard-limits a single text block; upload anything larger.
pub const MAX_TEXT: usize = 3500;

/// The Slack Web API surface `push_result` writes through — the seam a fake
/// implements in tests.
pub trait SlackClient {
    /// Returns the `ts` of the posted message.
    fn chat_post_message(&self, channel: &str, thread_ts: Option<&str>, text: &str) -> String;
    fn files_upload_v2(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        filename: &str,
        title: &str,
        content: &str,
    );
}

/// Post a Claude job result into Slack and record `thread_ts -> session_id` so a
/// reply continues the same session via `--resume`. If the session is already
/// mapped to a thread (a `find_by_session` reverse lookup), posts into THAT
/// thread instead of a new root, so one session stays one thread.
///
/// Returns the `thread_ts` of the (root) message.
///
/// STUBBED — port from `notify.py:push_result`.
pub fn push_result<C: SlackClient>(
    client: &C,
    store: &SessionStore,
    channel: &str,
    session_id: Option<&str>,
    text: &str,
    is_error: bool,
    title: Option<&str>,
) -> String {
    let header = match title {
        Some(t) => t.to_string(),
        None if is_error => "❌ *Claude job failed*".to_string(),
        None => "✅ *Claude job done*".to_string(),
    };
    let trimmed = text.trim();
    let body = if trimmed.is_empty() { "(no result text)".to_string() } else { trimmed.to_string() };
    let long = body.chars().count() > MAX_TEXT;

    // If this session is already mapped to a thread, post into THAT thread
    // instead of creating a new root — one session stays one Slack thread.
    let existing = store.find_by_session(session_id);
    let target_channel = existing.as_ref().map(|e| e.channel.clone()).unwrap_or_else(|| channel.to_string());

    let first_text = if !long {
        format!("{header}\n{body}")
    } else {
        format!("{header}\n_Result is long — see the attached file._")
    };
    let resp_ts = client.chat_post_message(
        &target_channel,
        existing.as_ref().map(|e| e.thread_ts.as_str()),
        &first_text,
    );
    // When posting into an existing thread, resp is the child message; the
    // thread root (our key) is the one we already had.
    let thread_ts = existing.as_ref().map(|e| e.thread_ts.clone()).unwrap_or(resp_ts);

    if long {
        client.files_upload_v2(&target_channel, Some(&thread_ts), "claude-result.md", "Claude result", &body);
    }

    // Seed (or refresh) the thread so an in-thread reply maps back to this session.
    if existing.is_none() {
        store.start(&thread_ts, &target_channel);
    }
    store.finish(&thread_ts, session_id, if is_error { "error" } else { "done" });
    thread_ts
}
