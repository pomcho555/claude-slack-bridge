"""Unit-level guarantees of the thread<->session map the whole bridge relies on."""
from __future__ import annotations


def test_start_then_finish_records_session(store):
    store.start("10.0", "C1")
    assert store.get("10.0").session_id is None  # running, id not known yet
    store.finish("10.0", "sess-1", "done")
    row = store.get("10.0")
    assert row.session_id == "sess-1" and row.status == "done"


def test_finish_coalesces_session_id(store):
    # A failed resume must never wipe a known session id with None.
    store.start("11.0", "C1")
    store.finish("11.0", "sess-1", "done")
    store.finish("11.0", None, "error")
    assert store.get("11.0").session_id == "sess-1"


def test_find_by_session_returns_latest_thread(store):
    store.start("a", "C1")
    store.finish("a", "sess-x", "done")
    assert store.find_by_session("sess-x").thread_ts == "a"
    assert store.find_by_session("nope") is None
    assert store.find_by_session(None) is None


def test_exists(store):
    assert not store.exists("z")
    store.start("z", "C1")
    assert store.exists("z")
