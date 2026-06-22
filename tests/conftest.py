"""Shared fixtures + fakes for the bridge's E2E tests.

These tests pin the *observable behaviour* of the bridge (what Slack sees for a
given event / job) without touching the network or a real model. The same
behaviours are the contract any future rewrite must reproduce, so the suite
doubles as an executable spec.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))

from slack_claude_bridge.claude_runner import ClaudeRunner  # noqa: E402
from slack_claude_bridge.store import SessionStore  # noqa: E402

FAKE_CLAUDE = Path(__file__).resolve().parent / "fake_claude.py"


class FakePoster:
    """Captures every outbound Slack call instead of sending it."""

    def __init__(self) -> None:
        self.posts: list[tuple[str, str, str]] = []  # (channel, thread_ts, text)
        self.uploads: list[dict] = []

    def post(self, channel, thread_ts, text):
        self.posts.append((channel, thread_ts, text))

    def upload(self, channel, thread_ts, filename, title, content):
        self.uploads.append(
            {
                "channel": channel,
                "thread_ts": thread_ts,
                "filename": filename,
                "title": title,
                "content": content,
            }
        )

    @property
    def texts(self) -> list[str]:
        return [t for _, _, t in self.posts]


class InlineWorkers:
    """Runs submitted work synchronously so tests can assert deterministically."""

    def __init__(self) -> None:
        self.keys: list[str] = []

    def submit(self, key, fn, *args):
        self.keys.append(key)
        fn(*args)


@pytest.fixture
def store(tmp_path):
    return SessionStore(str(tmp_path / "bridge.db"))


@pytest.fixture
def poster():
    return FakePoster()


@pytest.fixture
def workers():
    return InlineWorkers()


@pytest.fixture
def claude_log(tmp_path, monkeypatch):
    """Path to the fake-claude invocation log; also wires FAKE_CLAUDE_LOG."""
    path = tmp_path / "claude-invocations.jsonl"
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(path))
    return path


@pytest.fixture
def runner(tmp_path, claude_log):
    """A real ClaudeRunner driving the fake binary as a real subprocess."""
    return ClaudeRunner(
        binary=str(FAKE_CLAUDE),
        workdir=str(tmp_path),
        permission_mode="acceptEdits",
        model=None,
        extra_args=[],
        timeout=10,
    )
