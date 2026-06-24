//! Configuration, loaded from the environment, with an optional `config.toml`
//! as the lower-precedence fallback (env var > `config.toml`; see
//! [`crate::config_file`]).

use std::collections::{BTreeMap, HashSet};
use std::env;

use crate::config_file;

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

/// Resolve a setting: an environment variable wins; otherwise fall back to the
/// `config.toml` map. Empty/whitespace values are treated as absent at each
/// level so they fall through to the next source.
fn value(name: &str, file: &BTreeMap<String, String>) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            file.get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
}

fn require(name: &str, file: &BTreeMap<String, String>) -> Result<String, String> {
    value(name, file).ok_or_else(|| format!("Missing required setting: {name}"))
}

impl Config {
    /// Full configuration for the bridge: requires both Slack tokens.
    pub fn load() -> Result<Config, String> {
        Self::load_with(true)
    }

    /// Configuration for the Stop hook, which posts with the bot token alone and
    /// never opens a Socket Mode connection. `SLACK_APP_TOKEN` is therefore
    /// *not* required, so an env-var-only hook setup doesn't have to set a token
    /// it never uses.
    pub fn load_for_hook() -> Result<Config, String> {
        Self::load_with(false)
    }

    fn load_with(require_app_token: bool) -> Result<Config, String> {
        // Lower-precedence fallback; empty when no config.toml is present.
        let file = config_file::load_default();

        let app_token = if require_app_token {
            require("SLACK_APP_TOKEN", &file)?
        } else {
            value("SLACK_APP_TOKEN", &file).unwrap_or_default()
        };

        let allowed_users = value("ALLOWED_USERS", &file)
            .unwrap_or_default()
            .split(',')
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty())
            .collect();

        let extra_args = value("CLAUDE_EXTRA_ARGS", &file)
            .map(|raw| raw.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        let timeout_raw = value("CLAUDE_TIMEOUT", &file).unwrap_or_else(|| "14400".to_string());
        let timeout = timeout_raw
            .parse::<u64>()
            .map_err(|_| format!("CLAUDE_TIMEOUT must be an integer, got {timeout_raw:?}"))?;

        Ok(Config {
            bot_token: require("SLACK_BOT_TOKEN", &file)?,
            app_token,
            claude_bin: value("CLAUDE_BIN", &file).unwrap_or_else(|| "claude".to_string()),
            workdir: value("CLAUDE_WORKDIR", &file).unwrap_or_else(|| {
                env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            }),
            permission_mode: value("CLAUDE_PERMISSION_MODE", &file)
                .unwrap_or_else(|| "acceptEdits".to_string()),
            model: value("CLAUDE_MODEL", &file),
            extra_args,
            timeout,
            db_path: value("DB_PATH", &file).unwrap_or_else(|| "bridge.db".to_string()),
            allowed_users,
            notify_channel: value("SLACK_NOTIFY_CHANNEL", &file),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_value_used_when_env_absent() {
        // A name that is not a real env var, so this stays hermetic.
        let key = "ZZ_BRIDGE_CFG_ONLY_IN_FILE";
        let mut file = BTreeMap::new();
        file.insert(key.to_string(), "from-file".to_string());
        assert_eq!(value(key, &file).as_deref(), Some("from-file"));
    }

    #[test]
    fn env_takes_precedence_over_file() {
        let key = "ZZ_BRIDGE_CFG_ENV_WINS";
        // Safe: a unique key no other test touches.
        env::set_var(key, "from-env");
        let mut file = BTreeMap::new();
        file.insert(key.to_string(), "from-file".to_string());
        assert_eq!(value(key, &file).as_deref(), Some("from-env"));
        env::remove_var(key);
    }

    #[test]
    fn blank_values_fall_through() {
        let key = "ZZ_BRIDGE_CFG_BLANK_ENV";
        env::set_var(key, "   ");
        let mut file = BTreeMap::new();
        file.insert(key.to_string(), "from-file".to_string());
        // Whitespace-only env is treated as absent -> file wins.
        assert_eq!(value(key, &file).as_deref(), Some("from-file"));
        env::remove_var(key);
    }
}
