"""E2E of the outbound Stop-hook path: a finished local session -> Slack.

Covers the behaviours that were actually broken in the field: posting THIS
turn's final answer (not the previous one), waiting past the transcript-flush
race, and collapsing repeat fires of one session into a single thread.
"""
from __future__ import annotations

import io
import json

import pytest

from slack_claude_bridge import notify
from slack_claude_bridge import stop_hook
from slack_claude_bridge.store import SessionStore


# --- transcript builders (mirror the real JSONL shapes) --------------------

def _human(text):
    return {"type": "user", "message": {"content": text}}


def _tool_result():
    return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}


def _thinking():
    return {"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "…"}]}}


def _tool_use():
    return {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash"}]}}


def _text(text):
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def _write_transcript(path, objs):
    path.write_text("\n".join(json.dumps(o) for o in objs) + "\n", encoding="utf-8")


# --- fake Slack + env wiring ------------------------------------------------

class FakeSlackClient:
    instances: list["FakeSlackClient"] = []

    def __init__(self, token=None):
        self.posts = []
        self.uploads = []
        FakeSlackClient.instances.append(self)

    def chat_postMessage(self, channel, thread_ts=None, text=None):
        self.posts.append({"channel": channel, "thread_ts": thread_ts, "text": text})
        # Slack returns the child ts when threaded, a fresh root ts otherwise.
        return {"ts": "child.0" if thread_ts else "root.0"}

    def files_upload_v2(self, **kw):
        self.uploads.append(kw)


@pytest.fixture
def slack(monkeypatch):
    FakeSlackClient.instances = []
    monkeypatch.setattr(notify, "WebClient", FakeSlackClient)
    return FakeSlackClient


@pytest.fixture
def hook_env(tmp_path, monkeypatch):
    monkeypatch.setenv("SLACK_BOT_TOKEN", "xoxb-test")
    monkeypatch.setenv("SLACK_APP_TOKEN", "xapp-test")
    monkeypatch.setenv("SLACK_NOTIFY_CHANNEL", "C_NOTIFY")
    monkeypatch.setenv("DB_PATH", str(tmp_path / "bridge.db"))
    monkeypatch.setenv("CLAUDE_SLACK_NOTIFY", "1")
    return tmp_path


def _run_hook(monkeypatch, session_id, transcript_path):
    payload = json.dumps({"session_id": session_id, "transcript_path": str(transcript_path)})
    monkeypatch.setattr("sys.stdin", io.StringIO(payload))
    stop_hook.main()


# --- tests ------------------------------------------------------------------

def test_posts_latest_turn_answer(slack, hook_env, monkeypatch):
    t = hook_env / "t.jsonl"
    _write_transcript(t, [_human("Q1"), _thinking(), _text("ANSWER 1"),
                          _human("Q2"), _thinking(), _text("ANSWER 2")])

    _run_hook(monkeypatch, "sess-A", t)

    client = slack.instances[-1]
    assert len(client.posts) == 1
    post = client.posts[0]
    assert post["channel"] == "C_NOTIFY"
    assert post["thread_ts"] is None  # brand-new thread root
    assert "ANSWER 2" in post["text"]      # the latest turn ...
    assert "ANSWER 1" not in post["text"]  # ... not the previous one


def test_opt_out_when_flag_unset(slack, hook_env, monkeypatch):
    monkeypatch.setenv("CLAUDE_SLACK_NOTIFY", "0")
    t = hook_env / "t.jsonl"
    _write_transcript(t, [_human("Q"), _text("A")])

    _run_hook(monkeypatch, "sess-A", t)

    assert slack.instances == []  # WebClient never even constructed


def test_repeat_fire_same_session_reuses_thread(slack, hook_env, monkeypatch):
    # An earlier turn of this session already created a thread.
    store = SessionStore(str(hook_env / "bridge.db"))
    store.start("root-existing", "C_NOTIFY")
    store.finish("root-existing", "sess-A", "done")

    t = hook_env / "t.jsonl"
    _write_transcript(t, [_human("Q"), _text("second turn answer")])
    _run_hook(monkeypatch, "sess-A", t)

    post = slack.instances[-1].posts[0]
    assert post["thread_ts"] == "root-existing"  # posted INTO the one thread
    assert "second turn answer" in post["text"]


def test_waits_for_flush_then_posts(slack, hook_env, monkeypatch):
    # Shrink the poll window so the test is fast.
    monkeypatch.setattr(stop_hook, "_FLUSH_WAIT_S", 2.0)
    monkeypatch.setattr(stop_hook, "_FLUSH_POLL_S", 0.05)

    t = hook_env / "t.jsonl"
    # Turn just finished but its final text isn't on disk yet — only the prompt
    # and intermediate (text-less) blocks are.
    _write_transcript(t, [_human("Q"), _thinking(), _tool_use(), _tool_result()])

    import threading

    def flush_late():
        import time
        time.sleep(0.2)
        with open(t, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(_text("flushed answer")) + "\n")

    threading.Thread(target=flush_late).start()
    _run_hook(monkeypatch, "sess-A", t)

    assert "flushed answer" in slack.instances[-1].posts[0]["text"]


def test_long_answer_uploaded_as_file(slack, hook_env, monkeypatch):
    t = hook_env / "t.jsonl"
    _write_transcript(t, [_human("Q"), _text("Z" * (notify.MAX_TEXT + 100))])

    _run_hook(monkeypatch, "sess-A", t)

    client = slack.instances[-1]
    assert len(client.uploads) == 1
    assert client.uploads[0]["filename"] == "claude-result.md"
