"""Reference harness that runs the language-neutral spec (spec/scenarios.json)
against the real Python bridge.

This is the executable contract: a port in another language reproduces the
bridge by running the SAME scenarios.json through an equivalent harness and
asserting the same outputs. The fake `claude` CLI (tests/fake_claude.py) is
already language-neutral — any implementation just points CLAUDE_BIN at it.

See spec/README.md for the schema and a port checklist.
"""
from __future__ import annotations

import io
import json
from pathlib import Path

import pytest

from slack_claude_bridge import app as bridge
from slack_claude_bridge import notify, stop_hook
from slack_claude_bridge.claude_runner import ClaudeRunner
from slack_claude_bridge.config import Config
from slack_claude_bridge.store import SessionStore

ROOT = Path(__file__).resolve().parent.parent
SPEC = json.loads((ROOT / "spec" / "scenarios.json").read_text(encoding="utf-8"))
FAKE_CLAUDE = Path(__file__).resolve().parent / "fake_claude.py"


# --- shared assertion helpers ----------------------------------------------

def _match_post(actual, expected):
    channel, thread_ts, text = actual
    if "channel" in expected:
        assert channel == expected["channel"], f"channel {channel!r} != {expected['channel']!r}"
    if "thread_ts" in expected:
        assert thread_ts == expected["thread_ts"], f"thread_ts {thread_ts!r} != {expected['thread_ts']!r}"
    if "text_contains" in expected:
        assert expected["text_contains"] in text, f"{expected['text_contains']!r} not in {text!r}"
    if "text_not_contains" in expected:
        assert expected["text_not_contains"] not in text, f"{expected['text_not_contains']!r} unexpectedly in {text!r}"


def _assert_posts(posts, expect):
    if "posts" in expect:
        exp = expect["posts"]
        assert len(posts) == len(exp), f"expected {len(exp)} posts, got {len(posts)}: {posts}"
        for actual, expected in zip(posts, exp):
            _match_post(actual, expected)
    for needle in expect.get("post_any_contains", []):
        assert any(needle in text for _, _, text in posts), f"no post contains {needle!r}: {posts}"


def _assert_uploads(uploads, expect):
    exp = expect.get("uploads")
    if exp is None:
        return
    assert len(uploads) == len(exp), f"expected {len(exp)} uploads, got {len(uploads)}"
    for actual, expected in zip(uploads, exp):
        if "filename" in expected:
            assert actual["filename"] == expected["filename"]
        if "min_content_len" in expected:
            assert len(actual["content"]) >= expected["min_content_len"]


# --- inbound (Slack event -> bridge -> Slack) ------------------------------

class _Poster:
    def __init__(self):
        self.posts = []
        self.uploads = []

    def post(self, channel, thread_ts, text):
        self.posts.append((channel, thread_ts, text))

    def upload(self, channel, thread_ts, filename, title, content):
        self.uploads.append({"filename": filename, "title": title, "content": content})


class _InlineWorkers:
    def submit(self, key, fn, *args):
        fn(*args)


def _resolved_result(claude):
    if "result_len" in claude:
        return "X" * claude["result_len"]
    return claude.get("result", "ok")


