//! Reference harness that runs the language-neutral spec (`../spec/scenarios.json`)
//! against the Rust bridge — the SAME scenarios the Python reference harness
//! (`tests/test_spec.py`) runs. The fake `claude` CLI (`../tests/fake_claude.py`)
//! is shared verbatim; this crate just points the runner at it.
//!
//! Each scenario runs independently and is reported PASS/FAIL; the test fails
//! listing every scenario that failed. While the behavior layer is stubbed the
//! whole suite is RED — that is the target definition for the port.

use std::cell::RefCell;
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use slack_claude_bridge::app::{self, Event, Poster, Workers};
use slack_claude_bridge::claude_runner::ClaudeRunner;
use slack_claude_bridge::config::Config;
use slack_claude_bridge::notify::SlackClient;
use slack_claude_bridge::stop_hook;
use slack_claude_bridge::store::SessionStore;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn spec() -> Value {
    let path = repo_root().join("spec").join("scenarios.json");
    let raw = std::fs::read_to_string(&path).expect("read scenarios.json");
    serde_json::from_str(&raw).expect("parse scenarios.json")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rust-spec-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn panic_msg(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

// --- shared fakes ----------------------------------------------------------

type RecordedPosts = Arc<Mutex<Vec<(String, Option<String>, String)>>>;
type RecordedUploads = Arc<Mutex<Vec<(String, String)>>>; // (filename, content)

// Clone-able + Send so it satisfies the handlers' `Poster + Clone + Send`
// bound; clones share the same recorded posts/uploads via Arc.
#[derive(Default, Clone)]
struct FakePoster {
    posts: RecordedPosts,
    uploads: RecordedUploads,
}

impl Poster for FakePoster {
    fn post(&self, channel: &str, thread_ts: Option<&str>, text: &str) {
        self.posts.lock().unwrap().push((
            channel.to_string(),
            thread_ts.map(String::from),
            text.to_string(),
        ));
    }
    fn upload(
        &self,
        _channel: &str,
        _thread_ts: Option<&str>,
        filename: &str,
        _title: &str,
        content: &str,
    ) {
        self.uploads
            .lock()
            .unwrap()
            .push((filename.to_string(), content.to_string()));
    }
}

struct InlineWorkers;
impl Workers for InlineWorkers {
    fn submit(&self, _key: &str, job: Box<dyn FnOnce() + Send + 'static>) {
        job();
    }
}

#[derive(Default)]
struct FakeSlack {
    posts: RefCell<Vec<(String, Option<String>, String)>>,
    uploads: RefCell<Vec<(String, String)>>,
}

impl SlackClient for FakeSlack {
    fn chat_post_message(&self, channel: &str, thread_ts: Option<&str>, text: &str) -> String {
        self.posts.borrow_mut().push((
            channel.to_string(),
            thread_ts.map(String::from),
            text.to_string(),
        ));
        if thread_ts.is_some() {
            "child.0".to_string()
        } else {
            "root.0".to_string()
        }
    }
    fn files_upload_v2(
        &self,
        _c: &str,
        _t: Option<&str>,
        filename: &str,
        _title: &str,
        content: &str,
    ) {
        self.uploads
            .borrow_mut()
            .push((filename.to_string(), content.to_string()));
    }
}

// --- shared assertions -----------------------------------------------------

fn match_post(actual: &(String, Option<String>, String), expected: &Value) {
    let (channel, thread_ts, text) = actual;
    if let Some(c) = expected.get("channel").and_then(|v| v.as_str()) {
        assert_eq!(channel, c, "channel mismatch");
    }
    if let Some(exp) = expected.get("thread_ts") {
        if exp.is_null() {
            assert!(
                thread_ts.is_none(),
                "thread_ts expected null, got {thread_ts:?}"
            );
        } else {
            assert_eq!(thread_ts.as_deref(), exp.as_str(), "thread_ts mismatch");
        }
    }
    if let Some(tc) = expected.get("text_contains").and_then(|v| v.as_str()) {
        assert!(text.contains(tc), "{tc:?} not in {text:?}");
    }
    if let Some(tn) = expected.get("text_not_contains").and_then(|v| v.as_str()) {
        assert!(!text.contains(tn), "{tn:?} unexpectedly in {text:?}");
    }
}

fn assert_posts(posts: &[(String, Option<String>, String)], expect: &Value) {
    if let Some(exp) = expect.get("posts").and_then(|v| v.as_array()) {
        assert_eq!(
            posts.len(),
            exp.len(),
            "expected {} posts, got {}: {posts:?}",
            exp.len(),
            posts.len()
        );
        for (actual, expected) in posts.iter().zip(exp) {
            match_post(actual, expected);
        }
    }
    if let Some(any) = expect.get("post_any_contains").and_then(|v| v.as_array()) {
        for needle in any {
            let n = needle.as_str().unwrap();
            assert!(
                posts.iter().any(|(_, _, t)| t.contains(n)),
                "no post contains {n:?}: {posts:?}"
            );
        }
    }
}

fn assert_uploads(uploads: &[(String, String)], expect: &Value) {
    let Some(exp) = expect.get("uploads").and_then(|v| v.as_array()) else {
        return;
    };
    assert_eq!(
        uploads.len(),
        exp.len(),
        "expected {} uploads, got {}",
        exp.len(),
        uploads.len()
    );
    for ((filename, content), expected) in uploads.iter().zip(exp) {
        if let Some(f) = expected.get("filename").and_then(|v| v.as_str()) {
            assert_eq!(filename, f, "upload filename mismatch");
        }
        if let Some(min) = expected.get("min_content_len").and_then(|v| v.as_u64()) {
            assert!(content.chars().count() as u64 >= min, "upload too short");
        }
    }
}

// --- inbound (Slack event -> bridge -> Slack) ------------------------------

fn build_event(ev: &Value) -> Event {
    let s = |k: &str| ev.get(k).and_then(|v| v.as_str()).map(String::from);
    Event {
        kind: ev["type"].as_str().unwrap().to_string(),
        user: s("user"),
        bot_id: s("bot_id"),
        ts: s("ts"),
        thread_ts: s("thread_ts"),
        channel: s("channel"),
        text: s("text"),
        subtype: s("subtype"),
    }
}

fn run_inbound(sc: &Value) {
    let dir = scratch(&format!("in-{}", sc["name"].as_str().unwrap()));
    let log = dir.join("claude.jsonl");
    std::env::set_var("FAKE_CLAUDE_LOG", &log);

    let claude = &sc["claude"];
    match claude.get("session").and_then(|v| v.as_str()) {
        Some(s) => std::env::set_var("FAKE_CLAUDE_SESSION", s),
        None => std::env::remove_var("FAKE_CLAUDE_SESSION"),
    }
    let result = if let Some(n) = claude.get("result_len").and_then(|v| v.as_u64()) {
        "X".repeat(n as usize)
    } else {
        claude
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("ok")
            .to_string()
    };
    std::env::set_var("FAKE_CLAUDE_RESULT", &result);
    if claude
        .get("error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        std::env::set_var("FAKE_CLAUDE_ERROR", "1");
    } else {
        std::env::remove_var("FAKE_CLAUDE_ERROR");
    }

    let store = SessionStore::new(dir.join("bridge.db").to_str().unwrap());
    if let Some(seed) = sc.get("seed").and_then(|v| v.as_array()) {
        for s in seed {
            store.start(
                s["thread_ts"].as_str().unwrap(),
                s["channel"].as_str().unwrap(),
            );
            store.finish(
                s["thread_ts"].as_str().unwrap(),
                s["session_id"].as_str(),
                "done",
            );
        }
    }

    let allowed: HashSet<String> = sc["config"]["allowed_users"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let config = Config {
        bot_token: "x".into(),
        app_token: "x".into(),
        claude_bin: "ignored".into(),
        workdir: ".".into(),
        permission_mode: "acceptEdits".into(),
        model: None,
        extra_args: vec![],
        timeout: 10,
        db_path: ":memory:".into(),
        allowed_users: allowed,
        notify_channel: None,
    };
    let runner = ClaudeRunner {
        binary: env!("CARGO_BIN_EXE_fake_claude").to_string(),
        workdir: dir.to_str().unwrap().into(),
        permission_mode: "acceptEdits".into(),
        model: None,
        extra_args: vec![],
        timeout: 10,
    };
    let poster = FakePoster::default();
    let workers = InlineWorkers;

    for ev in sc["events"].as_array().unwrap() {
        let event = build_event(ev);
        if event.kind == "app_mention" {
            app::handle_mention(&event, &poster, &config, &store, &runner, &workers, "UBOT");
        } else {
            app::handle_message(&event, &poster, &config, &store, &runner, &workers, "UBOT");
        }
    }

    let expect = &sc["expect"];
    assert_posts(&poster.posts.lock().unwrap(), expect);
    assert_uploads(&poster.uploads.lock().unwrap(), expect);

    if expect
        .get("no_claude")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let empty = !log.exists()
            || std::fs::read_to_string(&log)
                .map(|s| s.trim().is_empty())
                .unwrap_or(true);
        assert!(empty, "claude was invoked but shouldn't be");
    }

    if let Some(invs) = expect.get("claude_invocations").and_then(|v| v.as_array()) {
        let content = std::fs::read_to_string(&log).unwrap_or_default();
        let lines: Vec<Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for (i, exp_inv) in invs.iter().enumerate() {
            let inv = &lines[i];
            let resume = inv.get("resume").unwrap_or(&Value::Null);
            assert_eq!(resume, exp_inv.get("resume").unwrap(), "resume mismatch");
            if let Some(pc) = exp_inv.get("prompt_contains").and_then(|v| v.as_str()) {
                assert!(
                    inv["prompt"].as_str().unwrap_or("").contains(pc),
                    "prompt mismatch"
                );
            }
        }
    }

    if let Some(store_exp) = expect.get("store").and_then(|v| v.as_object()) {
        for (thread_ts, exp) in store_exp {
            let row = store.get(thread_ts).expect("store row exists");
            assert_eq!(
                row.session_id.as_deref(),
                exp.get("session_id").and_then(|v| v.as_str()),
                "store session_id mismatch"
            );
        }
    }
}

// --- stop hook (finished local session -> Slack) ---------------------------

fn transcript_line(e: &Value) -> Value {
    if let Some(u) = e.get("u").and_then(|v| v.as_str()) {
        return json!({"type": "user", "message": {"content": u}});
    }
    if e.get("tr").is_some() {
        return json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}});
    }
    if e.get("think").is_some() {
        return json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "…"}]}});
    }
    if let Some(tool) = e.get("tool").and_then(|v| v.as_str()) {
        return json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": tool}]}});
    }
    let text = if let Some(n) = e.get("a_len").and_then(|v| v.as_u64()) {
        "Z".repeat(n as usize)
    } else {
        e["a"].as_str().unwrap().to_string()
    };
    json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
}

