//! `slack-claude-stop-hook` — Claude Code *Stop* hook entry point (mirror of
//! `stop_hook.py:main`).
//!
//! Reads the hook payload (session id + transcript path) as JSON on stdin,
//! extracts this turn's final message (polling past the transcript-flush race),
//! and pushes it to Slack. Opt-in via `CLAUDE_SLACK_NOTIFY`. Always exits 0 — a
//! failure here must never break the Claude session.

use std::io::Read;

use tokio::runtime::Runtime;
use tracing::warn;

use slack_claude_bridge::config::Config;
use slack_claude_bridge::slack::{build_client, RealSlack};
use slack_claude_bridge::stop_hook;
use slack_claude_bridge::store::SessionStore;

const TRUTHY: [&str; 4] = ["1", "true", "yes", "on"];

fn main() {
    // Swallow everything and always exit 0.
    if let Err(e) = run() {
        warn!("stop hook failed (ignored): {e}");
    }
}

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let flag = std::env::var("CLAUDE_SLACK_NOTIFY").unwrap_or_default().trim().to_lowercase();
    if !TRUTHY.contains(&flag.as_str()) {
        return Ok(()); // opt-in only — stay quiet for ordinary sessions
    }

    let mut payload = String::new();
    std::io::stdin().read_to_string(&mut payload)?;

    let config = Config::load().map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

    let channel = std::env::var("SLACK_NOTIFY_CHANNEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| config.notify_channel.clone());
    let channel = match channel {
        Some(c) => c,
        None => {
            warn!("No SLACK_NOTIFY_CHANNEL configured; skipping Slack push.");
            return Ok(());
        }
    };

    let rt = Runtime::new()?;
    let (client, token) = build_client(&config.bot_token)?;
    let slack = RealSlack::new(client, token, rt.handle().clone());
    let store = SessionStore::new(&config.db_path);

    // notify_enabled = true: the opt-in gate was already checked above.
    stop_hook::run(&slack, &store, true, Some(&channel), &payload);
    Ok(())
}
