//! Language-specific runner behavior NOT covered by the shared spec (see
//! spec/README.md "Out of scope"): missing binary, non-JSON output, `--resume`
//! threading, and the subprocess timeout.
//!
//! All sub-cases live in ONE `#[test]` so they run sequentially — they share
//! process-global env vars (`FAKE_CLAUDE_*`) that would race under cargo's
//! default per-test parallelism.

use std::path::PathBuf;

use slack_claude_bridge::claude_runner::ClaudeRunner;

fn fake_claude() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests")
        .join("fake_claude.py")
        .to_str()
        .unwrap()
        .to_string()
}

fn runner(binary: &str, timeout: u64) -> ClaudeRunner {
    ClaudeRunner {
        binary: binary.to_string(),
        workdir: std::env::temp_dir().to_str().unwrap().to_string(),
        permission_mode: "acceptEdits".into(),
        model: None,
        extra_args: vec![],
        timeout,
    }
}

#[test]
fn runner_behaviors() {
    // Clean slate for env-driven fake behavior.
    for k in ["FAKE_CLAUDE_SESSION", "FAKE_CLAUDE_RESULT", "FAKE_CLAUDE_ERROR", "FAKE_CLAUDE_RAW", "FAKE_CLAUDE_SLEEP", "FAKE_CLAUDE_LOG"] {
        std::env::remove_var(k);
    }

    // 1. Missing binary -> reported as an error result, not a panic.
    {
        let r = runner("definitely-not-a-real-binary-xyzzy", 10);
        let res = r.run_new("hi");
        assert!(res.is_error, "missing binary should be an error");
        assert!(res.text.contains("not found"), "got: {:?}", res.text);
    }

    // 2. Non-JSON output -> surfaced verbatim, not hidden.
    {
        std::env::set_var("FAKE_CLAUDE_RAW", "totally not json");
        let r = runner(&fake_claude(), 10);
        let res = r.run_new("hi");
        assert_eq!(res.text, "totally not json");
        assert!(!res.is_error, "exit 0 + non-JSON is not an error");
        std::env::remove_var("FAKE_CLAUDE_RAW");
    }

    // 3. run_resume passes --resume and parses session/result.
    {
        let log = std::env::temp_dir().join("rust-runner-resume.jsonl");
        let _ = std::fs::remove_file(&log);
        std::env::set_var("FAKE_CLAUDE_LOG", &log);
        std::env::set_var("FAKE_CLAUDE_SESSION", "sess-xyz");
        std::env::set_var("FAKE_CLAUDE_RESULT", "resumed ok");
        let r = runner(&fake_claude(), 10);
        let res = r.run_resume("sess-xyz", "continue please");
        assert_eq!(res.session_id.as_deref(), Some("sess-xyz"));
        assert_eq!(res.text, "resumed ok");
        assert!(!res.is_error);

        let logged = std::fs::read_to_string(&log).unwrap();
        let entry: serde_json::Value = serde_json::from_str(logged.lines().next().unwrap()).unwrap();
        assert_eq!(entry["resume"].as_str(), Some("sess-xyz"));
        assert!(entry["prompt"].as_str().unwrap().contains("continue please"));

        std::env::remove_var("FAKE_CLAUDE_LOG");
        std::env::remove_var("FAKE_CLAUDE_SESSION");
        std::env::remove_var("FAKE_CLAUDE_RESULT");
    }

    // 4. Timeout -> reported, the child is killed, no hang.
    {
        std::env::set_var("FAKE_CLAUDE_SLEEP", "5");
        let r = runner(&fake_claude(), 1); // 1s budget vs 5s sleep
        let start = std::time::Instant::now();
        let res = r.run_new("slow");
        let elapsed = start.elapsed();
        assert!(res.is_error, "timeout should be an error");
        assert!(res.text.contains("timed out"), "got: {:?}", res.text);
        assert!(elapsed.as_secs() < 4, "should have killed near 1s, took {elapsed:?}");
        std::env::remove_var("FAKE_CLAUDE_SLEEP");
    }
}
