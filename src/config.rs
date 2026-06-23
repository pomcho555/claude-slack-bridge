//! Configuration, loaded from the environment (mirror of `config.py`).

use std::collections::HashSet;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub bot_token: String,
    pub app_token: String,
    pub claude_bin: String,
    pub workdir: String,
    pub permission_mode: String,
    pub model: Option<String>,
    pub extra_args: Vec<String>,
    pub timeout: u64,
    pub db_path: String,
    pub allowed_users: HashSet<String>,
    pub notify_channel: Option<String>,
}

fn require(name: &str) -> Result<String, String> {
    match env::var(name).map(|v| v.trim().to_string()) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(format!("Missing required environment variable: {name}")),
    }
}

fn opt(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl Config {
    /// Full configuration for the bridge: requires both Slack tokens.
    pub fn load() -> Result<Config, String> {
        Self::load_with(true)
    }

    /// Configuration for the Stop hook, which posts with the bot token alone and
    /// never opens a Socket Mode connection. `SLACK_APP_TOKEN` is therefore
    /// *not* required, so an env-var-only (`.env`-less) hook setup doesn't have
    /// to set a token it never uses.
    pub fn load_for_hook() -> Result<Config, String> {
        Self::load_with(false)
    }

    fn load_with(require_app_token: bool) -> Result<Config, String> {
        // Load .env from the current dir (mirrors python-dotenv in config.py).
        // Missing file is fine; real env vars still win.
        let _ = dotenvy::dotenv();

        let app_token = if require_app_token {
            require("SLACK_APP_TOKEN")?
        } else {
            opt("SLACK_APP_TOKEN").unwrap_or_default()
        };

        let allowed_users = env::var("ALLOWED_USERS")
            .unwrap_or_default()
            .split(',')
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty())
            .collect();

        let extra_args = opt("CLAUDE_EXTRA_ARGS")
            .map(|raw| raw.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        let timeout_raw = opt("CLAUDE_TIMEOUT").unwrap_or_else(|| "14400".to_string());
        let timeout = timeout_raw
            .parse::<u64>()
            .map_err(|_| format!("CLAUDE_TIMEOUT must be an integer, got {timeout_raw:?}"))?;

        Ok(Config {
            bot_token: require("SLACK_BOT_TOKEN")?,
            app_token,
            claude_bin: opt("CLAUDE_BIN").unwrap_or_else(|| "claude".to_string()),
            workdir: opt("CLAUDE_WORKDIR").unwrap_or_else(|| {
                env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            }),
            permission_mode: opt("CLAUDE_PERMISSION_MODE")
                .unwrap_or_else(|| "acceptEdits".to_string()),
            model: opt("CLAUDE_MODEL"),
            extra_args,
            timeout,
            db_path: opt("DB_PATH").unwrap_or_else(|| "bridge.db".to_string()),
            allowed_users,
            notify_channel: opt("SLACK_NOTIFY_CHANNEL"),
        })
    }
}
