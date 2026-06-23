//! Inbound bridge core: Slack event -> dispatch -> Claude -> Slack
//! (mirror of `app.py`). Slack Bolt wiring lives in the binary; this module is
//! the testable core the spec harness drives directly.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::thread;

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
    fn upload(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        filename: &str,
        title: &str,
        content: &str,
    );
}

/// A unit of background work handed to [`Workers`].
pub type Job = Box<dyn FnOnce() + Send + 'static>;

/// One sequential worker per Slack thread: a reply that arrives mid-job queues
/// behind it instead of resuming the session concurrently. Different threads
/// run in parallel. Jobs must be `Send + 'static` because the real
/// implementation moves them onto a dedicated OS thread.
pub trait Workers {
    fn submit(&self, key: &str, job: Job);
}

/// Real `Workers`: one long-lived OS thread per Slack thread key, fed by a
/// channel, so jobs for a given thread run strictly in order while different
/// threads run concurrently. Threads live for the process lifetime (fine for
/// personal use; for many threads you'd reap idle ones).
#[derive(Default)]
pub struct ThreadWorkers {
    senders: Mutex<HashMap<String, Sender<Job>>>,
}

impl ThreadWorkers {
    pub fn new() -> ThreadWorkers {
        ThreadWorkers {
            senders: Mutex::new(HashMap::new()),
        }
    }
}

impl Workers for ThreadWorkers {
    fn submit(&self, key: &str, job: Job) {
        let mut senders = self.senders.lock().unwrap();
        let tx = senders.entry(key.to_string()).or_insert_with(|| {
            let (tx, rx) = channel::<Job>();
            thread::Builder::new()
                .name(format!("claude-{key}"))
                .spawn(move || {
                    for job in rx {
                        job();
                    }
                })
                .expect("spawn worker thread");
            tx
        });
        let _ = tx.send(job);
    }
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
                    && inner
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
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
    let prefix = if result.is_error {
        "❌ *Error*\n"
    } else {
        "✅ *Done*\n"
    };
    if result.text.chars().count() <= MAX_TEXT {
        poster.post(
            channel,
            Some(thread_ts),
            &format!("{prefix}{}", result.text),
        );
        return;
    }
    poster.post(
        channel,
        Some(thread_ts),
        &format!("{prefix}_Result is long — see the attached file._"),
    );
    poster.upload(
        channel,
        Some(thread_ts),
        "claude-result.md",
        "Claude result",
        &result.text,
    );
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
        Some(row)
            if row
                .session_id
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false) =>
        {
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
pub fn handle_mention<P, W>(
    event: &Event,
    poster: &P,
    config: &Config,
    store: &SessionStore,
    runner: &ClaudeRunner,
    workers: &W,
    bot_user_id: &str,
) where
    P: Poster + Clone + Send + 'static,
    W: Workers,
{
    if !is_allowed(config, event.user.as_deref()) {
        return;
    }

    // Anchor the conversation at the thread root (or this message if it starts a
    // new thread) so all follow-up replies map to one session.
    let thread_ts = match event.thread_ts.clone().or_else(|| event.ts.clone()) {
        Some(t) => t,
        None => return,
    };
    let channel = match event.channel.clone() {
        Some(c) => c,
        None => return,
    };
    let prompt = strip_mentions(event.text.as_deref().unwrap_or(""));

    if prompt.is_empty() {
        poster.post(
            &channel,
            Some(&thread_ts),
            "👋 Mention me with a task to run.",
        );
        return;
    }

    // A mention inside an already-tracked thread = continue that session. Clone
    // cheap handles into the job so it can outlive this call on a worker thread.
    if store.exists(&thread_ts) {
        poster.post(&channel, Some(&thread_ts), "💬 Continuing this session…");
        let (po, st, ru) = (poster.clone(), store.clone(), runner.clone());
        let (c, t, p) = (channel.clone(), thread_ts.clone(), prompt);
        workers.submit(
            &thread_ts,
            Box::new(move || run_reply(&po, &st, &ru, &c, &t, &p)),
        );
    } else {
        poster.post(
            &channel,
            Some(&thread_ts),
            "🛠 Started — I'll reply in this thread when done. Reply here anytime to continue.",
        );
        let (po, st, ru) = (poster.clone(), store.clone(), runner.clone());
        let (c, t, p) = (channel.clone(), thread_ts.clone(), prompt);
        workers.submit(
            &thread_ts,
            Box::new(move || run_new_job(&po, &st, &ru, &c, &t, &p)),
        );
    }
}

/// Handle a plain `message`: continue a tracked thread, ignoring chatter, bot
/// posts, top-level messages and untracked threads.
pub fn handle_message<P, W>(
    event: &Event,
    poster: &P,
    config: &Config,
    store: &SessionStore,
    runner: &ClaudeRunner,
    workers: &W,
    bot_user_id: &str,
) where
    P: Poster + Clone + Send + 'static,
    W: Workers,
{
    // Only plain human messages: skip edits/deletes/joins and bot posts.
    if event.subtype.is_some() || event.bot_id.is_some() {
        return;
    }
    let text = event.text.as_deref().unwrap_or("");
    // Mentions are handled by handle_mention; avoid double-processing.
    if text.contains(&format!("<@{bot_user_id}>")) {
        return;
    }

    // Must be a reply inside a thread we own; ignore top-level chatter and the
    // thread-root message itself.
    let thread_ts = match &event.thread_ts {
        Some(t) => t.clone(),
        None => return,
    };
    if Some(&thread_ts) == event.ts.as_ref() {
        return;
    }
    if !store.exists(&thread_ts) {
        return;
    }

    if !is_allowed(config, event.user.as_deref()) {
        return;
    }

    let prompt = strip_mentions(text);
    if prompt.is_empty() {
        return;
    }

    let channel = match event.channel.clone() {
        Some(c) => c,
        None => return,
    };
    let (po, st, ru) = (poster.clone(), store.clone(), runner.clone());
    let (c, t, p) = (channel, thread_ts.clone(), prompt);
    workers.submit(
        &thread_ts,
        Box::new(move || run_reply(&po, &st, &ru, &c, &t, &p)),
    );
}
