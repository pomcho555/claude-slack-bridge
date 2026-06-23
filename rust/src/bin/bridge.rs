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
use tracing::{error, info, warn};

use slack_claude_bridge::app::{self, Event, Poster, ThreadWorkers};
use slack_claude_bridge::claude_runner::ClaudeRunner;
use slack_claude_bridge::config::Config;
use slack_claude_bridge::store::SessionStore;

/// Real Slack Web API surface backing the core's `Poster` seam. Cloneable
/// (Arc/handle/token), and its sync methods bridge to slack-morphism's async
/// API via the tokio runtime handle — only ever called from non-runtime
/// threads (see module docs).
#[derive(Clone)]
struct RealSlack {
    client: Arc<SlackHyperClient>,
    token: SlackApiToken,
    rt: Handle,
}

impl Poster for RealSlack {
    fn post(&self, channel: &str, thread_ts: Option<&str>, text: &str) {
        let mut req = SlackApiChatPostMessageRequest::new(
            SlackChannelId(channel.to_string()),
            SlackMessageContent::new().with_text(text.to_string()),
        );
        if let Some(ts) = thread_ts {
            req = req.with_thread_ts(SlackTs(ts.to_string()));
        }
        let (client, token) = (self.client.clone(), self.token.clone());
        let res = self
            .rt
            .block_on(async move { client.open_session(&token).chat_post_message(&req).await });
        if let Err(e) = res {
            error!("chat.postMessage failed: {e}");
        }
    }

    fn upload(&self, channel: &str, thread_ts: Option<&str>, filename: &str, title: &str, content: &str) {
        let (client, token) = (self.client.clone(), self.token.clone());
        let (channel, filename, title) = (channel.to_string(), filename.to_string(), title.to_string());
        let thread = thread_ts.map(|s| s.to_string());
        let bytes = content.as_bytes().to_vec();
        // files_upload_v2 flow: get URL -> PUT bytes -> complete.
        let res = self.rt.block_on(async move {
            let session = client.open_session(&token);
            let url = session
                .get_upload_url_external(&SlackApiFilesGetUploadUrlExternalRequest::new(
                    filename,
                    bytes.len(),
                ))
                .await?;
            session
                .files_upload_via_url(&SlackApiFilesUploadViaUrlRequest::new(
                    url.upload_url.clone(),
                    bytes,
                    "text/markdown".to_string(),
                ))
                .await?;
            let mut complete = SlackApiFilesCompleteUploadExternalRequest::new(vec![
                SlackApiFilesComplete::new(url.file_id.clone()).with_title(title),
            ])
            .with_channel_id(SlackChannelId(channel));
            if let Some(t) = thread {
                complete = complete.with_thread_ts(SlackTs(t));
            }
            session.files_complete_upload_external(&complete).await?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
        if let Err(e) = res {
            error!("file upload failed: {e}");
        }
    }
}

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::load().map_err(|e| format!("Configuration error: {e}"))?;

    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));
    let bot_token = SlackApiToken::new(SlackApiTokenValue(config.bot_token.clone()));
    let app_token = SlackApiToken::new(SlackApiTokenValue(config.app_token.clone()));

    let auth = client.open_session(&bot_token).auth_test().await?;
    let bot_user_id = auth.user_id.0.clone();
    info!("Authenticated as {} ({})", auth.user.as_deref().unwrap_or("?"), bot_user_id);
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
    let poster = RealSlack { client: client.clone(), token: bot_token, rt: Handle::current() };

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
