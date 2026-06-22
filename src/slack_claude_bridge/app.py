from __future__ import annotations

import logging
import re
import threading
from concurrent.futures import ThreadPoolExecutor

from slack_bolt import App
from slack_bolt.adapter.socket_mode import SocketModeHandler

from .claude_runner import ClaudeResult, ClaudeRunner
from .config import Config, ConfigError
from .store import SessionStore

logger = logging.getLogger("slack_claude_bridge")

# Slack hard-limits a single text block; keep margin and upload anything larger.
MAX_TEXT = 3500

_MENTION_RE = re.compile(r"<@[A-Z0-9]+>")


def _strip_mentions(text: str) -> str:
    return _MENTION_RE.sub("", text or "").strip()


class ThreadWorkers:
    """One single-thread executor per Slack thread.

    Effect: messages within the same thread run sequentially (a reply that
    arrives while a job is still running simply queues behind it, so we never
    --resume a session concurrently), while different threads run in parallel.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._pools: dict[str, ThreadPoolExecutor] = {}

    def submit(self, key: str, fn, *args) -> None:
        with self._lock:
            pool = self._pools.get(key)
            if pool is None:
                pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="claude")
                self._pools[key] = pool
        pool.submit(fn, *args)


def register_handlers(
    app: App,
    config: Config,
    store: SessionStore,
    runner: ClaudeRunner,
    workers: ThreadWorkers,
    bot_user_id: str,
) -> None:
    @app.middleware
    def _log_every_event(logger, body, next):  # noqa: A002 - Bolt's arg name
        event = body.get("event", {}) if isinstance(body, dict) else {}
        logger.info(
            "INCOMING type=%s channel=%s user=%s text=%r",
            event.get("type"),
            event.get("channel"),
            event.get("user"),
            (event.get("text") or "")[:80],
        )
        next()

    def post(channel: str, thread_ts: str, text: str) -> None:
        app.client.chat_postMessage(channel=channel, thread_ts=thread_ts, text=text)

    def post_result(channel: str, thread_ts: str, result: ClaudeResult) -> None:
        prefix = "❌ *Error*\n" if result.is_error else "✅ *Done*\n"
        if len(result.text) <= MAX_TEXT:
            post(channel, thread_ts, prefix + result.text)
            return
        post(channel, thread_ts, prefix + "_Result is long — see the attached file._")
        app.client.files_upload_v2(
            channel=channel,
            thread_ts=thread_ts,
            filename="claude-result.md",
            title="Claude result",
            content=result.text,
        )

    def is_allowed(user: str | None) -> bool:
        if not config.allowed_users:
            return True
        return user in config.allowed_users

    def run_new_job(channel: str, thread_ts: str, prompt: str) -> None:
        store.start(thread_ts, channel)
        try:
            result = runner.run_new(prompt)
            store.finish(thread_ts, result.session_id, "error" if result.is_error else "done")
            post_result(channel, thread_ts, result)
        except Exception as exc:  # noqa: BLE001 - report any failure back to Slack
            logger.exception("new job failed")
            store.finish(thread_ts, None, "error")
            post(channel, thread_ts, f"❌ Bridge error: {exc}")

    def run_reply(channel: str, thread_ts: str, prompt: str) -> None:
        row = store.get(thread_ts)
        if row is None or not row.session_id:
            post(channel, thread_ts, "⚠️ No Claude session for this thread yet — ignoring.")
            return
        try:
            result = runner.run_resume(row.session_id, prompt)
            store.finish(thread_ts, result.session_id, "error" if result.is_error else "done")
            post_result(channel, thread_ts, result)
        except Exception as exc:  # noqa: BLE001
            logger.exception("reply failed")
            store.finish(thread_ts, None, "error")
            post(channel, thread_ts, f"❌ Bridge error: {exc}")

    @app.event("app_mention")
    def on_mention(event, say):
        user = event.get("user")
        if not is_allowed(user):
            logger.info("Ignoring mention from disallowed user %s", user)
            return

        # Anchor the conversation at the thread root (or this message if it
        # starts a new thread) so all follow-up replies map to one session.
        thread_ts = event.get("thread_ts") or event["ts"]
        channel = event["channel"]
        prompt = _strip_mentions(event.get("text", ""))

        if not prompt:
            say(channel=channel, thread_ts=thread_ts, text="👋 Mention me with a task to run.")
            return

        # A mention inside an already-tracked thread = continue that session.
        if store.exists(thread_ts):
            say(channel=channel, thread_ts=thread_ts, text="💬 Continuing this session…")
            workers.submit(thread_ts, run_reply, channel, thread_ts, prompt)
        else:
            say(
                channel=channel,
                thread_ts=thread_ts,
                text="🛠 Started — I'll reply in this thread when done. "
                "Reply here anytime to continue.",
            )
            workers.submit(thread_ts, run_new_job, channel, thread_ts, prompt)

    @app.event("message")
    def on_message(event):
        # Only plain human messages: skip edits/deletes/joins and bot posts.
        if event.get("subtype") or event.get("bot_id"):
            return
        # Mentions are handled by on_mention; avoid double-processing.
        if f"<@{bot_user_id}>" in event.get("text", ""):
            return

        thread_ts = event.get("thread_ts")
        # Must be a reply inside a thread we own; ignore top-level chatter and
        # the thread-root message itself.
        if not thread_ts or thread_ts == event.get("ts"):
            return
        if not store.exists(thread_ts):
            return

        user = event.get("user")
        if not is_allowed(user):
            return

        prompt = _strip_mentions(event.get("text", ""))
        if not prompt:
            return

        channel = event["channel"]
        workers.submit(thread_ts, run_reply, channel, thread_ts, prompt)


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    try:
        config = Config.load()
    except ConfigError as exc:
        raise SystemExit(f"Configuration error: {exc}")

    store = SessionStore(config.db_path)
    runner = ClaudeRunner(
        binary=config.claude_bin,
        workdir=config.workdir,
        permission_mode=config.permission_mode,
        model=config.model,
        extra_args=config.extra_args,
        timeout=config.timeout,
    )
    workers = ThreadWorkers()

    app = App(token=config.bot_token, logger=logger)

    auth = app.client.auth_test()
    bot_user_id = auth["user_id"]
    logger.info("Authenticated as %s (%s)", auth.get("user"), bot_user_id)
    logger.info("Claude workdir: %s | permission-mode: %s", config.workdir, config.permission_mode)
    if not config.allowed_users:
        logger.warning("ALLOWED_USERS is empty — everyone in the workspace can run Claude!")

    register_handlers(app, config, store, runner, workers, bot_user_id)

    logger.info("Starting Socket Mode handler — waiting for Slack events…")
    SocketModeHandler(app, config.app_token).start()


if __name__ == "__main__":
    main()
