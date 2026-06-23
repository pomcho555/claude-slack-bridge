//! Language-specific stop-hook timing NOT covered by the shared spec: the
//! transcript-flush race. The Stop hook can fire a beat before Claude flushes
//! the closing assistant line; `final_message` must poll until this turn's
//! answer lands rather than posting nothing (or the previous turn's answer).
//!
//! Mirror of `tests/test_stop_hook.py::test_waits_for_flush_then_posts`.

use std::cell::RefCell;
use std::io::Write;
use std::thread;
use std::time::Duration;

use serde_json::json;

use slack_claude_bridge::notify::SlackClient;
use slack_claude_bridge::stop_hook;
use slack_claude_bridge::store::SessionStore;

#[derive(Default)]
struct FakeSlack {
    posts: RefCell<Vec<(String, Option<String>, String)>>,
}

impl SlackClient for FakeSlack {
    fn chat_post_message(&self, channel: &str, thread_ts: Option<&str>, text: &str) -> String {
        self.posts.borrow_mut().push((
            channel.to_string(),
            thread_ts.map(String::from),
            text.to_string(),
        ));
        if thread_ts.is_some() {
            "child.0".into()
        } else {
            "root.0".into()
        }
    }
    fn files_upload_v2(&self, _c: &str, _t: Option<&str>, _f: &str, _ti: &str, _ct: &str) {}
}

#[test]
fn waits_for_flush_then_posts() {
    let dir = std::env::temp_dir().join("rust-flush-race");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let store = SessionStore::new(dir.join("bridge.db").to_str().unwrap());
    let transcript = dir.join("t.jsonl");

    // Start with ONLY the human turn — this turn's assistant answer has not been
    // flushed yet, so a naive read would find nothing.
    let human = json!({"type": "user", "message": {"content": "do the thing"}});
    std::fs::write(&transcript, format!("{human}\n")).unwrap();

    // Flush the answer shortly after the hook starts polling.
    let t_path = transcript.clone();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        let answer = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "DELAYED ANSWER"}]}});
        let mut fh = std::fs::OpenOptions::new()
            .append(true)
            .open(&t_path)
            .unwrap();
        writeln!(fh, "{answer}").unwrap();
    });

    let payload = json!({
        "session_id": "sess-race",
        "transcript_path": transcript.to_str().unwrap(),
    })
    .to_string();

    let client = FakeSlack::default();
    // Blocks while polling past the flush race, then posts this turn's answer.
    stop_hook::run(&client, &store, true, Some("C_NOTIFY"), &payload);
    writer.join().unwrap();

    let posts = client.posts.borrow();
    assert_eq!(posts.len(), 1, "expected exactly one post, got {posts:?}");
    assert!(
        posts[0].2.contains("DELAYED ANSWER"),
        "should post the flushed answer, got: {:?}",
        posts[0].2
    );
    assert!(
        !posts[0].2.contains("no final message"),
        "should not post the no-message fallback"
    );
}
