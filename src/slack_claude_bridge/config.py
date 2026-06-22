from __future__ import annotations

import os
from dataclasses import dataclass

from dotenv import load_dotenv

# Load .env once at import time so every module sees the same environment.
load_dotenv()


class ConfigError(RuntimeError):
    """Raised when required configuration is missing or invalid."""


def _require(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise ConfigError(f"Missing required environment variable: {name}")
    return value


@dataclass(frozen=True)
class Config:
    bot_token: str
    app_token: str
    claude_bin: str
    workdir: str
    permission_mode: str
    model: str | None
    extra_args: list[str]
    timeout: int
    db_path: str
    allowed_users: set[str]
    notify_channel: str | None

    @classmethod
    def load(cls) -> "Config":
        allowed_raw = os.environ.get("ALLOWED_USERS", "")
        allowed_users = {u.strip() for u in allowed_raw.split(",") if u.strip()}

        extra_raw = os.environ.get("CLAUDE_EXTRA_ARGS", "").strip()
        extra_args = extra_raw.split() if extra_raw else []

        timeout_raw = os.environ.get("CLAUDE_TIMEOUT", "14400").strip() or "14400"
        try:
            timeout = int(timeout_raw)
        except ValueError as exc:
            raise ConfigError(f"CLAUDE_TIMEOUT must be an integer, got {timeout_raw!r}") from exc

        return cls(
            bot_token=_require("SLACK_BOT_TOKEN"),
            app_token=_require("SLACK_APP_TOKEN"),
            claude_bin=os.environ.get("CLAUDE_BIN", "claude").strip() or "claude",
            workdir=os.environ.get("CLAUDE_WORKDIR", "").strip() or os.getcwd(),
            permission_mode=os.environ.get("CLAUDE_PERMISSION_MODE", "acceptEdits").strip()
            or "acceptEdits",
            model=os.environ.get("CLAUDE_MODEL", "").strip() or None,
            extra_args=extra_args,
            timeout=timeout,
            db_path=os.environ.get("DB_PATH", "bridge.db").strip() or "bridge.db",
            allowed_users=allowed_users,
            notify_channel=os.environ.get("SLACK_NOTIFY_CHANNEL", "").strip() or None,
        )