@pytest.mark.parametrize("sc", SPEC["inbound"], ids=[s["name"] for s in SPEC["inbound"]])
def test_inbound_scenarios(sc, tmp_path, monkeypatch):
    log = tmp_path / "claude.jsonl"
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(log))
    claude = sc.get("claude", {})
    if "session" in claude:
        monkeypatch.setenv("FAKE_CLAUDE_SESSION", claude["session"])
    monkeypatch.setenv("FAKE_CLAUDE_RESULT", _resolved_result(claude))
    if claude.get("error"):
        monkeypatch.setenv("FAKE_CLAUDE_ERROR", "1")

    store = SessionStore(str(tmp_path / "bridge.db"))
    for s in sc.get("seed", []):
        store.start(s["thread_ts"], s["channel"])
        store.finish(s["thread_ts"], s["session_id"], "done")

    config = Config(
        bot_token="x", app_token="x", claude_bin="ignored", workdir=".",
        permission_mode="acceptEdits", model=None, extra_args=[], timeout=10,
        db_path=":memory:", allowed_users=set(sc["config"].get("allowed_users", [])),
        notify_channel=None,
    )
    runner = ClaudeRunner(
        binary=str(FAKE_CLAUDE), workdir=str(tmp_path), permission_mode="acceptEdits",
        model=None, extra_args=[], timeout=10,
    )
    poster, workers = _Poster(), _InlineWorkers()
    deps = dict(poster=poster, config=config, store=store, runner=runner,
                workers=workers, bot_user_id="UBOT")

    for event in sc["events"]:
        ev = dict(event)
        kind = ev.pop("type")
        (bridge.handle_mention if kind == "app_mention" else bridge.handle_message)(ev, **deps)

    expect = sc["expect"]
    _assert_posts(poster.posts, expect)
    _assert_uploads(poster.uploads, expect)

    if expect.get("no_claude"):
        assert (not log.exists()) or not log.read_text().strip(), "claude was invoked but shouldn't be"
    for i, exp_inv in enumerate(expect.get("claude_invocations", [])):
        invs = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
        inv = invs[i]
        assert inv["resume"] == exp_inv["resume"], f"resume {inv['resume']!r} != {exp_inv['resume']!r}"
        if "prompt_contains" in exp_inv:
            assert exp_inv["prompt_contains"] in inv["prompt"]
    for thread_ts, exp in expect.get("store", {}).items():
        assert store.get(thread_ts).session_id == exp["session_id"]


# --- stop hook (finished local session -> Slack) ---------------------------

class _FakeSlackClient:
    last = None

    def __init__(self, token=None):
        self.posts = []
        self.uploads = []
        _FakeSlackClient.last = self

    def chat_postMessage(self, channel, thread_ts=None, text=None):
        self.posts.append((channel, thread_ts, text))
        return {"ts": "child.0" if thread_ts else "root.0"}

    def files_upload_v2(self, **kw):
        self.uploads.append({"filename": kw.get("filename"), "content": kw.get("content")})


def _transcript_line(entry):
    if "u" in entry:
        return {"type": "user", "message": {"content": entry["u"]}}
    if "tr" in entry:
        return {"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}
    if "think" in entry:
        return {"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "…"}]}}
    if "tool" in entry:
        return {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": entry["tool"]}]}}
    text = "Z" * entry["a_len"] if "a_len" in entry else entry["a"]
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


@pytest.mark.parametrize("sc", SPEC["stop_hook"], ids=[s["name"] for s in SPEC["stop_hook"]])
def test_stop_hook_scenarios(sc, tmp_path, monkeypatch):
    _FakeSlackClient.last = None
    monkeypatch.setattr(notify, "WebClient", _FakeSlackClient)
    monkeypatch.setenv("SLACK_BOT_TOKEN", "xoxb-test")
    monkeypatch.setenv("SLACK_APP_TOKEN", "xapp-test")
    monkeypatch.setenv("SLACK_NOTIFY_CHANNEL", sc["notify_channel"])
    monkeypatch.setenv("DB_PATH", str(tmp_path / "bridge.db"))
    monkeypatch.setenv("CLAUDE_SLACK_NOTIFY", sc["hook"]["notify_flag"])

    store = SessionStore(str(tmp_path / "bridge.db"))
    for s in sc.get("seed", []):
        store.start(s["thread_ts"], s["channel"])
        store.finish(s["thread_ts"], s["session_id"], "done")

    transcript = tmp_path / "t.jsonl"
    transcript.write_text(
        "\n".join(json.dumps(_transcript_line(e)) for e in sc["transcript"]) + "\n",
        encoding="utf-8",
    )

    payload = json.dumps({"session_id": sc["hook"]["session_id"], "transcript_path": str(transcript)})
    monkeypatch.setattr("sys.stdin", io.StringIO(payload))
    stop_hook.main()

    client = _FakeSlackClient.last
    posts = client.posts if client else []
    _assert_posts(posts, sc["expect"])
    _assert_uploads(client.uploads if client else [], sc["expect"])
