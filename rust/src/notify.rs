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
    unimplemented!("push_result: port from notify.py:push_result")
}
