---
title: Privacy
description: What kikimimi collects, what it never does, and the controls — local-only mode, full export, a repo allowlist — that keep your data yours.
---

## Principles

1. **Agent-native sources only.** kikimimi reads hooks, OpenTelemetry export, and session logs — the mechanisms each agent already exposes. No proxy, no TLS interception.
2. **Fail-open, never slow the agent.** Collection failing — the daemon down, a socket timeout, a malformed payload — never blocks or breaks the agent it's watching.
3. **Metadata by default; content is opt-in, and off.** Prompts, tool arguments, file contents, and command lines are not part of what gets collected unless you turn that on yourself.
4. **Honest numbers.** A value kikimimi can't measure is reported as `unknown`, never filled in with a guess or an estimate.
5. **Your data is yours.** Local-only mode, a full export at any time, and BYO S3 all exist so nothing about using kikimimi requires trusting kikimimi cloud with your only copy.

## What's collected

Metadata only, by schema: tool name, MCP server/tool, skill name, duration, success/failure, permission decisions, tokens, cost, model, session/turn/repo identifiers, a hashed working directory. See [How it works](/kikimimi/how-it-works/) for exactly where each of those comes from per agent.

## What's never collected

Prompt text, tool arguments (a `Bash` command, an MCP call's parameters), file contents, and tool output are not read into an event at all — with one narrow, deliberate exception: a **skill's name** (not its arguments, not its instructions) is pulled out as metadata, the same way a tool's name is, because "which skill ran" is itself a metadata question kikimimi's queries depend on ([`skills`](/kikimimi/queries/#skills), [`schema-tax`](/kikimimi/queries/#schema-tax)).

Concretely, the schema has four body columns — `tool_input_json`, `tool_output_excerpt`, `prompt_text`, `redaction_applied` — and every one of them is written as `NULL` by every collector, on every event, always. They're reserved in the schema for a future opt-in content-capture path; there's no switch to turn that on yet, so today they're unconditionally empty.

## Defense in depth for the cloud sink

If you do run `kikimimi login`, the same four body columns get nulled twice on the way to kikimimi cloud, by two independent pieces of code:

- **Client-side**: the sink that buffers events for upload masks `tool_input_json`, `tool_output_excerpt`, `prompt_text`, and `redaction_applied` to `None` before they ever leave the buffer.
- **Server-side**: the ingest endpoint binds those same four columns to `NULL` in the `INSERT` itself, regardless of what's in the JSON body of the request. Even a compromised or misbehaving client can't get body text into kikimimi cloud's database through this path — the server doesn't trust the client's own nulling and doesn't read those fields off the wire for storage at all.

## The hook shim's exit-0 guarantee

`kikimimi hook <event>` — the process Claude Code's `settings.json` actually invokes — is built so that kikimimi failing can never mean your agent fails. It reads stdin, hands the payload to a local queue, and exits 0, always: the whole call runs inside a panic guard, and even a panic gets caught, logged locally, and swallowed rather than propagated as a nonzero exit or an error Claude Code has to react to. See [How it works](/kikimimi/how-it-works/#fail-open-spool-design) for the mechanics.

## Local-only mode

The local Parquet (`file`) sink is always on — `~/.kikimimi/data/events/dt=YYYY-MM-DD/*.parquet` — independent of whether you've ever run `kikimimi login`. Skip `login` entirely and nothing leaves the machine: `kikimimi query` and `kikimimi web` both read that local Parquet directly, and there's no cloud sink to push to. `kikimimi status` shows exactly which sinks are active on a given machine.

That local-only stance depends on the data actually being yours: `127.0.0.1:4318` is reachable by any process on the machine, not just Claude Code, so the OTLP receiver requires the per-install bearer token `kikimimi init` writes for it — without one, anything else running locally could POST fabricated OTel data and have it recorded as a real session.

## Full export

`kikimimi export` downloads the complete `kikimimi.v1` Parquet for your account from kikimimi cloud (`GET /v1/export`), scoped optionally by `--from`/`--to`. It exists specifically so using the hosted cloud is never a one-way door — everything you sent up, you can always pull back down in the same schema, whether that's for backup, migration, or just closing your account. This is on top of the fact that the local Parquet sink never strips anything the way the cloud sink does — it writes whatever the event actually contains, on every machine, from day one — and that BYO S3 (`kikimimi sink add s3 ...`) writes that same unfiltered Parquet to storage you control, with kikimimi never holding your S3 credentials — uploads shell out to your own `aws` CLI.

## The repo allowlist as a privacy control

On a machine bound to a team org (`kikimimi login --org <slug>`), `kikimimi repos allow <glob>` sets a local allowlist of repo patterns. Only events whose resolved `repo` matches one of those globs are pushed to the *team* cloud sink — everything else from that machine either goes nowhere (no matching org) or stays in the personal org, never the company's. This matters concretely on a machine you use for both work and personal projects: without an allowlist configured, every event goes to the active org unfiltered, so setting one is how you keep a personal repo's activity out of your employer's dashboard. Personal orgs are never filtered this way, and neither the local file sink nor a BYO S3 sink is ever filtered — the allowlist only gates what reaches the *team* cloud sink specifically.
