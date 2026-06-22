"""E2E of the claude subprocess seam: real process, real argv, real parsing."""
from __future__ import annotations

import json
from pathlib import Path

from slack_claude_bridge.claude_runner import ClaudeRunner

FAKE_CLAUDE = Path(__file__).resolve().parent / "fake_claude.py"


def _invocations(log_path):
    return [json.loads(line) for line in log_path.read_text().splitlines() if line.strip()]


def test_run_new_parses_session_and_result(runner, claude_log, monkeypatch):
    monkeypatch.setenv("FAKE_CLAUDE_SESSION", "sess-1")
    monkeypatch.setenv("FAKE_CLAUDE_RESULT", "all done")

    result = runner.run_new("do the thing")

    assert result.session_id == "sess-1"
    assert result.text == "all done"
    assert result.is_error is False

    inv = _invocations(claude_log)
    assert len(inv) == 1
    assert inv[0]["resume"] is None  # a new job must NOT pass --resume
    assert inv[0]["prompt"] == "do the thing"


def test_run_resume_passes_resume_flag(runner, claude_log):
    result = runner.run_resume("sess-42", "keep going")

    assert result.session_id == "sess-42"
    inv = _invocations(claude_log)
    assert inv[0]["resume"] == "sess-42"  # continuation MUST pass --resume <id>
    assert inv[0]["prompt"] == "keep going"


def test_timeout_is_reported_not_raised(tmp_path, claude_log, monkeypatch):
    monkeypatch.setenv("FAKE_CLAUDE_SLEEP", "3")
    fast = ClaudeRunner(
        binary=str(FAKE_CLAUDE),
        workdir=str(tmp_path),
        permission_mode="acceptEdits",
        model=None,
        extra_args=[],
        timeout=1,
    )

    result = fast.run_new("slow one")

    assert result.is_error is True
    assert "timed out" in result.text.lower()


def test_non_json_output_surfaced_verbatim(runner, monkeypatch):
    monkeypatch.setenv("FAKE_CLAUDE_RAW", "boom: not json")

    result = runner.run_new("x")

    assert "boom: not json" in result.text


def test_missing_binary_is_reported(tmp_path):
    broken = ClaudeRunner(
        binary="claude-does-not-exist",
        workdir=str(tmp_path),
        permission_mode="acceptEdits",
        model=None,
        extra_args=[],
        timeout=5,
    )

    result = broken.run_new("x")

    assert result.is_error is True
    assert "not found" in result.text.lower()
