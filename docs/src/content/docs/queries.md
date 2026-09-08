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

**Where `configured` comes from.** Claude Code's `session.start` hook snapshots the names of every currently-configured MCP server (`~/.claude/settings.json`'s and `~/.claude.json`'s `mcpServers`, plus the project-scoped `.mcp.json` in the session's working directory) onto the event as `configured_mcp_servers` — so "configured" is normally a real fact read straight off a recent session, not a guess. Sessions that came in through the transcript backfill get the same snapshot on their `session.end` instead, from the MCP tool names Claude Code listed in the transcript — which also covers claude.ai connectors that are not in any settings file; where a session has both, the transcript's wins. If no `session.start`/`session.end` row in the query's date range carries a snapshot (an install that predates it, or simply no session yet in range), `unused-mcp` quietly falls back to its old proxy instead: any server observed in the *trailing 30 days*, independent of the query's own range, counts as "configured". That fallback can't ever show a server configured-but-truly-never-called (it only "knows" a server exists once something has called it), so on kikimimi cloud the web MCP page's richer `/web/q/unused-mcp` also returns a `configured_from_snapshot` flag and the page shows a note when it's `false`, telling you to upgrade and start a new session rather than silently trusting a weaker signal. The local daemon has no such gap — it reads the live config files directly, so `configured_from_snapshot` is always `true` there.

## unused-skills

The `unused-mcp` idea for skills: which skills Claude Code had loaded in a session versus which were actually invoked.

`configured` comes from Claude Code's own skill listing, which it writes into the session transcript (bundled skills, `~/.claude/skills`, plugin skills — whatever it loaded), captured by the transcript backfill as `configured_skills` on that session's `session.end`. Hooks carry no equivalent, so a hooks-only session says nothing about what was configured; kikimimi does not guess from directories. `calls` are `tool.call` rows with a `skill_name`. Configured-but-never-invoked rows sort first.

```
$ kikimimi query unused-skills
```

| skill_name | configured | sessions_configured | calls | distinct_sessions | last_used_dt |
|---|---|---|---|---|---|
| dataviz | true | 41 | 0 | 0 |  |
| design | true | 41 | 12 | 9 | 2026-09-05 |
| katamari-review | false | 0 | 3 | 2 | 2026-09-02 |

Also on the web **Skills** page (`/web/q/unused-skills`). A skill that is `configured = false` was invoked but never listed — a session without a transcript backfill, or a skill invoked by name that Claude Code does not list.

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

## mcp-tax

`schema-tax` per MCP server: how much of each session's fixed context each configured server is carrying, and how much of that was carried for nothing.

- A session's fixed context is `first_input_tokens` (the same proxy as `schema-tax`), and every one of its API requests re-reads it.
- `session.start`'s `configured_mcp_servers` snapshot says which servers were loaded; `tool.call` rows say which were used.
- **`fixed_tokens_est`** splits that fixed context *equally* across the configured servers and multiplies by the session's request count, summed over every session where the server was configured.
- **`unused_tokens_est`** is the same sum over sessions where the server was configured but never called.
- **`marginal_first_tokens_est`** is the median `first_input_tokens` of sessions with the server minus the median without it — a measured per-server size when your configs vary enough to give a contrast, `NULL` otherwise.

```
$ kikimimi query mcp-tax
```

| mcp_server | sessions_configured | sessions_used | sessions_unused | sessions_with_usage | fixed_tokens_est | unused_tokens_est | marginal_first_tokens_est |
|---|---|---|---|---|---|---|---|
| jira | 2 | 1 | 1 | 2 | 3700 | 1000 | 550 |
| gh | 3 | 1 | 2 | 2 | 1400 | 400 | -200 |

**Honesty note:** the equal split is an allocation rule, not a measurement — a server with one tiny tool and one with forty large ones get the same share. OTel reports token counts, not what is inside them, so this is the best available without per-server schema sizes; `marginal_first_tokens_est` is the measurement-shaped number when the data supports it. Sessions without a snapshot (older clients) or without OTel usage contribute nothing rather than 0; `sessions_with_usage` says how many were priceable.

