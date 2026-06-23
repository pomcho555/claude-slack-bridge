//! Binary-level CLI tests: `--help`/`--version` for every user-facing binary,
//! plus the Stop hook's relaxed token requirement (it must not demand
//! `SLACK_APP_TOKEN`, unlike the bridge). Hermetic — no network: the hook is
//! steered to return at the missing-channel guard before any Slack call.

use std::io::Write;
use std::process::{Command, Stdio};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Path to a built binary by its cargo bin name.
fn bin(name: &str) -> &'static str {
    match name {
        "slack-claude-bridge" => env!("CARGO_BIN_EXE_slack-claude-bridge"),
        "slack-claude-job" => env!("CARGO_BIN_EXE_slack-claude-job"),
        "slack-claude-notify" => env!("CARGO_BIN_EXE_slack-claude-notify"),
        "slack-claude-stop-hook" => env!("CARGO_BIN_EXE_slack-claude-stop-hook"),
        other => panic!("unknown bin {other}"),
    }
}

const ALL_BINS: [&str; 4] = [
    "slack-claude-bridge",
    "slack-claude-job",
    "slack-claude-notify",
    "slack-claude-stop-hook",
];

#[test]
fn version_flag_prints_name_and_version() {
    for name in ALL_BINS {
        for flag in ["--version", "-V"] {
            let out = Command::new(bin(name)).arg(flag).output().unwrap();
            assert!(out.status.success(), "{name} {flag} should exit 0");
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                stdout.contains(name) && stdout.contains(VERSION),
                "{name} {flag} stdout missing name/version: {stdout:?}"
            );
        }
    }
}

#[test]
fn help_flag_prints_usage() {
    for name in ALL_BINS {
        for flag in ["--help", "-h"] {
            let out = Command::new(bin(name)).arg(flag).output().unwrap();
            assert!(out.status.success(), "{name} {flag} should exit 0");
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Help prints the `<name> <version>` header plus a multi-line body,
            // so it is strictly longer than the single-line `--version` output.
            assert!(
                stdout.contains(name) && stdout.lines().count() >= 3,
                "{name} {flag} stdout missing name/help body: {stdout:?}"
            );
        }
    }
}

/// A unique, empty working dir (no `.env`) so config comes purely from the env.
fn empty_workdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("scb-cli-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The Stop hook must NOT require `SLACK_APP_TOKEN`: with only the bot token set
/// it should load config and reach the missing-channel guard (no app-token
/// error), then exit 0 without touching the network.
#[test]
fn stop_hook_does_not_require_app_token() {
    let dir = empty_workdir("hook");
    let path = std::env::var("PATH").unwrap_or_default();

    let mut child = Command::new(bin("slack-claude-stop-hook"))
        .current_dir(&dir)
        .env_clear()
        .env("PATH", path)
        .env("CLAUDE_SLACK_NOTIFY", "1")
        .env("SLACK_BOT_TOKEN", "xoxb-bogus-for-test")
        // intentionally NO SLACK_APP_TOKEN and NO SLACK_NOTIFY_CHANNEL
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"session_id\":\"s\",\"transcript_path\":\"/nonexistent\"}")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "hook always exits 0; got {:?}",
        out.status
    );
    assert!(
        !stderr.contains("SLACK_APP_TOKEN"),
        "hook must not require SLACK_APP_TOKEN; stderr: {stderr}"
    );
    // Positive proof it got PAST config load (issue would otherwise abort earlier):
    // it reaches the channel guard and reports the missing notify channel.
    assert!(
        stderr.contains("SLACK_NOTIFY_CHANNEL"),
        "expected to reach the channel guard; stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Contrast: the bridge still requires `SLACK_APP_TOKEN` (Socket Mode needs it),
/// failing at config load before any network.
#[test]
fn bridge_still_requires_app_token() {
    let dir = empty_workdir("bridge");
    let path = std::env::var("PATH").unwrap_or_default();

    let out = Command::new(bin("slack-claude-bridge"))
        .current_dir(&dir)
        .env_clear()
        .env("PATH", path)
        .env("SLACK_BOT_TOKEN", "xoxb-bogus-for-test")
        // intentionally NO SLACK_APP_TOKEN
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !out.status.success(),
        "bridge should fail without app token"
    );
    assert!(
        stderr.contains("SLACK_APP_TOKEN"),
        "expected app-token config error; stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
