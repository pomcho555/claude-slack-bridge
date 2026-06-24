//! Optional `config.toml` config source plus first-run interactive bootstrap.
//!
//! `config.toml` is the *lower*-precedence source (env var > `config.toml`) so a
//! user who already exports the variables is unaffected. Its purpose is to let a
//! first-time user run the bridge with **no** manual configuration: when the
//! file is absent and the terminal is interactive, [`bootstrap_default`] walks
//! them through setup and persists the answers.
//!
//! Keys mirror the environment-variable names verbatim (e.g. `SLACK_BOT_TOKEN`)
//! so the value lookup in `config.rs` is a single 1:1 map, and the file is
//! self-documenting against the README.
//!
//! The reader/writer handle a deliberately small, flat subset of TOML
//! (`KEY = "value"`, `#` comments, blank lines) — enough for what we generate
//! and tolerant of light hand-editing — rather than pulling in a TOML crate, in
//! keeping with the crate's lean, clap-free dependency set.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

/// Directory (under the user config home) holding `config.toml`.
const APP_DIR: &str = "claude-slack-bridge";
const FILE_NAME: &str = "config.toml";

/// Default config path: `$XDG_CONFIG_HOME/claude-slack-bridge/config.toml`,
/// falling back to `$HOME/.config/claude-slack-bridge/config.toml`.
///
/// Returns `None` when neither `XDG_CONFIG_HOME` nor `HOME` is set, in which
/// case there is simply no config-file layer (environment variables still apply).
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join(APP_DIR).join(FILE_NAME))
}

/// Load the default `config.toml` into a key/value map, or an empty map when it
/// is absent or unreadable. Never errors: a missing or malformed file just
/// means this layer contributes nothing.
pub fn load_default() -> BTreeMap<String, String> {
    let Some(path) = default_path() else {
        return BTreeMap::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => BTreeMap::new(),
    }
}

/// Parse the flat TOML subset we support into a key/value map.
///
/// Recognises `KEY = "quoted"` and `KEY = bare` lines, skips blank lines,
/// `#` comments, and `[section]` headers (we never write sections, but a
/// hand-edited file might contain one). Unparseable lines are ignored rather
/// than rejected — this layer is best-effort.
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = unquote(value.trim());
        map.insert(key.to_string(), value);
    }
    map
}

/// Strip surrounding double quotes and unescape `\\ \" \n \t` if the value is a
/// quoted string; otherwise return the trimmed bare value as-is.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        let inner = &value[1..value.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        value.to_string()
    }
}

