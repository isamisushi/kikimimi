---
title: Queries
description: Reference for every named query kikimimi ships, what each one measures, and how to run it locally or against kikimimi cloud.
---

```
kikimimi query <NAME> [--show-sql]
kikimimi query --sql "<SQL>"
kikimimi query <NAME> --cloud [--from YYYY-MM-DD] [--to YYYY-MM-DD]
```

By default, `kikimimi query <name>` runs a fixed SQL query against your local Parquet (`~/.kikimimi/data/events/dt=*/*.parquet`) through the `duckdb` CLI, which needs to be on `PATH`. `--show-sql` prints that SQL before running it. `--sql "<SQL>"` runs arbitrary DuckDB SQL instead of a named query — useful for one-off exploration — and is mutually exclusive with a query name.

`--cloud` runs the same named query against kikimimi cloud's `GET /v1/query/<name>` instead of touching local Parquet (requires `kikimimi login`). It only accepts named queries — `--sql` is local-only and is rejected with `--cloud only runs named queries; --sql is a local (DuckDB) option`. `--from`/`--to` (`YYYY-MM-DD`, inclusive) scope the cloud query to a date range and are ignored locally. Left unset, every named query is unbounded on both paths *except* `today`, which still defaults to today's date on `--cloud` too, matching what the local query hardcodes.

