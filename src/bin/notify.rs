//! `slack-claude-notify` — push text to Slack for an existing Claude session id
//! and seed the thread (mirror of `job.py:notify_main`).
//!
//! Usage: slack-claude-notify --session <id> [--channel C] [--title T] [--error]
//!        [--text TEXT | --file PATH]   (otherwise reads stdin)

use std::io::Read;
use std::process::exit;

use tokio::runtime::Runtime;

use slack_claude_bridge::config::Config;
use slack_claude_bridge::notify::push_result;
use slack_claude_bridge::slack::{build_client, RealSlack};
use slack_claude_bridge::store::SessionStore;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut session: Option<String> = None;
    let mut channel: Option<String> = None;
    let mut title: Option<String> = None;
    let mut text: Option<String> = None;
    let mut file: Option<String> = None;
    let mut is_error = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--session" => session = args.next(),
            "--channel" => channel = args.next(),
            "--title" => title = args.next(),
            "--text" => text = args.next(),
            "--file" => file = args.next(),
            "--error" => is_error = true,
            other => {
                eprintln!("unknown argument: {other}");
                exit(2);
            }
        }
    }

    let session = session.unwrap_or_else(|| {
        eprintln!("usage: slack-claude-notify --session <id> [--channel C] [--title T] [--error] [--text T | --file P]");
        exit(2);
    });
    if text.is_some() && file.is_some() {
        eprintln!("--text and --file are mutually exclusive.");
        exit(2);
    }

    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("Configuration error: {e}");
        exit(2);
    });
    let channel = channel
        .or_else(|| config.notify_channel.clone())
        .unwrap_or_else(|| {
            eprintln!(
                "No target channel. Pass --channel C0123 or set SLACK_NOTIFY_CHANNEL in .env."
            );
            exit(2);
        });

    let text = if let Some(t) = text {
        t
    } else if let Some(path) = file {
        std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("Could not read {path}: {e}");
            exit(2);
        })
    } else {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).ok();
        buf
    };

    let rt = Runtime::new().expect("tokio runtime");
    let (client, token) = build_client(&config.bot_token).unwrap_or_else(|e| {
        eprintln!("Slack client error: {e}");
        exit(2);
    });
    let slack = RealSlack::new(client, token, rt.handle().clone());
    let store = SessionStore::new(&config.db_path);

    let thread_ts = push_result(
        &slack,
        &store,
        &channel,
        Some(&session),
        &text,
        is_error,
        title.as_deref(),
    );
    println!("Posted to {channel} (thread_ts={thread_ts}, session={session}).");
}
