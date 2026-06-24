//! Bridge entry point: Slack Socket Mode (via slack-morphism) wired to the
//! transport-agnostic core in the library.
//!
//! slack-morphism runs the async WebSocket receive loop. Each event is handed
//! to the synchronous core (`handle_mention` / `handle_message`) on a plain OS
//! thread — outside the tokio runtime — so the core's blocking `claude` runs
//! and its outbound Slack calls (bridged back to async via `Handle::block_on`)
//! never block a runtime worker.

use std::sync::Arc;

use slack_morphism::prelude::*;
use tokio::runtime::Handle;
use tracing::{info, warn};

use slack_claude_bridge::app::{self, Event, ThreadWorkers};
use slack_claude_bridge::claude_runner::ClaudeRunner;
use slack_claude_bridge::config::Config;
use slack_claude_bridge::slack::RealSlack;
use slack_claude_bridge::store::SessionStore;

/// Everything the core needs, shared across events via the listener user-state.
struct AppState {
    poster: RealSlack,
    config: Config,
    store: SessionStore,
    runner: ClaudeRunner,
    workers: ThreadWorkers,
    bot_user_id: String,
}

impl AppState {
    fn dispatch_mention(&self, event: Event) {
        app::handle_mention(
            &event,
            &self.poster,
            &self.config,
            &self.store,
            &self.runner,
            &self.workers,
            &self.bot_user_id,
        );
    }

    fn dispatch_message(&self, event: Event) {
        app::handle_message(
            &event,
            &self.poster,
            &self.config,
            &self.store,
            &self.runner,
            &self.workers,
            &self.bot_user_id,
        );
    }
}

/// Socket Mode push-event callback. Must be a plain `async fn` (coerces to the
/// fn-pointer the callback registry expects), so shared state arrives via the
/// listener user-state rather than a capture.
async fn on_push_event(
    event: SlackPushEventCallback,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = {
        let storage = states.read().await;
        storage
            .get_user_state::<Arc<AppState>>()
            .cloned()
            .expect("AppState user-state set at startup")
    };

    match event.event {
        SlackEventCallbackBody::AppMention(am) => {
            let ev = Event {
                kind: "app_mention".to_string(),
                user: Some(am.user.0),
                channel: Some(am.channel.0),
                ts: Some(am.origin.ts.0),
                thread_ts: am.origin.thread_ts.map(|t| t.0),
                text: am.content.text,
                bot_id: None,
                subtype: None,
            };
            // Hand off to the sync core on a non-runtime thread.
            std::thread::spawn(move || app.dispatch_mention(ev));
        }
        SlackEventCallbackBody::Message(msg) => {
            let ev = Event {
                kind: "message".to_string(),
                user: msg.sender.user.map(|u| u.0),
                bot_id: msg.sender.bot_id.map(|b| b.0),
                ts: Some(msg.origin.ts.0),
                thread_ts: msg.origin.thread_ts.map(|t| t.0),
                channel: msg.origin.channel.map(|c| c.0),
                text: msg.content.and_then(|c| c.text),
                subtype: msg.subtype.map(|s| format!("{s:?}")),
            };
            std::thread::spawn(move || app.dispatch_message(ev));
        }
        _ => {}
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    slack_claude_bridge::cli::handle_help_version(
        "slack-claude-bridge",
        "Run the Slack <-> Claude Code bridge over Socket Mode.\n\n\
         Usage: slack-claude-bridge   (takes no arguments)\n\n\
         Configured from the environment / .env, or an optional\n\
         ~/.config/claude-slack-bridge/config.toml. On first run with none of\n\
         these, an interactive terminal prompts for the required settings and\n\
         writes config.toml. Keys: SLACK_BOT_TOKEN, SLACK_APP_TOKEN, ALLOWED_USERS,\n\
         CLAUDE_WORKDIR, CLAUDE_PERMISSION_MODE, and more (see README).",
    );

    // First-run setup: if no config.toml exists, the required tokens aren't
    // already in the env/.env, and we're on a real terminal, walk the user
    // through creating config.toml. Stays silent (and falls back to env vars)
    // when non-interactive. Runs before logging is configured so its prompts
    // aren't interleaved with log lines on stdout.
    if let Err(e) = slack_claude_bridge::config_file::bootstrap_if_interactive() {
        eprintln!("First-run setup skipped: {e}");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::load().map_err(|e| format!("Configuration error: {e}"))?;

    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));
    let bot_token = SlackApiToken::new(SlackApiTokenValue(config.bot_token.clone()));
    let app_token = SlackApiToken::new(SlackApiTokenValue(config.app_token.clone()));

    let auth = client.open_session(&bot_token).auth_test().await?;
    let bot_user_id = auth.user_id.0.clone();
    info!(
        "Authenticated as {} ({})",
        auth.user.as_deref().unwrap_or("?"),
        bot_user_id
    );
    info!(
        "Claude workdir: {} | permission-mode: {}",
        config.workdir, config.permission_mode
    );
    if config.allowed_users.is_empty() {
        warn!("ALLOWED_USERS is empty — everyone in the workspace can run Claude!");
    }

    let store = SessionStore::new(&config.db_path);
    let runner = ClaudeRunner {
        binary: config.claude_bin.clone(),
        workdir: config.workdir.clone(),
        permission_mode: config.permission_mode.clone(),
        model: config.model.clone(),
        extra_args: config.extra_args.clone(),
        timeout: config.timeout,
    };
    let poster = RealSlack::new(client.clone(), bot_token, Handle::current());

    let app_state = Arc::new(AppState {
        poster,
        config,
        store,
        runner,
        workers: ThreadWorkers::new(),
        bot_user_id,
    });

    let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);
    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone()).with_user_state(app_state),
    );
    let listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment,
        callbacks,
    );

    info!("Starting Socket Mode listener — waiting for Slack events…");
    listener.listen_for(&app_token).await?;
    listener.serve().await;
    Ok(())
}
