# Security Policy

> Please also read the **USE AT YOUR OWN RISK** disclaimer in the
> [README](README.md). This software is provided "AS IS" with no warranty and
> no liability. Securing your own deployment (Slack tokens, `ALLOWED_USERS`,
> permission mode, network exposure) is your responsibility.

## Supported versions

This is a single-developer project without formal releases. Only the latest
commit on the `main` branch is supported. Fixes land on `main`; there are no
backports.

## Reporting a vulnerability

**Do not open a public issue for security problems.**

Please report privately via GitHub's private vulnerability reporting:

- Go to the [**Security** tab → **Report a vulnerability**](https://github.com/pomcho555/claude-slack-bridge/security/advisories/new).

Include, where possible:

- a description of the issue and its impact,
- steps to reproduce or a proof of concept,
- affected component (e.g. `claude_runner.rs`, the Stop hook, a binary),
- any suggested remediation.

You will get an acknowledgement, and the report will be handled privately until
a fix is available, after which a GitHub Security Advisory may be published.

## Scope notes

This tool deliberately executes Claude Code with tool access on the host, driven
by Slack messages. Behaviour that is **by design and documented** is out of
scope, in particular:

- anyone listed in `ALLOWED_USERS` being able to run code on the host, and
- running with `CLAUDE_PERMISSION_MODE=bypassPermissions`.

In scope are issues that let an **unauthorized** actor trigger jobs, read or
exfiltrate tokens/secrets, bypass `ALLOWED_USERS`, or otherwise escalate beyond
the documented trust model.
