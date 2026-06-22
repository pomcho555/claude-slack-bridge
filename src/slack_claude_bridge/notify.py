from __future__ import annotations

import logging

from slack_sdk import WebClient

from .config import Config
from .store import SessionStore

logger = logging.getLogger("slack_claude_bridge.notify")

# Match app.py: Slack hard-limits a single text block; upload anything larger.
MAX_TEXT = 3500


def push_result(
    *,
    config: Config,
    store: SessionStore,
    channel: str,
    session_id: str | None,
    text: str,
    is_error: bool = False,
    title: str | None = None,
) -> str:
    """Post a Claude job result into Slack as a NEW thread root and record
    thread_ts -> session_id, so a reply in that thread continues the same
    Claude session via --resume.

    This is the outbound counterpart to app.py: there a Slack mention starts a
    job; here a job that was started locally pushes its result into Slack. Once
    the thread is seeded, the existing inbound handlers continue it.

    Returns the thread_ts of the posted message.
    """
    client = WebClient(token=config.bot_token)

    header = title or ("❌ *Claude job failed*" if is_error else "✅ *Claude job done*")
    body = text.strip() or "(no result text)"
    long = len(body) > MAX_TEXT

    # If this session is already mapped to a thread (e.g. a Stop hook firing on
    # an earlier turn of the same interactive session), post into THAT thread
    # instead of creating a new root — one session stays one Slack thread.
    existing = store.find_by_session(session_id)
    target_channel = existing.channel if existing else channel

    first_text = f"{header}\n{body}" if not long else f"{header}\n_Result is long — see the attached file._"
    resp = client.chat_postMessage(
        channel=target_channel,
        thread_ts=existing.thread_ts if existing else None,
        text=first_text,
    )
    # When posting into an existing thread, resp["ts"] is the child message; the
    # thread root (our key) is the one we already had.
    thread_ts = existing.thread_ts if existing else resp["ts"]

    if long:
        client.files_upload_v2(
            channel=target_channel,
            thread_ts=thread_ts,
            filename="claude-result.md",
            title="Claude result",
            content=body,
        )

    # Seed (or refresh) the thread so an in-thread reply maps back to this session.
    if not existing:
        store.start(thread_ts, target_channel)
    store.finish(thread_ts, session_id, "error" if is_error else "done")
    logger.info(
        "Pushed result to %s (thread_ts=%s, session=%s, reused=%s)",
        target_channel,
        thread_ts,
        session_id,
        bool(existing),
    )
    return thread_ts
