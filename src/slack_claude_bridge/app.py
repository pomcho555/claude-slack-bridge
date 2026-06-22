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


class SlackPoster:
    """The outbound Slack surface the bridge core writes through.

    Going through this small interface (instead of calling ``app.client``
    directly) is what makes the core flows testable without a live Slack, and
    it is the seam a future rewrite re-implements.
    """

    def __init__(self, client) -> None:
        self._client = client

    def post(self, channel: str, thread_ts: str, text: str) -> None:
        self._client.chat_postMessage(channel=channel, thread_ts=thread_ts, text=text)

    def upload(self, channel: str, thread_ts: str, filename: str, title: str, content: str) -> None:
        self._client.files_upload_v2(
            channel=channel,
            thread_ts=thread_ts,
            filename=filename,
            title=title,
            content=content,
        )


def post_result(poster: SlackPoster, channel: str, thread_ts: str, result: ClaudeResult) -> None:
    prefix = "❌ *Error*\n" if result.is_error else "✅ *Done*\n"
    if len(result.text) <= MAX_TEXT:
        poster.post(channel, thread_ts, prefix + result.text)
        return
    poster.post(channel, thread_ts, prefix + "_Result is long — see the attached file._")
    poster.upload(channel, thread_ts, "claude-result.md", "Claude result", result.text)


def is_allowed(config: Config, user: str | None) -> bool:
    if not config.allowed_users:
        return True
    return user in config.allowed_users


def run_new_job(
    poster: SlackPoster,
    store: SessionStore,
    runner: ClaudeRunner,
    channel: str,
    thread_ts: str,
    prompt: str,
) -> None:
    store.start(thread_ts, channel)
    try:
        result = runner.run_new(prompt)
        store.finish(thread_ts, result.session_id, "error" if result.is_error else "done")
        post_result(poster, channel, thread_ts, result)
    except Exception as exc:  # noqa: BLE001 - report any failure back to Slack
        logger.exception("new job failed")
        store.finish(thread_ts, None, "error")
        poster.post(channel, thread_ts, f"❌ Bridge error: {exc}")


def run_reply(
    poster: SlackPoster,
    store: SessionStore,
    runner: ClaudeRunner,
    channel: str,
    thread_ts: str,
    prompt: str,
) -> None:
    row = store.get(thread_ts)
    if row is None or not row.session_id:
        poster.post(channel, thread_ts, "⚠️ No Claude session for this thread yet — ignoring.")
        return
    try:
        result = runner.run_resume(row.session_id, prompt)
        store.finish(thread_ts, result.session_id, "error" if result.is_error else "done")
        post_result(poster, channel, thread_ts, result)
    except Exception as exc:  # noqa: BLE001
        logger.exception("reply failed")
        store.finish(thread_ts, None, "error")
        poster.post(channel, thread_ts, f"❌ Bridge error: {exc}")


def handle_mention(
    event: dict,
    *,
    poster: SlackPoster,
    config: Config,
    store: SessionStore,
    runner: ClaudeRunner,
    workers: ThreadWorkers,
    bot_user_id: str,
) -> None:
    user = event.get("user")
    if not is_allowed(config, user):
        logger.info("Ignoring mention from disallowed user %s", user)
        return

    # Anchor the conversation at the thread root (or this message if it starts a
    # new thread) so all follow-up replies map to one session.
    thread_ts = event.get("thread_ts") or event["ts"]
    channel = event["channel"]
    prompt = _strip_mentions(event.get("text", ""))

    if not prompt:
        poster.post(channel, thread_ts, "👋 Mention me with a task to run.")
        return

    # A mention inside an already-tracked thread = continue that session.
    if store.exists(thread_ts):
        poster.post(channel, thread_ts, "💬 Continuing this session…")
        workers.submit(thread_ts, run_reply, poster, store, runner, channel, thread_ts, prompt)
    else:
        poster.post(
            channel,
            thread_ts,
            "🛠 Started — I'll reply in this thread when done. Reply here anytime to continue.",
        )
        workers.submit(thread_ts, run_new_job, poster, store, runner, channel, thread_ts, prompt)


def handle_message(
    event: dict,
    *,
    poster: SlackPoster,
    config: Config,
    store: SessionStore,
    runner: ClaudeRunner,
    workers: ThreadWorkers,
    bot_user_id: str,
) -> None:
    # Only plain human messages: skip edits/deletes/joins and bot posts.
    if event.get("subtype") or event.get("bot_id"):
        return
    # Mentions are handled by handle_mention; avoid double-processing.
    if f"<@{bot_user_id}>" in event.get("text", ""):
        return

    thread_ts = event.get("thread_ts")
    # Must be a reply inside a thread we own; ignore top-level chatter and the
    # thread-root message itself.
    if not thread_ts or thread_ts == event.get("ts"):
        return
    if not store.exists(thread_ts):
        return

    user = event.get("user")
    if not is_allowed(config, user):
        return

    prompt = _strip_mentions(event.get("text", ""))
    if not prompt:
        return

    channel = event["channel"]
    workers.submit(thread_ts, run_reply, poster, store, runner, channel, thread_ts, prompt)


def register_handlers(
    app: App,
    config: Config,
    store: SessionStore,
    runner: ClaudeRunner,
    workers: ThreadWorkers,
    bot_user_id: str,
) -> None:
    poster = SlackPoster(app.client)

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

    @app.event("app_mention")
    def on_mention(event):
        handle_mention(
            event,
            poster=poster,
            config=config,
            store=store,
            runner=runner,
            workers=workers,
            bot_user_id=bot_user_id,
        )

    @app.event("message")
    def on_message(event):
        handle_message(
            event,
            poster=poster,
            config=config,
            store=store,
            runner=runner,
            workers=workers,
            bot_user_id=bot_user_id,
        )


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
