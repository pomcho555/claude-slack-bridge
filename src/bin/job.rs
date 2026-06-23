//! `slack-claude-job` — run a Claude job locally, then push its result to Slack
//! and seed the thread (mirror of `job.py:run_job_main`).
//!
//! Usage: slack-claude-job [--channel C0123] [--workdir DIR] [--title T] <prompt...>

use std::process::exit;

use tokio::runtime::Runtime;

use slack_claude_bridge::claude_runner::ClaudeRunner;
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

    let mut channel: Option<String> = None;
    let mut workdir: Option<String> = None;
    let mut title: Option<String> = None;
    let mut prompt_parts: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--channel" => channel = args.next(),
            "--workdir" => workdir = args.next(),
            "--title" => title = args.next(),
            _ => prompt_parts.push(arg),
        }
    }

    if prompt_parts.is_empty() {
        eprintln!("usage: slack-claude-job [--channel C] [--workdir DIR] [--title T] <prompt...>");
        exit(2);
    }
    let prompt = prompt_parts.join(" ");

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

    let runner = ClaudeRunner {
        binary: config.claude_bin.clone(),
        workdir: workdir.unwrap_or_else(|| config.workdir.clone()),
        permission_mode: config.permission_mode.clone(),
        model: config.model.clone(),
        extra_args: config.extra_args.clone(),
        timeout: config.timeout,
    };

    eprintln!("Running Claude job (this may take a while)…");
    let result = runner.run_new(&prompt);

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
        result.session_id.as_deref(),
        &result.text,
        result.is_error,
        title.as_deref(),
    );
    println!(
        "Posted to {channel} (thread_ts={thread_ts}, session={}).",
        result.session_id.as_deref().unwrap_or("?")
    );
    if result.is_error {
        exit(1);
    }
}
