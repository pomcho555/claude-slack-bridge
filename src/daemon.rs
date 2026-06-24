//! Opt-in standalone daemon mode for the bridge.
//!
//! The foreground is and stays the default — the bridge logs to stdout/stderr
//! and lets the environment supervise it (systemd, Docker, tmux, …). This
//! module adds an **explicit** way to background it for users without a process
//! supervisor (e.g. Kaggle without systemd): `start` (or the `--daemon` flag)
//! re-executes the bridge detached from the terminal, `stop` terminates it, and
//! `status` reports whether it is alive.
//!
//! Detaching is done by re-execution rather than `fork`/`setsid`: a `fork` after
//! tokio's runtime threads exist is unsound, whereas spawning a fresh process
//! via [`std::process::Command`] sidesteps that entirely and needs no `unsafe`.
//! The child's stdout/stderr are pointed at a log file, it is placed in its own
//! process group, and the parent exits immediately so the child is orphaned to
//! init — surviving the terminal closing. Its PID is recorded in a pidfile so
//! `stop` can signal it. Signalling and the liveness check shell out to `kill`
//! (POSIX), in keeping with the crate's lean, dependency-light toolbox.
//!
//! Unix-only; on other platforms the control commands report that and exit.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Directory (under the user state home) holding the pidfile and log.
const APP_DIR: &str = "claude-slack-bridge";

/// Set on the re-executed child so it runs the bridge in the foreground (its
/// std streams are already the log file) instead of daemonizing a second time.
const MARKER_ENV: &str = "_SLACK_CLAUDE_BRIDGE_DAEMONIZED";

/// Inspect the first CLI argument for a daemon control command.
///
/// `start` / `--daemon`, `stop` and `status` each perform their action and
/// **exit the process**. The function simply *returns* — letting the caller run
/// the bridge in the foreground — for the re-executed child, for an unrecognised
/// first argument, and when no argument is given at all. Backgrounding is thus
/// never implicit: only an explicit subcommand or flag daemonizes.
pub fn dispatch() {
    if std::env::var_os(MARKER_ENV).is_some() {
        return; // re-executed child: run the bridge in the foreground
    }
    let Some(arg) = std::env::args().nth(1) else {
        return; // no subcommand: ordinary foreground run
    };
    match arg.as_str() {
        "start" | "--daemon" => start(),
        "stop" => stop(),
        "status" => status(),
        _ => {} // unknown argument: fall through to a foreground run
    }
}

/// Base directory for the pidfile and log: `$XDG_STATE_HOME/claude-slack-bridge`,
/// falling back to `$HOME/.local/state/claude-slack-bridge`, and finally the
/// current directory when neither is set.
fn base_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
        .map(|base| base.join(APP_DIR))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Pidfile path, overridable with `BRIDGE_PID_FILE`.
fn pid_path() -> PathBuf {
    env_override("BRIDGE_PID_FILE").unwrap_or_else(|| base_dir().join("bridge.pid"))
}

/// Log path, overridable with `BRIDGE_LOG_FILE`.
fn log_path() -> PathBuf {
    env_override("BRIDGE_LOG_FILE").unwrap_or_else(|| base_dir().join("bridge.log"))
}

