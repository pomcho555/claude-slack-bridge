#!/usr/bin/env python3
"""Stand-in for the real ``claude`` CLI, used by the E2E tests.

It mimics ``claude --print --output-format json`` closely enough to exercise
the whole bridge without a real model: it emits the JSON envelope the bridge
parses, and records how it was invoked (so tests can assert e.g. that a reply
passed ``--resume <session>``).

Behaviour is driven entirely by environment variables so individual tests can
shape one invocation:

  FAKE_CLAUDE_LOG      append-only JSONL of each invocation (resume/prompt/argv)
  FAKE_CLAUDE_SESSION  force the returned session_id
  FAKE_CLAUDE_RESULT   force the returned result text (default: "echo: <prompt>")
  FAKE_CLAUDE_ERROR    "1" -> is_error: true
  FAKE_CLAUDE_SLEEP    seconds to sleep before answering (to trigger timeouts)
  FAKE_CLAUDE_RAW      emit this raw string verbatim instead of JSON
"""
from __future__ import annotations

import json
import os
import sys
import time


def main() -> None:
    args = sys.argv[1:]

    resume = None
    for i, a in enumerate(args):
        if a == "--resume" and i + 1 < len(args):
            resume = args[i + 1]
    prompt = args[-1] if args else ""

    log = os.environ.get("FAKE_CLAUDE_LOG")
    if log:
        with open(log, "a", encoding="utf-8") as fh:
            fh.write(json.dumps({"resume": resume, "prompt": prompt, "argv": args}) + "\n")

    sleep = float(os.environ.get("FAKE_CLAUDE_SLEEP", "0") or "0")
    if sleep:
        time.sleep(sleep)

    raw = os.environ.get("FAKE_CLAUDE_RAW")
    if raw is not None:
        sys.stdout.write(raw)
        return

    if "FAKE_CLAUDE_SESSION" in os.environ:
        session = os.environ["FAKE_CLAUDE_SESSION"]
    elif resume:
        session = resume
    else:
        session = "sess-new"

    result = os.environ.get("FAKE_CLAUDE_RESULT", f"echo: {prompt}")
    is_error = os.environ.get("FAKE_CLAUDE_ERROR") == "1"
    print(json.dumps({"session_id": session, "result": result, "is_error": is_error}))


if __name__ == "__main__":
    main()
