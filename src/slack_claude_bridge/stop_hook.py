"""Claude Code *Stop* hook: when a Claude session finishes, push its final
message to Slack and seed the thread, so you can continue it from your phone —
without having to start the job through ``slack-claude-job``.

Wire it into settings.json (see README). It is OPT-IN: it does nothing unless
the environment variable ``CLAUDE_SLACK_NOTIFY`` is truthy, so ordinary
interactive sessions stay quiet. Run a long job you want to be notified about
with, e.g.::

    CLAUDE_SLACK_NOTIFY=1 claude -p "refactor the auth module and run the tests"

The hook receives the session id and transcript path as JSON on stdin (the
Claude Code Stop-hook contract) and always exits 0 — a failure here must never
break the Claude session.
"""

from __future__ import annotations

import json
import logging
import os
import sys
import time

from .config import Config
from .notify import push_result
from .store import SessionStore

logger = logging.getLogger("slack_claude_bridge.stop_hook")

_TRUTHY = {"1", "true", "yes", "on"}

# How long to wait for the just-finished turn's final message to hit the
# transcript. The Stop hook can fire a beat before Claude flushes the closing
# assistant line, so a naive read returns the PREVIOUS turn's answer (or nothing
# on turn 1). We poll until this turn's answer lands.
_FLUSH_WAIT_S = 5.0
_FLUSH_POLL_S = 0.25


def _assistant_text(obj: dict) -> str:
    """Concatenated text blocks of one assistant transcript entry ('' if none)."""
    if obj.get("type") != "assistant":
        return ""
    content = obj.get("message", {}).get("content")
    if isinstance(content, list):
        parts = [
            b.get("text", "")
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        ]
        return "\n".join(p for p in parts if p).strip()
    if isinstance(content, str):
        return content.strip()
    return ""


def _is_human_turn(obj: dict) -> bool:
    """True for a genuine user prompt, False for tool_result plumbing.

    The transcript records tool results as ``user`` entries too; those are not
    human turns. We anchor on the last real human turn to know which assistant
    text belongs to the turn that just finished."""
    if obj.get("type") != "user":
        return False
    content = obj.get("message", {}).get("content")
    if isinstance(content, list):
        return not any(
            isinstance(b, dict) and b.get("type") == "tool_result" for b in content
        )
    return True  # string content == a typed human message


def _answer_for_last_turn(transcript_path: str) -> str | None:
    """Final assistant text produced AFTER the most recent human turn.

    Anchoring to the last human turn does two things the old whole-file scan
    couldn't: it targets *this* turn's answer (not an earlier one), and it lets
    us detect the flush race — if the turn's answer isn't on disk yet this
    returns None, so the caller can retry."""
    objs: list[dict] = []
    try:
        with open(transcript_path, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    objs.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
    except OSError as exc:
        logger.warning("Could not read transcript %s: %s", transcript_path, exc)
        return None

    last_human = -1
    for i, obj in enumerate(objs):
        if _is_human_turn(obj):
            last_human = i
    if last_human < 0:
        return None

    answer: str | None = None
    for obj in objs[last_human + 1 :]:
        text = _assistant_text(obj)
        if text:
            answer = text  # keep the last non-empty text block of this turn
    return answer


def _final_message(transcript_path: str) -> str | None:
    """Read the finished turn's answer, polling past the transcript-flush race."""
    deadline = time.monotonic() + _FLUSH_WAIT_S
    while True:
        answer = _answer_for_last_turn(transcript_path)
        if answer or time.monotonic() >= deadline:
            return answer
        time.sleep(_FLUSH_POLL_S)


def main() -> None:
    # Never break the Claude session: swallow everything and always exit 0.
    try:
        if os.environ.get("CLAUDE_SLACK_NOTIFY", "").strip().lower() not in _TRUTHY:
            return  # opt-in only — stay quiet for ordinary sessions

        raw = sys.stdin.read()
        data = json.loads(raw) if raw.strip() else {}
        session_id = data.get("session_id")
        transcript = data.get("transcript_path")

        text = _final_message(transcript) if transcript else None
        if not text:
            text = "(Claude session finished, but no final message was found.)"

        config = Config.load()
        channel = os.environ.get("SLACK_NOTIFY_CHANNEL", "").strip() or config.notify_channel
        if not channel:
            logger.warning("No SLACK_NOTIFY_CHANNEL configured; skipping Slack push.")
            return

        store = SessionStore(config.db_path)
        push_result(
            config=config,
            store=store,
            channel=channel,
            session_id=session_id,
            text=text,
            is_error=False,
            title="✅ *Claude session done*",
        )
    except Exception:  # noqa: BLE001 - a hook must never crash the session
        logger.exception("stop hook failed (ignored)")


if __name__ == "__main__":
    main()