fn run_stop_hook(sc: &Value) {
    let dir = scratch(&format!("hook-{}", sc["name"].as_str().unwrap()));
    let store = SessionStore::new(dir.join("bridge.db").to_str().unwrap());
    if let Some(seed) = sc.get("seed").and_then(|v| v.as_array()) {
        for s in seed {
            store.start(
                s["thread_ts"].as_str().unwrap(),
                s["channel"].as_str().unwrap(),
            );
            store.finish(
                s["thread_ts"].as_str().unwrap(),
                s["session_id"].as_str(),
                "done",
            );
        }
    }

    let transcript = dir.join("t.jsonl");
    let mut body = String::new();
    for e in sc["transcript"].as_array().unwrap() {
        body.push_str(&serde_json::to_string(&transcript_line(e)).unwrap());
        body.push('\n');
    }
    std::fs::write(&transcript, body).unwrap();

    let payload = json!({
        "session_id": sc["hook"]["session_id"],
        "transcript_path": transcript.to_str().unwrap(),
    })
    .to_string();

    let flag = sc["hook"]["notify_flag"].as_str().unwrap().to_lowercase();
    let enabled = ["1", "true", "yes", "on"].contains(&flag.as_str());
    let channel = sc["notify_channel"].as_str();

    let client = FakeSlack::default();
    stop_hook::run(&client, &store, enabled, channel, &payload);

    let expect = &sc["expect"];
    assert_posts(&client.posts.borrow(), expect);
    assert_uploads(&client.uploads.borrow(), expect);
}

// --- drivers ---------------------------------------------------------------

fn drive(kind: &str, run: impl Fn(&Value)) {
    let spec = spec();
    let mut failures = vec![];
    for sc in spec[kind].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap().to_string();
        match catch_unwind(AssertUnwindSafe(|| run(sc))) {
            Ok(()) => println!("PASS {kind} {name}"),
            Err(e) => {
                let msg = panic_msg(e);
                println!("FAIL {kind} {name}: {msg}");
                failures.push(format!("{name}: {msg}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} {kind} scenario(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn inbound_scenarios() {
    drive("inbound", run_inbound);
}

#[test]
fn stop_hook_scenarios() {
    drive("stop_hook", run_stop_hook);
}