/// Render a key/value map as the flat TOML we read back. Keys are emitted in
/// `BTreeMap` (sorted) order for a stable, diff-friendly file.
fn render(map: &BTreeMap<String, String>) -> String {
    let mut out = String::from(
        "# claude-slack-bridge configuration\n\
         # Lower precedence: an environment variable of the same name overrides\n\
         # the value here. Created by first-run setup; safe to edit.\n\n",
    );
    for (key, value) in map {
        out.push_str(key);
        out.push_str(" = \"");
        out.push_str(&escape(value));
        out.push_str("\"\n");
    }
    out
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Write `map` to `path` as TOML, creating parent directories. On Unix the file
/// is created `0600` — it holds Slack tokens, so it must not be world-readable.
fn write(path: &std::path::Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let contents = render(map);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        f.write_all(contents.as_bytes())
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// First-run bootstrap for the default config path.
///
/// Returns `Ok(true)` when a fresh `config.toml` was written, `Ok(false)` when
/// nothing was needed (already configured, non-interactive, or no home dir).
/// It is deliberately conservative — it prompts **only** when *all* of these
/// hold, so it never surprises an already-working setup or a scripted run:
///
/// 1. the default `config.toml` does not yet exist,
/// 2. the required Slack tokens are not already present in the environment, and
/// 3. `interactive` is true (the bridge passes stdin's TTY-ness).
///
/// Per the design, non-interactive contexts (no TTY — e.g. the Stop hook) fall
/// straight through to env vars and stay silent.
pub fn bootstrap_default(interactive: bool) -> Result<bool, String> {
    let Some(path) = default_path() else {
        return Ok(false);
    };
    if path.exists() {
        return Ok(false);
    }

    if env_present("SLACK_BOT_TOKEN") && env_present("SLACK_APP_TOKEN") {
        return Ok(false); // already configured via the environment — don't prompt
    }

    if !interactive {
        return Ok(false); // no TTY: stay silent, fall back to env vars
    }

    let map = prompt(std::io::stdin().lock(), std::io::stdout().lock())?;
    write(&path, &map)?;
    println!("\nSaved configuration to {}", path.display());
    Ok(true)
}

/// True when `name` is set to a non-empty (trimmed) value in the environment.
fn env_present(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Convenience wrapper: bootstrap only when stdin is a real terminal.
pub fn bootstrap_if_interactive() -> Result<bool, String> {
    bootstrap_default(std::io::stdin().is_terminal())
}

/// Interactive Q&A producing the key/value map to persist. Split out from
/// [`bootstrap_default`] so it can be unit-tested over arbitrary readers.
fn prompt<R: std::io::BufRead, W: Write>(
    mut input: R,
    mut out: W,
) -> Result<BTreeMap<String, String>, String> {
    let io_err = |e: std::io::Error| format!("setup aborted: {e}");

    writeln!(
        out,
        "No config found — let's set up claude-slack-bridge.\n\
         Answers are saved to config.toml; press Enter to skip an optional field.\n"
    )
    .map_err(io_err)?;

    let mut map = BTreeMap::new();

    let bot = ask_required(
        &mut input,
        &mut out,
        "Slack bot token (xoxb-…)",
        "SLACK_BOT_TOKEN",
    )?;
    map.insert("SLACK_BOT_TOKEN".to_string(), bot);

    let app = ask_required(
        &mut input,
        &mut out,
        "Slack app token (xapp-…, for Socket Mode)",
        "SLACK_APP_TOKEN",
    )?;
    map.insert("SLACK_APP_TOKEN".to_string(), app);

    for (label, key) in [
        (
            "Allowed Slack user IDs, comma-separated (recommended; empty = everyone)",
            "ALLOWED_USERS",
        ),
        (
            "Default notify channel ID (e.g. C0123, optional)",
            "SLACK_NOTIFY_CHANNEL",
        ),
        (
            "Claude working directory (optional, defaults to CWD)",
            "CLAUDE_WORKDIR",
        ),
    ] {
        if let Some(value) = ask_optional(&mut input, &mut out, label)? {
            map.insert(key.to_string(), value);
        }
    }

    Ok(map)
}

/// Prompt until a non-empty value is given.
fn ask_required<R: std::io::BufRead, W: Write>(
    input: &mut R,
    out: &mut W,
    label: &str,
    key: &str,
) -> Result<String, String> {
    loop {
        match ask_optional(input, out, label)? {
            Some(v) => return Ok(v),
            None => {
                writeln!(out, "  {key} is required.").map_err(|e| format!("setup aborted: {e}"))?;
            }
        }
    }
}

/// Print `label: `, read one line, and return it trimmed (None when empty).
/// EOF on the input stream aborts setup rather than looping forever.
fn ask_optional<R: std::io::BufRead, W: Write>(
    input: &mut R,
    out: &mut W,
    label: &str,
) -> Result<Option<String>, String> {
    let io_err = |e: std::io::Error| format!("setup aborted: {e}");
    write!(out, "{label}: ").map_err(io_err)?;
    out.flush().map_err(io_err)?;

    let mut line = String::new();
    let n = input.read_line(&mut line).map_err(io_err)?;
    if n == 0 {
        return Err("setup aborted: reached end of input".to_string());
    }
    let trimmed = line.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_values() {
        let mut map = BTreeMap::new();
        map.insert("SLACK_BOT_TOKEN".to_string(), "xoxb-abc".to_string());
        map.insert("ALLOWED_USERS".to_string(), "U1,U2".to_string());
        let parsed = parse(&render(&map));
        assert_eq!(parsed, map);
    }

    #[test]
    fn parse_ignores_comments_blanks_and_sections() {
        let text = "\
# a comment
[section]

SLACK_BOT_TOKEN = \"xoxb-1\"
  CLAUDE_WORKDIR = /home/me/repo
bogus line without equals
= novalue
";
        let map = parse(text);
        assert_eq!(map.get("SLACK_BOT_TOKEN").unwrap(), "xoxb-1");
        // Bare (unquoted) values are accepted as-is.
        assert_eq!(map.get("CLAUDE_WORKDIR").unwrap(), "/home/me/repo");
        assert!(!map.contains_key(""));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn escapes_special_characters() {
        let mut map = BTreeMap::new();
        map.insert("CLAUDE_EXTRA_ARGS".to_string(), "a\"b\\c\nd".to_string());
        let rendered = render(&map);
        assert!(rendered.contains(r#"CLAUDE_EXTRA_ARGS = "a\"b\\c\nd""#));
        assert_eq!(parse(&rendered), map);
    }

    #[test]
    fn prompt_collects_required_and_optional() {
        // bot, app, allowed-users, (skip channel), workdir
        let input = b"xoxb-tok\nxapp-tok\nU1,U2\n\n/repo\n" as &[u8];
        let mut out = Vec::new();
        let map = prompt(std::io::BufReader::new(input), &mut out).unwrap();
        assert_eq!(map.get("SLACK_BOT_TOKEN").unwrap(), "xoxb-tok");
        assert_eq!(map.get("SLACK_APP_TOKEN").unwrap(), "xapp-tok");
        assert_eq!(map.get("ALLOWED_USERS").unwrap(), "U1,U2");
        assert_eq!(map.get("CLAUDE_WORKDIR").unwrap(), "/repo");
        assert!(!map.contains_key("SLACK_NOTIFY_CHANNEL"));
    }

    #[test]
    fn prompt_reprompts_when_required_blank() {
        // first bot answer blank, then provided; app provided; skip the rest
        let input = b"\nxoxb-tok\nxapp-tok\n\n\n\n" as &[u8];
        let mut out = Vec::new();
        let map = prompt(std::io::BufReader::new(input), &mut out).unwrap();
        assert_eq!(map.get("SLACK_BOT_TOKEN").unwrap(), "xoxb-tok");
        assert_eq!(map.get("SLACK_APP_TOKEN").unwrap(), "xapp-tok");
        assert!(String::from_utf8_lossy(&out).contains("SLACK_BOT_TOKEN is required"));
    }

    #[test]
    fn prompt_aborts_on_eof() {
        let input = b"" as &[u8];
        let mut out = Vec::new();
        let err = prompt(std::io::BufReader::new(input), &mut out).unwrap_err();
        assert!(err.contains("end of input"));
    }
}
