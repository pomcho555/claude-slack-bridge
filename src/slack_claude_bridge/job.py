"""Outbound entry points: start a Claude job locally and push its result into
Slack so you can continue it from your phone.

- ``slack-claude-job "<prompt>"``  — run a prompt headless via the same
  ClaudeRunner the bridge uses, then post the result to Slack and seed the
  thread with the new session id.
- ``slack-claude-notify --session <id>`` — push arbitrary text for an existing
  session (for a Claude Code Stop hook, or any other integration).

Both create a Slack thread mapped to a Claude session, so replying in that
thread (mention the bot) resumes it via --resume.
"""

from __future__ import annotations

import argparse
import logging
import sys

from .claude_runner import ClaudeRunner
from .config import Config, ConfigError
from .notify import push_result
from .store import SessionStore

logger = logging.getLogger("slack_claude_bridge.job")


def _resolve_channel(config: Config, override: str | None) -> str:
    channel = override or config.notify_channel
    if not channel:
        raise SystemExit(
            "No target channel. Pass --channel C0123 or set SLACK_NOTIFY_CHANNEL in .env."
        )
    return channel


def run_job_main() -> None:
    """`slack-claude-job` — run a prompt headless, then push the result."""
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")

    parser = argparse.ArgumentParser(
        prog="slack-claude-job",
        description="Run a Claude Code job locally and post the result to Slack.",
    )
    parser.add_argument("prompt", nargs="+", help="The task for Claude (quote it).")
    parser.add_argument("--channel", help="Slack channel ID to post to (overrides SLACK_NOTIFY_CHANNEL).")
    parser.add_argument("--workdir", help="Directory Claude runs in (overrides CLAUDE_WORKDIR).")
    parser.add_argument("--title", help="Custom header for the Slack message.")
    args = parser.parse_args()

    try:
        config = Config.load()
    except ConfigError as exc:
        raise SystemExit(f"Configuration error: {exc}")

    channel = _resolve_channel(config, args.channel)
    prompt = " ".join(args.prompt)

    runner = ClaudeRunner(
        binary=config.claude_bin,
        workdir=args.workdir or config.workdir,
        permission_mode=config.permission_mode,
        model=config.model,
        extra_args=config.extra_args,
        timeout=config.timeout,
    )

    logger.info("Running Claude job (this may take a while)…")
    result = runner.run_new(prompt)

    store = SessionStore(config.db_path)
    thread_ts = push_result(
        config=config,
        store=store,
        channel=channel,
        session_id=result.session_id,
        text=result.text,
        is_error=result.is_error,
        title=args.title,
    )
    print(f"Posted to {channel} (thread_ts={thread_ts}, session={result.session_id}).")
    if result.is_error:
        raise SystemExit(1)


def notify_main() -> None:
    """`slack-claude-notify` — push text for an existing session id."""
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")

    parser = argparse.ArgumentParser(
        prog="slack-claude-notify",
        description="Post text to Slack for an existing Claude session and seed the thread.",
    )
    parser.add_argument("--session", required=True, help="Claude session id to resume on reply.")
    parser.add_argument("--channel", help="Slack channel ID (overrides SLACK_NOTIFY_CHANNEL).")
    parser.add_argument("--title", help="Custom header for the Slack message.")
    parser.add_argument("--error", action="store_true", help="Mark the result as an error.")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--text", help="Result text. If omitted, read from stdin.")
    group.add_argument("--file", help="Read result text from this file.")
    args = parser.parse_args()

    try:
        config = Config.load()
    except ConfigError as exc:
        raise SystemExit(f"Configuration error: {exc}")

    channel = _resolve_channel(config, args.channel)

    if args.text is not None:
        text = args.text
    elif args.file:
        with open(args.file, encoding="utf-8") as fh:
            text = fh.read()
    else:
        text = sys.stdin.read()

    store = SessionStore(config.db_path)
    thread_ts = push_result(
        config=config,
        store=store,
        channel=channel,
        session_id=args.session,
        text=text,
        is_error=args.error,
        title=args.title,
    )
    print(f"Posted to {channel} (thread_ts={thread_ts}, session={args.session}).")
