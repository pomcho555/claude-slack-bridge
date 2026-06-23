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

const USAGE: &str = "\
Claude Code Stop hook: push a finished session's final message to Slack.

Reads the hook payload (JSON with session_id + transcript_path) on stdin, so it
takes no arguments. Wire it into ~/.claude/settings.json under hooks.Stop.

Opt-in: does nothing unless CLAUDE_SLACK_NOTIFY is truthy (1/true/yes/on). When
opted in, diagnostics are written to stderr (tune with RUST_LOG). Needs
SLACK_BOT_TOKEN and a target channel (SLACK_NOTIFY_CHANNEL); SLACK_APP_TOKEN is
not required. Always exits 0 so a failure never breaks the Claude session.";

fn main() {
    slack_claude_bridge::cli::handle_help_version("slack-claude-stop-hook", USAGE);

    let notify = notify_enabled();

    // Only when opted in do we surface diagnostics: this hook fires for *every*
    // Claude session, so ordinary (non-notify) runs must stay completely silent.
    // Warnings go to stderr so they never pollute the hook's stdout.
    if notify {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn".into()),
            )
            .try_init();
    }

    // Swallow everything and always exit 0.
    if let Err(e) = run(notify) {
        warn!("stop hook failed (ignored): {e}");
    }
}

fn notify_enabled() -> bool {
    let flag = std::env::var("CLAUDE_SLACK_NOTIFY")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    TRUTHY.contains(&flag.as_str())
}

fn run(notify: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !notify {
        return Ok(()); // opt-in only — stay quiet for ordinary sessions
    }

    let mut payload = String::new();
    std::io::stdin().read_to_string(&mut payload)?;

    // The hook posts with the bot token alone, so it does not require
    // SLACK_APP_TOKEN (unlike the Socket Mode bridge).
    let config = Config::load_for_hook()
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

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
