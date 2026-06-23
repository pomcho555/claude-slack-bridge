//! Runs the `claude` CLI in headless print mode and parses its JSON output
//! (mirror of `claude_runner.py`).
//!
//!   New job:   claude --print --output-format json [...] "<prompt>"
//!   Continue:  claude --resume <id> --print --output-format json [...] "<prompt>"
//!
//! NOTE: timeout handling is intentionally language-specific and not modeled in
//! the shared spec (see spec/README.md "Out of scope"); the fake CLI is fast.

use std::process::Command;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ClaudeResult {
    pub session_id: Option<String>,
    pub text: String,
    pub is_error: bool,
}

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

        let output = match cmd.output() {
            Ok(o) => o,
            Err(_) => {
                return ClaudeResult {
                    session_id: resumed(),
                    text: format!("❌ Claude binary not found: {:?}. Set CLAUDE_BIN.", self.binary),
                    is_error: true,
                }
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let code_nonzero = !output.status.success();

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