fn env_override(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// PID recorded in the pidfile, but only when that process is still alive.
///
/// Returns `None` when there is no pidfile, it is malformed, or the process has
/// exited — in which case the (stale) pidfile is left for `start`/`stop` to deal
/// with, so callers can distinguish "not running" from "running".
fn running_pid() -> Option<u32> {
    let pid: u32 = fs::read_to_string(pid_path()).ok()?.trim().parse().ok()?;
    process_alive(pid).then_some(pid)
}

fn write_pidfile(path: &std::path::Path, pid: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(path)?;
    writeln!(f, "{pid}")
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // `kill -0` sends no signal but performs the existence/permission check.
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn start() -> ! {
    use std::os::unix::process::CommandExt; // process_group

    if let Some(pid) = running_pid() {
        eprintln!("claude-slack-bridge is already running (PID {pid}).");
        std::process::exit(1);
    }

    let pid_file = pid_path();
    let log_file = log_path();

    if let Some(parent) = log_file.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("Could not create {}: {e}", parent.display());
            std::process::exit(1);
        }
    }

    let out = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Could not open log file {}: {e}", log_file.display());
            std::process::exit(1);
        }
    };
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Could not set up daemon logging: {e}");
            std::process::exit(1);
        }
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Could not locate the bridge executable: {e}");
            std::process::exit(1);
        }
    };

    // Re-execute ourselves with no arguments (the marker tells the child to run
    // the bridge in the foreground), stdin closed, std streams in the log file,
    // and in its own process group so terminal job-control signals skip it.
    let child = Command::new(exe)
        .env(MARKER_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to start the daemon: {e}");
            std::process::exit(1);
        }
    };
    let pid = child.id();

    if let Err(e) = write_pidfile(&pid_file, pid) {
        eprintln!(
            "Started (PID {pid}) but could not write pidfile {}: {e}\n\
             `stop`/`status` won't find it — terminate it manually with: kill {pid}",
            pid_file.display()
        );
        std::process::exit(1);
    }

    // Parent exits without waiting → the child is orphaned to init and survives
    // this terminal closing.
    println!("claude-slack-bridge started in the background (PID {pid}).");
    println!("  logs:    {}", log_file.display());
    println!("  pidfile: {}", pid_file.display());
    println!("Stop it with: slack-claude-bridge stop");
    std::process::exit(0);
}

#[cfg(unix)]
fn stop() -> ! {
    let pid_file = pid_path();
    match running_pid() {
        Some(pid) => {
            let signalled = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !signalled {
                eprintln!("Failed to signal PID {pid}.");
                std::process::exit(1);
            }
            let _ = fs::remove_file(&pid_file);
            println!("Sent SIGTERM to claude-slack-bridge (PID {pid}).");
            std::process::exit(0);
        }
        None => {
            if pid_file.exists() {
                let _ = fs::remove_file(&pid_file);
                println!(
                    "claude-slack-bridge is not running; removed stale pidfile {}.",
                    pid_file.display()
                );
            } else {
                println!("claude-slack-bridge is not running.");
            }
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
fn status() -> ! {
    match running_pid() {
        Some(pid) => {
            println!("claude-slack-bridge is running (PID {pid}).");
            println!("  logs:    {}", log_path().display());
            println!("  pidfile: {}", pid_path().display());
            std::process::exit(0);
        }
        None => {
            println!("claude-slack-bridge is not running.");
            std::process::exit(1);
        }
    }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(not(unix))]
fn start() -> ! {
    unsupported()
}

#[cfg(not(unix))]
fn stop() -> ! {
    unsupported()
}

#[cfg(not(unix))]
fn status() -> ! {
    unsupported()
}

#[cfg(not(unix))]
fn unsupported() -> ! {
    eprintln!(
        "Background daemon mode is only supported on Unix; \
         run the bridge in the foreground instead."
    );
    std::process::exit(1);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn process_alive_detects_self_and_rejects_bogus() {
        assert!(process_alive(std::process::id()));
        // A PID far above any real pid_max but still a positive `pid_t` (avoid
        // u32::MAX, which wraps to -1 — "all processes" — and would succeed).
        assert!(!process_alive(2_000_000_000));
    }

    #[test]
    fn pidfile_round_trips() {
        let path = std::env::temp_dir().join(format!(
            "scb-daemon-test-{}-{}.pid",
            std::process::id(),
            "rt"
        ));
        write_pidfile(&path, 4242).unwrap();
        let read: u32 = fs::read_to_string(&path).unwrap().trim().parse().unwrap();
        assert_eq!(read, 4242);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn env_override_wins_for_paths() {
        // Hermetic: unique keys nothing else touches.
        std::env::set_var("BRIDGE_PID_FILE", "/custom/bridge.pid");
        std::env::set_var("BRIDGE_LOG_FILE", "/custom/bridge.log");
        assert_eq!(pid_path(), PathBuf::from("/custom/bridge.pid"));
        assert_eq!(log_path(), PathBuf::from("/custom/bridge.log"));
        std::env::remove_var("BRIDGE_PID_FILE");
        std::env::remove_var("BRIDGE_LOG_FILE");
    }
}
