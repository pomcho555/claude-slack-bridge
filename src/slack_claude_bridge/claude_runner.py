from __future__ import annotations

import json
import logging
import subprocess
from dataclasses import dataclass

logger = logging.getLogger(__name__)


@dataclass
class ClaudeResult:
    session_id: str | None
    text: str
    is_error: bool


class ClaudeRunner:
    """Runs the `claude` CLI in headless print mode and parses its JSON output.

    New job:   claude --print --output-format json [...] "<prompt>"
    Continue:  claude --resume <session_id> --print --output-format json [...] "<prompt>"
    """

    def __init__(
        self,
        *,
        binary: str,
        workdir: str,
        permission_mode: str,
        model: str | None,
        extra_args: list[str],
        timeout: int,
    ):
        self.binary = binary
        self.workdir = workdir
        self.permission_mode = permission_mode
        self.model = model
        self.extra_args = extra_args
        self.timeout = timeout

    def run_new(self, prompt: str) -> ClaudeResult:
        return self._run(prompt, resume=None)

    def run_resume(self, session_id: str, prompt: str) -> ClaudeResult:
        return self._run(prompt, resume=session_id)

    def _build_cmd(self, prompt: str, resume: str | None) -> list[str]:
        cmd = [
            self.binary,
            "--print",
            "--output-format",
            "json",
            "--permission-mode",
            self.permission_mode,
        ]
        if self.model:
            cmd += ["--model", self.model]
        if resume:
            cmd += ["--resume", resume]
        cmd += self.extra_args
        cmd += [prompt]  # positional prompt last
        return cmd

    def _run(self, prompt: str, resume: str | None) -> ClaudeResult:
        cmd = self._build_cmd(prompt, resume)
        logger.info("Running claude (resume=%s) in %s", resume, self.workdir)
        try:
            proc = subprocess.run(
                cmd,
                cwd=self.workdir,
                capture_output=True,
                text=True,
                timeout=self.timeout,
            )
        except subprocess.TimeoutExpired:
            return ClaudeResult(resume, f"⏱ Claude timed out after {self.timeout}s.", True)
        except FileNotFoundError:
            return ClaudeResult(
                resume, f"❌ Claude binary not found: {self.binary!r}. Set CLAUDE_BIN.", True
            )

        stdout = (proc.stdout or "").strip()
        stderr = (proc.stderr or "").strip()

        if not stdout:
            msg = stderr or "Claude produced no output."
            return ClaudeResult(resume, f"❌ {msg}", True)

        try:
            data = json.loads(stdout)
        except json.JSONDecodeError:
            # Not JSON (unexpected) — surface the raw output rather than hide it.
            return ClaudeResult(resume, stdout, proc.returncode != 0)

        session_id = data.get("session_id") or resume
        text = data.get("result") or data.get("error") or ""
        is_error = bool(data.get("is_error")) or proc.returncode != 0
        if not text:
            text = "(Claude returned no result text)"
        return ClaudeResult(session_id, text, is_error)
