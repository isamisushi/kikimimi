---
title: Quickstart
description: Init, start the daemon, check status, open the local dashboard, and run your first queries.
---

Run these from anywhere — `kikimimi init` touches your global Claude Code settings, not a project directory.

## kikimimi init

```sh
kikimimi init
```

Writes into `~/.claude/settings.json` (idempotent — safe to re-run):

- Seven hook entries under `hooks.*`: `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionDenied`, `SubagentStop`, `SessionStart` (5s timeout each), and `SessionEnd` (1s timeout). Each runs `kikimimi hook <EVENT>`, which always exits `0` and just spools the event to disk — the daemon drains the spool separately, so a slow or crashed daemon can never block a hook.
- Five env vars that turn on Claude Code's own OpenTelemetry export and point it at kikimimi's local OTLP receiver: `CLAUDE_CODE_ENABLE_TELEMETRY=1`, `OTEL_METRICS_EXPORTER=otlp`, `OTEL_LOGS_EXPORTER=otlp`, `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`, `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318`. If port 4318 is already taken, `init` picks a free one instead and writes that port everywhere it's needed.

If `~/.claude/settings.json` already exists, the first change backs it up once, alongside the original, as `settings.json.kikimimi-backup`. A hook or env value you've edited yourself since `init` last ran is never overwritten — `init` warns and leaves it alone instead.

If `~/.codex` exists, `init` also reports detecting it, but writes nothing there — Codex is covered by `kikimimi agent`'s own rollout-log tailer (`~/.codex/sessions`), which needs no config change to work.

## kikimimi agent

```sh
kikimimi agent &
```

Starts the resident daemon. One process runs four jobs at once: drains the spool `kikimimi hook` writes to, runs the local OTLP receiver Claude Code's telemetry lands on, tails Codex's rollout session logs, and pushes to whatever sinks are configured — local Parquet always, cloud and BYO S3 only if you've set them up. It daemonizes itself by default (detaches from your terminal, so the trailing `&` isn't strictly required); pass `--foreground` to keep it attached, e.g. while debugging.

## kikimimi status

```sh
kikimimi status
```

Reports on everything `init` and `agent` set up — settings.json hook/env state, whether the daemon is running and its pid, per-source event counts and anything skipped, the Codex tailer's file/line counts, cloud and S3 sink state, spool backlog, and local data dir size:

```
daemon: running
state.json:
  pid: 84213
  events: hook=142 otel=38 log=0
  otlp_port: 4318
  web_port: 4319
  cloud:
    endpoint: https://kikimimi.dev
    pending: 0

web UI: http://127.0.0.1:4319/?t=<32-hex token>

spool backlog: 0 file(s)
data dir: /home/you/.kikimimi/data/events (212 file(s), 4.1 MB)
```

## kikimimi web

```sh
kikimimi web
```

Prints the local dashboard's URL and makes a best-effort attempt to open it in a browser:

```
http://127.0.0.1:4319/?t=<32-hex token>
```

The token is regenerated every time the daemon (re)starts, lives only in `state.json`, and is never persisted anywhere else. Visiting the tokened URL once sets a 30-day cookie in that browser, so you don't need to re-paste the token on every reload — until the daemon restarts and rotates it. The dashboard itself (Overview, Tools, MCP, Skills, Sessions) reads local Parquet only; nothing leaves the machine, whether or not you've ever run `kikimimi login`.

## First queries

```sh
kikimimi query thrash
kikimimi query unused-mcp
kikimimi query skills
```

Named queries run against local Parquet through the `duckdb` CLI — it needs to be on `PATH` (`brew install duckdb`, or a release binary from [duckdb.org](https://duckdb.org)). kikimimi shells out to it (`duckdb -c "<sql>"`) rather than embedding its own query engine, and says so plainly if it's missing.

- **`thrash`** — v0 stuck-agent signals, one row per incident. `repeat_failure`: the same tool failed 3+ times in a session with no success for that tool at all. `deny_detour`: a permission denial followed, within 5 events, by a bash or browser call. Both are proxies, not certainty — a session that fails 3 times, recovers, then fails 3 more times elsewhere won't show as `repeat_failure` under this v0 definition, since any success for that (session, tool) pair excludes it entirely.
- **`unused-mcp`** — MCP servers configured in `~/.claude/settings.json` / `~/.claude.json` that were never actually called. Pure context tax: their schemas ship on every request whether the agent ever uses them or not.
- **`skills`** — per-skill invocation/failure/session counts and last-used date, across both Claude Code and Codex.

`kikimimi query <name> --show-sql` prints the SQL before running it. `kikimimi query --sql "<raw sql>"` runs anything else against the same Parquet files. Other named queries: `today`, `tools`, `mcp`, `reach`, `bypass`, `schema-tax` — the same `--show-sql` flag works on any of them. Add `--cloud` to run a named query against kikimimi cloud instead of local Parquet (requires `kikimimi login` first).