## patterns

The persisted detections behind the ranking and before/after views. One row per incident, each attributed to a **subject** — the MCP server or tool the incident points at — and priced:

- **`mcp_bypass`** — a failed MCP `tool.result` followed within 5 events by a `bash`/`browser` call (same windowing as `bypass`). `subject` = the MCP server.
- **`deny_detour`** — a `tool.denied` followed within 5 events by a `bash`/`browser` call (same as `thrash`). `subject` = the denied tool.
- **`retry_spiral`** — at least 3 *consecutive* failures of one tool in one session (a success ends the run). This is the gaps-and-islands version of `thrash`'s `repeat_failure` proxy. `subject` = the tool.
- **`permission_denied_loop`** — at least 2 consecutive `tool.denied` for the same tool. `subject` = the tool.
- **`context_bloat`** — an `api.request` whose context (`input + cache_read + cache_write`) is at least 1.5× and 20k tokens above the previous request *of the same model in the same conversation stream* — the main conversation or one subagent (see [subagents](#subagents): transcript rows carry `agent_id`, so a session with transcript data is split per agent; a session with only OTel rows keeps the ones whose `query_source` is `main` or unknown) — ignoring a previous request under 5k tokens, and only when the next request of that stream stays at 80% or more of the new size. Claude Code interleaves small Haiku helper calls with the main model, and parallel subagents share the parent's session id and alternate between two context sizes; on one machine's real data the naive rule flagged 35 jumps in one session and the same-model rule 30 (both over OTel rows, which cannot separate subagents); with the per-stream rule over a full transcript backfill of the same machine the busiest session has 16 and 60 sessions together 27 (2026-09-07). `subject` = the last `tool.result` in that stream before the jump (the usual culprit is an oversized tool output), `wasted_tokens_est` = the jump. Separately, a session with 2 or more `compaction` events gets one hit with `subject` = `compaction`.
- **`long_tool_tail`** — an MCP `tool.result` taking at least 10 s and at least 3× that tool's median for the day. `subject` = the MCP server. (§7.2 says p95; with a day's samples the outlier is its own p95, so the median rule is the honest small-sample version.)
- **`unused_mcp_server`** — a server in the session's `configured_mcp_servers` snapshot that the session never called. `subject` = the server. Priced with the `mcp-tax` equal-split allocation for that session (`detail` carries `n_configured`, `api_requests`, `allocation`).

`wasted_tokens_est` is `input_tokens + output_tokens` of the session's OTel `api.request` rows inside `[first_ts, last_ts]` — what the model burned while stuck. Cache reads are excluded. It is `NULL` (unknown), never 0, when no OTel usage fell in the window. `detail` is a small JSON object (the detour tool, the failed tool, the MCP server).

```
$ kikimimi query patterns
```

| session_id | pattern_id | subject | first_ts | last_ts | incidents | wasted_tokens_est | detail |
|---|---|---|---|---|---|---|---|
| sess_a1b2c3 | mcp_bypass | gh | 1788602401000 | 1788602404000 | 1 | 1600 | {"failed_tool":"mcp__gh__search","detour_tool":"Bash"} |
| sess_a1b2c3 | retry_spiral | mcp__jira__create | 1788602460000 | 1788602462000 | 3 |  | {"mcp_server":"jira"} |

Locally this recomputes over your Parquet on every call. On the cloud a background scanner writes the same rows into a `pattern_hits` table per `(org, dt)` and `kikimimi query patterns --cloud` reads that table (it also carries `dt` and `first_detected_at`). A day is rescanned while events keep arriving for it, until 72 hours after the day ends; after that its detections are frozen and later arrivals are only counted, so the numbers you looked at yesterday don't move today. Everything is org-scoped by row-level security, like `events`.

**Honesty note:** windows are evaluated per day, so an incident that straddles midnight UTC is priced within the scanned day only. `mcp_bypass` (full version with a resource map) is Stage 2; the subagent side of §7.2 is the [subagents](#subagents) query below.

## subagents

How much of each session ran inside Agent-tool subagents (architecture.md §7.2 `subagent_fanout`). Claude Code reports a subagent's hooks, transcript lines and API requests under the **parent's** `session_id`; kikimimi keeps it that way (every per-session view already includes the subagents) and adds three `kikimimi.v1` columns to tell the streams apart: `agent_id` (the hook's `agent_id` / the transcript's `agentId`), `agent_type` (`Explore`, `general-purpose`, …; from hooks, or OTel's `agent.name`) and `query_source` (`main` / `subagent` / `auxiliary`; OTel's own field, derived from `agent_id` for hooks and transcripts, NULL for rows written before this column existed — unknown, not main). Transcripts of subagents live in `<session>/subagents/` (Workflow-launched ones a level deeper) and are backfilled like the parent's.

One row per session that had at least one subagent, plus a `TOTAL` row:

| column | meaning |
|---|---|
| `subagents` | distinct `agent_id`s in the session |
| `agent_types` | the types seen, comma-joined (NULL when only the transcript saw them — it has no type) |
| `subagent_tool_calls` / `tool_calls` | tool calls inside subagents / in the whole session |
| `subagent_duration_ms` / `session_duration_ms` / `duration_share` | Σ per agent of its `subagent.stop` duration (else last − first event), the session's first-to-last span, and the ratio. Parallel subagents can push the share above 1 |
| `subagent_api_requests` | `api.request` rows that carry an `agent_id` (transcript rows; OTel has none) |
| `subagent_tokens_est` / `session_tokens_est` / `token_share` | `input + output` of those rows (or the `SubagentStop` hook's usage block when it had one), the session's OTel total (transcript total when there is no OTel), and the ratio. NULL when nothing carried usage — never 0 |
| `subagents_with_usage` | how many of the `subagents` were priceable. Claude Code still drops subagent usage in several paths ([#83430](https://github.com/anthropics/claude-code/issues/83430), [#88107](https://github.com/anthropics/claude-code/issues/88107)); this is the `usage_source = unknown` rate, per session |

```
$ kikimimi query subagents
```

| session_id | started_at | subagents | agent_types | subagent_tool_calls | tool_calls | subagent_duration_ms | session_duration_ms | duration_share | subagent_api_requests | subagent_tokens_est | session_tokens_est | token_share | subagents_with_usage |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| a09c9517-… | 2026-08-28T13:51:32Z | 5 |  | 292 | 822 | 2005457 | 386026617 | 0.005 | 258 | 2581 | 793826 | 0.003 | 5 |
| TOTAL |  | 5 |  | 292 | 822 | 2005457 | 386026617 | 0.005 | 258 | 2581 | 793826 | 0.003 | 5 |

The web **Subagents** page (`/web/q/subagents`) is the same view without the `TOTAL` row, scoped like Sessions (a team member sees their own; an admin/owner request writes a `sessions_drilldown` audit row).

**Honesty note:** the exact "which Agent call spawned this subagent" link exists only in the parent transcript (the Agent tool's result carries the `agentId`) and is not stored; the subagent's rows do share the parent's `turn_id`, so per-turn attribution works. Live sessions are captured by hooks, which carry `agent_id` but no usage, so on a machine where the transcript backfill has nothing to do `subagents_with_usage` stays low — that is the upstream gap, reported rather than estimated.

## session (web only)

One session, drilled down — the page you land on when you click a session id on the web **Sessions** or **Subagents** page (`/sessions/<id>`, `GET /web/q/session?session_id=<id>&events_limit=N`). There is no `kikimimi query session` yet; the same six sections come back in one response, each in the usual `{columns, rows}` shape, from the cloud (one RLS transaction) or from the local daemon (`kikimimi web`, one DuckDB call per section):

| section | what it holds |
|---|---|
| `summary` | one row: agent + version, host, repo, `started_at` / `ended_at` / `duration_ms` (`ended` says whether a `session.end` was actually seen — otherwise the end is just the last event so far), events, turns, tool calls, `failures` (the same definition as the Sessions list: every `success = false` row, hook/OTel `tool.result` pairs deduped), denied tools, API requests / errors, compactions, distinct subagents, models, sources, token sums (input / output / cache read / cache write), cost, the `configured_mcp_servers` / `configured_skills` snapshots, and `efforts` (the distinct `effort` values seen, comma-joined) |
| `tools` | per tool: calls, how many of them came from a subagent, failures, denied, p50 / p95 and **total** result duration — where the wall-clock went |
| `subagents` | per `agent_id`: type, parent `turn_id`, start, duration (the `SubagentStop` hook's, else first-to-last event), events, tool calls, failures, API requests, `tokens_est` (NULL when nothing carried usage — never 0), the distinct tools it used, and `models` / `efforts` — which model and effort the subagent actually ran on (NULL when no row of that agent carried them) |
| `models` | per (model, effort) inside this session: API requests, API errors, how many of the requests came from subagents, input / output / cache read / cache write / reasoning tokens and cost — the same per-session OTel-over-transcript rule as [models](#models-web-only), so a request seen by both sources is counted once |
| `timeline` | events / tool calls / failures / API requests / tokens / subagent events per time bucket. `bucket_ms` is picked from the session's span (≥ 1 minute, snapped to 1/2/5/10/15/30 min or 1/2/6/12/24 h, so the whole session fits in ~240 bars) and returned alongside |
| `events` | the first `events_limit` (default 500, max 2000) events chronologically, metadata columns only — type, source, tool / MCP server / skill, `agent_id` / `agent_type`, duration, success / error type / decision, model, tokens, cost, `turn_id`, `effort`. `summary.events` is the uncapped count, so the page can say "first N of M" |

Scoped like Sessions: in a team org a member gets a 404 for anyone else's session (indistinguishable from an unknown id), and an admin/owner's request writes a `session_drilldown` audit row naming the session.

## models (web only)

Which model, at which effort, burned how many tokens — the web **Models** page (`/models`, `GET /web/q/models?days=N`, default 14 days, on both kikimimi cloud and the local `kikimimi web` daemon). Org-wide aggregate, never per person, so every role can read it. Two sections in one response:

| section | what it holds |
|---|---|
| `models` | one row per (`model`, `effort`): `api_requests`, `api_errors`, `sessions`, `subagent_api_requests` / `subagent_tokens` (the requests a subagent made — an `agent_id` on transcript rows, an `agent_type` / `query_source` of `agent:…` on OTel rows, which never carry `agent_id` — and their input + output), `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `cost_usd`. Ordered by input + output, descending |
| `daily` | per (`dt`, `model`): `input_tokens`, `output_tokens`, `cost_usd` — the stacked bars on the page |

Where the columns come from, per source (see [How it works](/kikimimi/how-it-works/#collection-model)):

- `model` and `effort` are Claude Code's own fields: OTel `api_request` attributes, the transcript record's top-level `effort` + `message.model`, and every hook payload's `effort`. `effort` is NULL for Claude Code's internal helper calls (the Haiku `<synthetic>`-style requests have no effort), and `model` is `unknown` when a source had none. Codex: `model` from `turn_context`, `effort` from its `reasoning_effort` (top-level or `collaboration_mode.settings`), which is usually null in practice.
- Tokens and cost come from `api.request` rows only. A request that the daemon saw both via OTel and via the transcript backfill is counted **once**: per session, OTel rows win when the session has any, otherwise transcript rows — the same rule [subagents](#subagents) uses for `session_tokens_est`. `cost_usd` therefore exists only for OTel-sourced sessions (transcripts carry no cost), and `reasoning_tokens` only for transcript-sourced ones (OTel does not export thinking tokens) — each is NULL, never 0, when its source was absent.
- `api_errors` counts `api.error` rows for the same (model, effort) regardless of source.

**Honesty note:** the page cannot say which effort *setting* a user had chosen, only what Claude Code reported per request (`effortLevel` in `settings.json` and `modelSettings` per model both end up in this field). Sessions whose usage never arrived (`usage_source = unknown`, see [What is missing, measured](/kikimimi/how-it-works/#what-is-missing-measured)) are simply absent here.
