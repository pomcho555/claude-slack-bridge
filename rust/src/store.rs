//! Persistent map of Slack thread -> Claude session (mirror of `store.py`).
//!
//! One Slack thread corresponds to one Claude session, so a human reply in the
//! thread continues the same conversation via `--resume`.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone)]
pub struct ThreadRow {
    pub thread_ts: String,
    pub channel: String,
    pub session_id: Option<String>,
    pub status: String, // running | done | error
    pub updated_at: f64,
}

pub struct SessionStore {
    conn: Mutex<Connection>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn row_from(row: &rusqlite::Row) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        thread_ts: row.get("thread_ts")?,
        channel: row.get("channel")?,
        session_id: row.get("session_id")?,
        status: row.get("status")?,
        updated_at: row.get("updated_at")?,
    })
}

impl SessionStore {
    pub fn new(path: &str) -> SessionStore {
        let conn = Connection::open(path).expect("open sqlite");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS threads (
                 thread_ts  TEXT PRIMARY KEY,
                 channel    TEXT NOT NULL,
                 session_id TEXT,
                 status     TEXT NOT NULL,
                 updated_at REAL NOT NULL
             )",
            [],
        )
        .expect("create table");
        SessionStore { conn: Mutex::new(conn) }
    }

    pub fn get(&self, thread_ts: &str) -> Option<ThreadRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT * FROM threads WHERE thread_ts = ?1",
            params![thread_ts],
            row_from,
        )
        .optional()
        .expect("query get")
    }

    pub fn exists(&self, thread_ts: &str) -> bool {
        self.get(thread_ts).is_some()
    }

    /// Reverse lookup: the thread currently mapped to this Claude session.
    /// Returns the most recently updated match, or None.
    pub fn find_by_session(&self, session_id: Option<&str>) -> Option<ThreadRow> {
        let session_id = session_id?;
        if session_id.is_empty() {
            return None;
        }
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT * FROM threads WHERE session_id = ?1 ORDER BY updated_at DESC LIMIT 1",
            params![session_id],
            row_from,
        )
        .optional()
        .expect("query find_by_session")
    }

    /// Register a thread as running, before the (possibly multi-hour) Claude run
    /// produces a session id.
    pub fn start(&self, thread_ts: &str, channel: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO threads (thread_ts, channel, session_id, status, updated_at)
             VALUES (?1, ?2, NULL, 'running', ?3)
             ON CONFLICT(thread_ts) DO UPDATE SET status='running', updated_at=excluded.updated_at",
            params![thread_ts, channel, now()],
        )
        .expect("start");
    }

    /// Record the outcome. `session_id` is COALESCEd so a None from a failed
    /// resume never wipes a known id.
    pub fn finish(&self, thread_ts: &str, session_id: Option<&str>, status: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE threads
                SET session_id = COALESCE(?1, session_id),
                    status = ?2,
                    updated_at = ?3
              WHERE thread_ts = ?4",
            params![session_id, status, now(), thread_ts],
        )
        .expect("finish");
    }
}
