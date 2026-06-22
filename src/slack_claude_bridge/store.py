from __future__ import annotations

import sqlite3
import threading
import time
from dataclasses import dataclass


@dataclass
class ThreadRow:
    thread_ts: str
    channel: str
    session_id: str | None
    status: str  # running | done | error
    updated_at: float


class SessionStore:
    """Persistent map of Slack thread -> Claude session.

    A single Slack thread corresponds to a single Claude Code session, so a
    human reply in the thread continues the same conversation via --resume.
    """

    def __init__(self, path: str):
        # check_same_thread=False because Bolt dispatches handlers on worker
        # threads; we serialize all access with our own lock.
        self._conn = sqlite3.connect(path, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._lock = threading.Lock()
        self._init()

    def _init(self) -> None:
        with self._lock:
            self._conn.execute(
                """
                CREATE TABLE IF NOT EXISTS threads (
                    thread_ts  TEXT PRIMARY KEY,
                    channel    TEXT NOT NULL,
                    session_id TEXT,
                    status     TEXT NOT NULL,
                    updated_at REAL NOT NULL
                )
                """
            )
            self._conn.commit()

    def get(self, thread_ts: str) -> ThreadRow | None:
        with self._lock:
            cur = self._conn.execute(
                "SELECT * FROM threads WHERE thread_ts = ?", (thread_ts,)
            )
            row = cur.fetchone()
        if row is None:
            return None
        return ThreadRow(
            thread_ts=row["thread_ts"],
            channel=row["channel"],
            session_id=row["session_id"],
            status=row["status"],
            updated_at=row["updated_at"],
        )

    def exists(self, thread_ts: str) -> bool:
        return self.get(thread_ts) is not None

    def find_by_session(self, session_id: str | None) -> ThreadRow | None:
        """Reverse lookup: the thread currently mapped to this Claude session.

        Used by the outbound push so repeated notifications for the SAME session
        (e.g. a Stop hook firing on every turn of one interactive terminal
        session) land in the one existing thread instead of spawning a fresh
        root each time. Returns the most recently updated match, or None."""
        if not session_id:
            return None
        with self._lock:
            cur = self._conn.execute(
                "SELECT * FROM threads WHERE session_id = ? ORDER BY updated_at DESC LIMIT 1",
                (session_id,),
            )
            row = cur.fetchone()
        if row is None:
            return None
        return ThreadRow(
            thread_ts=row["thread_ts"],
            channel=row["channel"],
            session_id=row["session_id"],
            status=row["status"],
            updated_at=row["updated_at"],
        )

    def start(self, thread_ts: str, channel: str) -> None:
        """Register a thread as running. Called the moment a job is accepted,
        before the (possibly multi-hour) Claude run produces a session id."""
        now = time.time()
        with self._lock:
            self._conn.execute(
                """
                INSERT INTO threads (thread_ts, channel, session_id, status, updated_at)
                VALUES (?, ?, NULL, 'running', ?)
                ON CONFLICT(thread_ts) DO UPDATE SET status='running', updated_at=excluded.updated_at
                """,
                (thread_ts, channel, now),
            )
            self._conn.commit()

    def finish(self, thread_ts: str, session_id: str | None, status: str) -> None:
        """Record the outcome. session_id is COALESCEd so we never wipe a known
        id with a None from a failed resume."""
        with self._lock:
            self._conn.execute(
                """
                UPDATE threads
                   SET session_id = COALESCE(?, session_id),
                       status = ?,
                       updated_at = ?
                 WHERE thread_ts = ?
                """,
                (session_id, status, time.time(), thread_ts),
            )
            self._conn.commit()
