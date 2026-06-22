"""E2E of the inbound Slack flow: a synthetic event in, captured Slack out.

Drives the real handlers with a real store and a real ClaudeRunner (fake
binary), so each test asserts the full mention/reply -> claude -> Slack loop.
"""
from __future__ import annotations

import json

import pytest

from slack_claude_bridge import app as bridge
from slack_claude_bridge.config import Config


def _config(allowed=None):
    return Config(
        bot_token="xoxb-test",
        app_token="xapp-test",
        claude_bin="ignored",  # the `runner` fixture owns the real binary
        workdir=".",
        permission_mode="acceptEdits",
        model=None,
        extra_args=[],
        timeout=10,
        db_path=":memory:",
        allowed_users=set(allowed or []),
        notify_channel=None,
    )


def _invocations(log_path):
    return [json.loads(line) for line in log_path.read_text().splitlines() if line.strip()]


def _mention(deps, **event):
    bridge.handle_mention(event, bot_user_id="UBOT", **deps)


def _message(deps, **event):
    bridge.handle_message(event, bot_user_id="UBOT", **deps)


@pytest.fixture
def deps(poster, store, runner, workers):
    return {
        "poster": poster,
        "config": _config(),
        "store": store,
        "runner": runner,
        "workers": workers,
    }


def test_new_mention_starts_job_and_posts_result(deps, poster, store, claude_log, monkeypatch):
    monkeypatch.setenv("FAKE_CLAUDE_SESSION", "sess-new-1")
    monkeypatch.setenv("FAKE_CLAUDE_RESULT", "refactor done")

    _mention(deps, user="U1", ts="111.0", channel="C1", text="<@UBOT> refactor auth")

    # Slack sees: an ack, then the result.
    assert poster.texts[0].startswith("🛠 Started")
    assert "✅ *Done*" in poster.texts[1] and "refactor done" in poster.texts[1]
    # Both posted into the thread rooted at the mention.
    assert all(thread == "111.0" for _, thread, _ in poster.posts)
    # Store maps the thread to the new session.
    assert store.get("111.0").session_id == "sess-new-1"
    # A new job does not resume.
    assert _invocations(claude_log)[0]["resume"] is None


def test_mention_in_tracked_thread_resumes(deps, poster, store, claude_log):
    store.start("200.0", "C1")
    store.finish("200.0", "sess-existing", "done")

    _mention(deps, user="U1", ts="200.5", thread_ts="200.0", channel="C1", text="<@UBOT> keep going")

    assert poster.texts[0].startswith("💬 Continuing")
    assert _invocations(claude_log)[0]["resume"] == "sess-existing"
    assert "keep going" in _invocations(claude_log)[0]["prompt"]


def test_plain_reply_in_tracked_thread_resumes(deps, poster, store, claude_log):
    store.start("300.0", "C1")
    store.finish("300.0", "sess-300", "done")

    _message(deps, user="U1", ts="300.9", thread_ts="300.0", channel="C1", text="and the tests?")

    assert _invocations(claude_log)[0]["resume"] == "sess-300"
    assert any("✅ *Done*" in t for t in poster.texts)


def test_reply_in_untracked_thread_ignored(deps, poster, claude_log):
    _message(deps, user="U1", ts="400.9", thread_ts="400.0", channel="C1", text="hello?")

    assert poster.posts == []
    assert not claude_log.exists()  # claude never invoked


def test_top_level_message_ignored(deps, poster):
    # No thread_ts (or thread_ts == ts) => not a reply we own.
    _message(deps, user="U1", ts="500.0", channel="C1", text="just chatting")
    assert poster.posts == []


def test_bot_own_message_ignored(deps, poster):
    _message(deps, bot_id="B1", ts="600.9", thread_ts="600.0", channel="C1", text="✅ Done")
    assert poster.posts == []


def test_empty_mention_prompts_for_a_task(deps, poster, claude_log):
    _mention(deps, user="U1", ts="700.0", channel="C1", text="<@UBOT>")
    assert poster.texts == ["👋 Mention me with a task to run."]
    assert not claude_log.exists()


def test_disallowed_user_is_ignored(poster, store, runner, workers, claude_log):
    deps = {
        "poster": poster,
        "config": _config(allowed=["U_OK"]),
        "store": store,
        "runner": runner,
        "workers": workers,
    }
    _mention(deps, user="U_BAD", ts="800.0", channel="C1", text="<@UBOT> do it")
    assert poster.posts == []
    assert not claude_log.exists()


def test_long_result_is_uploaded_as_file(deps, poster, monkeypatch):
    monkeypatch.setenv("FAKE_CLAUDE_RESULT", "X" * (bridge.MAX_TEXT + 50))

    _mention(deps, user="U1", ts="900.0", channel="C1", text="<@UBOT> big one")

    assert len(poster.uploads) == 1
    assert poster.uploads[0]["filename"] == "claude-result.md"
    assert len(poster.uploads[0]["content"]) > bridge.MAX_TEXT
    # The in-thread message points at the attachment rather than inlining it.
    assert any("see the attached file" in t for t in poster.texts)
