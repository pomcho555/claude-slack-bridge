//! Runs the `claude` CLI in headless print mode and parses its JSON output
//! (mirror of `claude_runner.py`).
//!
//!   New job:   claude --print --output-format json [...] "<prompt>"
//!   Continue:  claude --resume <id> --print --output-format json [...] "<prompt>"
//!
//! NOTE: timeout handling is intentionally language-specific and not modeled in
//! the shared spec (see spec/README.md "Out of scope"); the fake CLI is fast.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ClaudeResult {
    pub session_id: Option<String>,
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone)]
pub struct ClaudeRunner {
    pub binary: String,
    pub workdir: String,
    pub permission_mode: String,
    pub model: Option<String>,
    pub extra_args: Vec<String>,
    pub timeout: u64,
}

impl ClaudeRunner {
    pub fn run_new(&self, prompt: &str) -> ClaudeResult {
        self.run(prompt, None)
    }

    pub fn run_resume(&self, session_id: &str, prompt: &str) -> ClaudeResult {
        self.run(prompt, Some(session_id))
    }

    fn run(&self, prompt: &str, resume: Option<&str>) -> ClaudeResult {
        let resumed = || resume.map(String::from);

        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&self.workdir)
            .arg("--print")
            .arg("--output-format")
            .arg("json")
            .arg("--permission-mode")
            .arg(&self.permission_mode);
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        if let Some(r) = resume {
            cmd.arg("--resume").arg(r);
        }
        for a in &self.extra_args {
            cmd.arg(a);
        }
        cmd.arg(prompt); // positional prompt last
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => {
                return ClaudeResult {
                    session_id: resumed(),
                    text: format!("❌ Claude binary not found: {:?}. Set CLAUDE_BIN.", self.binary),
                    is_error: true,
                }
            }
        };

        // Drain the pipes on threads so a chatty child can't deadlock on a full
        // pipe buffer while we poll for completion.
        let mut out_pipe = child.stdout.take().unwrap();
        let mut err_pipe = child.stderr.take().unwrap();
        let out_reader = thread::spawn(move || {
            let mut s = String::new();
            let _ = out_pipe.read_to_string(&mut s);
            s
        });
        let err_reader = thread::spawn(move || {
            let mut s = String::new();
            let _ = err_pipe.read_to_string(&mut s);
            s
        });

        // std has no wait-with-timeout; poll try_wait and kill past the deadline.
        let deadline = Instant::now() + Duration::from_secs(self.timeout);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break None,
            }
        };

        let status = match status {
            Some(st) => st,
            None => {
                return ClaudeResult {
                    session_id: resumed(),
                    text: format!("⏱ Claude timed out after {}s.", self.timeout),
                    is_error: true,
                }
            }
        };

        let stdout = out_reader.join().unwrap_or_default().trim().to_string();
        let stderr = err_reader.join().unwrap_or_default().trim().to_string();
        let code_nonzero = !status.success();

        if stdout.is_empty() {
            let msg = if !stderr.is_empty() { stderr } else { "Claude produced no output.".to_string() };
            return ClaudeResult { session_id: resumed(), text: format!("❌ {msg}"), is_error: true };
        }

        match serde_json::from_str::<Value>(&stdout) {
            Ok(data) => {
                let session_id = data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(resumed);
                let mut text = data
                    .get("result")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("error").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                let is_error =
                    data.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false) || code_nonzero;
                if text.is_empty() {
                    text = "(Claude returned no result text)".to_string();
                }
                ClaudeResult { session_id, text, is_error }
            }
            // Not JSON (unexpected) — surface the raw output rather than hide it.
            Err(_) => ClaudeResult { session_id: resumed(), text: stdout, is_error: code_nonzero },
        }
    }
}