**Honesty note — `tool.result` is deduplicated, not double-counted:** `kikimimi init` enables both the Claude Code hook (`PostToolUse`/`PostToolUseFailure`) and the OTel exporter for the same install, so a single tool call can legitimately produce *two* `tool.result` rows for the same `tool_use_id` — one `source='hook'`, one `source='otel'` — kept as separate rows on purpose (they carry different fields, and losing either makes gaps and hook/OTel correlation harder to see later). Every query below that counts or measures `tool.result` (`today`'s `failures`, `tools`, `mcp`, `skills`, `bypass`'s `mcp_fail`, `thrash`'s `repeat_failure`/`success_pairs`) collapses that pair back to one logical result before counting: it keeps the OTel row when one exists for a given `(session_id, correlation_key)` — OTel reliably carries `success` and `duration_ms`, the hook row often doesn't — and falls back to the hook row otherwise (Claude Code OTel export can be silently absent, e.g. on Windows). A `tool.result` with no `correlation_key` (nothing to match it against) is never merged into another row. `tool.call` never duplicates this way (hook-only), so call counts are unaffected. `events`/`tool_calls` totals in `today` stay raw ingested counts on purpose — only the columns that would otherwise silently double an outcome are deduplicated.

## today

Today's activity: total events, tool calls, and failures for the day, broken out by model. The `events`/`tool_calls`/`failures` totals repeat on every row — they're whole-day scalars, not aggregated per model — only the token/cost columns actually vary row to row.

```
$ kikimimi query today
```

| events | tool_calls | failures | model | input_tokens | output_tokens | cost_usd |
|---|---|---|---|---|---|---|
| 412 | 168 | 9 | claude-opus-4-5-20260514 | 1284302 | 48211 | 9.42118841 |
| 412 | 168 | 9 | claude-sonnet-4-5-20260514 | 512004 | 21030 | 1.18004402 |

## tools

Per-tool call volume, failure count, and p50/p95 duration — one row per `tool_name`, sorted by call count. Covers everything, not just MCP: builtins like `Bash` and `Read` show up next to `mcp__<server>__<tool>` entries.

```
$ kikimimi query tools
```

| tool_name | calls | failures | p50_duration_ms | p95_duration_ms |
|---|---|---|---|---|
| Bash | 241 | 6 | 340 | 2210 |
| Read | 188 | 0 | 41 | 95 |
| mcp__github__get_issue | 52 | 1 | 310 | 880 |
| mcp__playwright__navigate | 37 | 4 | 610 | 3400 |

## mcp

Per-MCP-server call volume, failures, and distinct sessions that touched it — the same shape as `tools` but rolled up to the server, not the individual tool.

```
$ kikimimi query mcp
```

| mcp_server | calls | failures | distinct_sessions |
|---|---|---|---|
| github | 96 | 3 | 11 |
| playwright | 61 | 7 | 8 |
| linear | 14 | 0 | 4 |

## skills

Per-skill invocation count, failures, distinct sessions, and the most recent date it was used — Claude Code and Codex skills together (see [How it works](/kikimimi/how-it-works/) for how each is detected).

```
$ kikimimi query skills
```

| skill_name | calls | failures | distinct_sessions | last_used_dt |
|---|---|---|---|---|
| code-review | 22 | 1 | 9 | 2026-08-30 |
| katamari-review | 14 | 0 | 6 | 2026-08-29 |
| simplify | 6 | 0 | 3 | 2026-08-27 |

## thrash

v0 stuck-agent signals. Two independent patterns, unioned into one table (`kind` tells them apart):

- **`repeat_failure`** — same session, same `tool_name`, at least 3 `tool.result` failures, and *not one success* for that (session, tool) pair anywhere in the session. `incidents` is the failure count, `first_ts`/`last_ts` bound the run.
- **`deny_detour`** — a `tool.denied` event followed, within 5 events (by row order, not wall-clock) in the same session, by a `bash` or `browser` tool call. `tool_name` is the *denied* tool, not the one the agent switched to. `incidents` is always 1 per detour found.

```
$ kikimimi query thrash
```

| session_id | kind | tool_name | incidents | first_ts | last_ts |
|---|---|---|---|---|---|
| sess_a1b2c3 | repeat_failure | mcp__jira__create_issue | 4 | 1788098531000 | 1788098612000 |
| sess_a1b2c3 | deny_detour | WebFetch | 1 | 1788098650000 | 1788098654000 |

`first_ts`/`last_ts` are raw `ts` values — epoch milliseconds, UTC — the same as the schema column, not a formatted date.

**Honesty note (v0 proxy):** `repeat_failure` is not gaps-and-islands detection; it doesn't require the failures to be *consecutive*, only that the pair never once succeeded. A session that fails a tool 3 times, succeeds once, then fails 3 more times is excluded entirely — one success anywhere clears the whole pair. A stricter, consecutive-run version needs a different query.

## bypass

An MCP tool failure followed, within 5 events in the same session, by a `bash` or `browser` tool call — the literal "gave up on MCP, went around it" pattern.

```
$ kikimimi query bypass
```

| session_id | mcp_server | following_tool_name | fail_ts | bypass_ts |
|---|---|---|---|---|
| sess_a1b2c3 | github | Bash | 1788098531000 | 1788098567000 |
| sess_d4e5f6 | linear | mcp__playwright__navigate | 1788081300000 | 1788081322000 |

**Honesty note:** this is a measurement, not an alarm. CLI-over-MCP is often a deliberate, reasonable optimization — an agent hitting `gh` directly instead of a flaky MCP wrapper isn't necessarily "stuck." Treat a high `bypass` rate on one server as a prompt to look closer, not as a verdict. `thrash`'s `deny_detour` signal (above) captures the narrower case where the MCP side was an outright permission denial rather than any kind of failure. Browser-automation MCP servers (Playwright, claude-in-chrome, ...) classify as `tool_kind='browser'`, not `'mcp'` (see [How it works](/kikimimi/how-it-works/)), so a failure from one of *those* servers never appears as the `mcp_server` origin here — only as the `following_tool_name` an agent switched to, as in the `linear` → `mcp__playwright__navigate` row above.

## reach

How agents actually reach resources, broken down by day, session, and `tool_kind` (`mcp` / `bash` / `browser`) — the raw material for "what fraction of calls go through MCP vs. shelling out" over time.

```
$ kikimimi query reach
```

| dt | session_id | tool_kind | calls |
|---|---|---|---|
| 2026-08-30 | sess_a1b2c3 | bash | 41 |
| 2026-08-30 | sess_a1b2c3 | mcp | 12 |
| 2026-08-30 | sess_a1b2c3 | browser | 3 |
| 2026-08-30 | sess_d4e5f6 | mcp | 27 |

## unused-mcp

MCP servers configured in `~/.claude/settings.json` / `~/.claude.json` (top-level `mcpServers`, and the per-project `projects.<path>.mcpServers` entries `~/.claude.json` actually uses) but never called — pure context tax, since a configured server's tool schemas ship with every request whether or not the agent ever calls it. Rows where a server is configured *and* has zero calls sort first, on purpose; that's the whole point of the query.

```
$ kikimimi query unused-mcp
```

| mcp_server | configured | calls_in_range | last_called_dt |
|---|---|---|---|
| figma | true | 0 | NULL |
| notion | true | 0 | NULL |
| github | true | 128 | 2026-08-30 |
| jira | false | 6 | 2026-08-12 |

`jira` here means a server that got called historically but isn't in the current config — removed, renamed, or configured only in a project you're not in right now. `calls_in_range` is unbounded history locally (there's no local date filter on this query); `--cloud --from/--to` can scope it.

## schema-tax

v0 fixed-context proxy, per session, from OTel `api.request` rows: `first_input_tokens` is `input_tokens + cache_read_tokens` on the session's *earliest* request — turn 1 has nothing cached yet, so (almost) everything read there is fixed overhead (tool schemas, `CLAUDE.md`, system prompt) rather than conversation history. `fixed_share_pct` divides that by the session's total `input_tokens + cache_read_tokens` across every request. A `TOTAL` row rolls the whole range up.

```
$ kikimimi query schema-tax
```

| session_id | api_requests | input_tokens | cache_read_tokens | cache_write_tokens | output_tokens | first_input_tokens | fixed_share_pct |
|---|---|---|---|---|---|---|---|
| sess_a1b2c3 | 2 | 410 | 38598 | 6772 | 464 | 16017 | 41.060808039377 |
| sess_d4e5f6 | 5 | 604 | 317752 | 9720 | 1121 | 58590 | 18.403925165538 |
| TOTAL | 7 | 1014 | 356350 | 16492 | 1585 | 74607 | 20.877032941203 |

**Honesty note (v0 limitation):** this is a coarse proxy, not a true schema-vs-`CLAUDE.md`-vs-prompt breakdown. OTel gives token *counts* per request, not what's inside them — telling an MCP tool schema apart from `CLAUDE.md` apart from the actual first user prompt needs transcript-level data, which kikimimi doesn't collect — the `prompt_text` body column that would carry it stays unpopulated (see [Privacy](/kikimimi/privacy/)). Treat `fixed_share_pct` as a same-session-turn-1-vs-rest signal, not an exact accounting.
