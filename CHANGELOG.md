# Changelog

All notable changes to kikimimi. The GitHub release for each tag reproduces the matching section below.

## 0.6.0 - 2026-09-07

### Added

- Backfilled Claude Code sessions now carry `configured_mcp_servers` (from the MCP tool names
  the transcript lists — claude.ai connectors included) and a new additive `configured_skills`
  column (Claude Code's own skill listing; migration `0013_configured_skills`) on their
  `session.end`, so `unused-mcp`, `mcp-tax` and the `unused_mcp_server` pattern no longer fall
  back to the 30-day proxy for them (KKM-18). Where a session has both the hook's `session.start`
  snapshot and the transcript's, the transcript's wins. New named query `unused-skills` (local
  and cloud) and a "Configured vs invoked" table on the Skills page (`/web/q/unused-skills`):
  skills Claude Code listed per session versus skills invoked; hooks-only sessions contribute
  nothing to "configured" rather than a directory guess.

- Codex `session.start` rows now carry `configured_mcp_servers`, read from the
  `[mcp_servers.<name>]` tables of `$CODEX_HOME/config.toml` (names only), so `unused-mcp`
  and the `unused_mcp_server` pattern cover Codex sessions (KKM-16). Codex `tool.denied` is
  still not emitted: the installed codex-cli's approval vocabulary was confirmed but no
  denial record was observed on disk, and kikimimi does not guess record shapes — see
  [How it works](https://isamisushi.github.io/kikimimi/how-it-works/#codex-cli).

- **Coverage** (KKM-17, architecture.md §7.1 "数字の信頼度を隠さない"): `/web/q/coverage`
  (local and cloud) returns the counts behind every missing-data rate — sessions without
  token usage, hook↔OTel `tool_use_id` match, what the dedup folded away, subagents without
  usage, events without an account, hosts silent for a day — and the web UI shows them as a
  badge in the top bar on every page plus a "Data coverage" panel on Overview. The hook shim
  now records its own latency (`~/.kikimimi/shim-latency.log`, rotated) and `kikimimi status`
  prints the p50/p99. Measured numbers are published in
  [How it works](https://isamisushi.github.io/kikimimi/how-it-works/#what-is-missing-measured).

- **Subagent attribution** (KKM-15, architecture.md §7.2 `subagent_fanout`). Three additive
  `kikimimi.v1` columns — `agent_id`, `agent_type`, `query_source` (cloud migration
  `0012_subagent_attribution`) — carry which Agent-tool subagent a row belongs to, from the
  hook payload's `agent_id`/`agent_type`, the transcript's `agentId`/`isSidechain`, and OTel's
  `query_source`/`agent.name`. `session_id` stays the parent's, so every existing per-session
  view already includes the subagents. `kikimimi init` now also registers the `SubagentStart`
  hook (`subagent.start` event); the transcript backfill reads the sidechain transcripts under
  `<session>/subagents/` (Workflow-launched ones included) and emits `subagent.stop` at their
  end instead of session boundaries. New named query `subagents` (local and cloud) and a
  **Subagents** web page (`/web/q/subagents`): per session, subagent count, types, tool calls,
  time share, token share where usage was recorded, and `subagents_with_usage` — the honest
  count of subagents that could be priced at all (Claude Code still loses subagent usage
  upstream: anthropics/claude-code #83430, #88107).
- `context_bloat` now compares requests within one conversation stream (main, or one subagent)
  instead of across the whole session. Parallel subagents alternating 40k/90k/45k/91k were the
  main false positive: on one machine's real data the same-model rule over OTel rows flagged 30
  jumps in one session; the per-stream rule over a full transcript backfill of that machine
  leaves 16 in the busiest session and 27 across 60 sessions.
- A zero-row schema stub (`~/.kikimimi/data/events/dt=_schema/kikimimi.v1-<n>.parquet`),
  written by the daemon at startup and by `kikimimi query`, so local DuckDB queries can name
  a column that this machine's older Parquet predates (DuckDB's `union_by_name` takes the
  union of every file's columns). Without it, `query patterns` on pre-upgrade data failed with
  "Referenced column not found" until the first post-upgrade flush.

- Persisted struggle-pattern detection (architecture.md §7.2, KKM-9). The cloud runs a
  background scanner (`PATTERN_SCAN_INTERVAL_SECS`, default 300) that writes one row per
  incident into a new org-scoped `pattern_hits` table (migration `0010_pattern_hits`):
  `mcp_bypass`, `deny_detour`, `retry_spiral`, `permission_denied_loop`, `context_bloat`,
  `long_tool_tail` and `unused_mcp_server`, each attributed to a `subject` (the MCP
  server or tool) and priced with `wasted_tokens_est` (input+output tokens of the session's
  OTel `api.request` rows inside the incident window; NULL when unknown). Days are rescanned
  while events arrive until 72 hours after they end (`PATTERN_WATERMARK_HOURS`), then frozen;
  late arrivals are counted, not folded in. New named query `patterns` on both sides:
  `kikimimi query patterns` recomputes the same rows live from local Parquet,
  `--cloud` / `GET /v1/query/patterns` reads the table.
- **Struggles page** (web, local and hosted) and `/web/q/patterns` / `/web/q/pattern-hits`
  (KKM-11, architecture.md §7.2 "組織横断ランキング"): the detected patterns aggregated per
  (pattern, subject) — the MCP server or tool to fix, never a person — ranked by
  `wasted_tokens_est × sessions`; unpriced rows show "unknown" and sort last rather than
  pretending to be zero. Clicking a row lists the sessions behind it (metadata only). In a
  team org a member below admin only sees their own sessions in the drilldown, and an
  admin/owner drilldown writes an `audit_log` row, exactly like the Sessions page.
- **Before/after** on the Struggles page (KKM-12, architecture.md §7.3): each ranking row
  opens a per-day trend (sessions, sessions hit, hit rate — the §7.3 KPI — incidents, wasted
  tokens; `/web/q/pattern-timeline`) and an "improvement mark" (`/web/marks`, migration
  `0011_improvement_marks`; locally `~/.kikimimi/marks.json`): "we changed this subject on
  <day>". With a mark the page shows the hit rate and cost before vs. after it. In a team org,
  recording or removing a mark needs admin or owner.
- Docs: [The improvement loop](https://isamisushi.github.io/kikimimi/improvement-loop/) — the
  runbook for running detect → fix → before/after with one team, what each pattern means for
  its subject, and how to sample false positives/negatives from the drilldown.
- `mcp-tax` named query (local and cloud; architecture.md §7.2 `schema_tax`, KKM-13): the
  per-session fixed context (`schema-tax`'s `first_input_tokens`) allocated to the MCP servers in
  each session's `configured_mcp_servers` snapshot — `fixed_tokens_est` (equal split, paid per
  request), `unused_tokens_est` (the share carried by sessions that never called the server) and
  `marginal_first_tokens_est` (median with minus median without the server, when the org's
  configs give a contrast). The scanner persists the same allocation as `unused_mcp_server`
  pattern hits, one per (session, never-called server).

## 0.5.1 - 2026-09-07

Patch release. Re-running `kikimimi init` on 0.5.0 could move the OTLP port out from under
Claude Code; `kikimimi flush` now waits for uploads to finish; licensing is split so
everything you install is Apache-2.0.

### Changed

- License split: the repository root is now Apache-2.0 (agent daemon, CLI, adapters,
  `kikimimi.v1` schema, detection SQL, local web UI, docs). Only `crates/cloud/` (the
  hosted cloud service) stays FSL-1.1-Apache-2.0, in `crates/cloud/LICENSE.md`. Previous
  releases were FSL-1.1-Apache-2.0 as a whole while `Cargo.toml` claimed Apache-2.0; the
  two now agree. See `NOTICE`.

### Fixed

- `kikimimi flush` now waits until the daemon has flushed every sink and reports the
  outcome (`flush ok file=1 s3=1`, or `flush error s3: <reason>` with exit code 1).
  Before, it printed `flush acked by daemon: true` the moment the control byte was
  written, so an s3 upload still sitting in its retry loop (up to 3 x 60 s) was
  indistinguishable from a flush that never ran, and a container's `postStopCommand`
  could exit before the upload finished (GitHub #1). Against a daemon older than this
  release it still returns immediately and says so.
- `kikimimi init` no longer mistakes its own running daemon for a foreign process on the
  OTLP port. Re-running `init` while a 0.4.x `kikimimi agent &` (or the installed service)
  was still bound to 4318 picked a random alternate port, persisted it in `config.json`,
  and left `OTEL_EXPORTER_OTLP_ENDPOINT` at the old value ("differs, leaving unchanged"),
  so Claude Code kept exporting to a port nobody listened on. `init` now keeps the port
  when the daemon behind the control socket reports it in `state.json`, and when the port
  does legitimately change it rewrites the endpoint a previous `init` wrote instead of
  warning about it (a user-customized endpoint is still left alone).
  If you hit this on 0.5.0: set `otlp_port` back to 4318 in `~/.kikimimi/config.json` and
  restart the service (`systemctl --user restart kikimimi-agent` / `launchctl kickstart -k
  gui/$(id -u)/dev.kikimimi.agent`), or run `kikimimi init` again with this fix.

## 0.5.0 - 2026-09-04

**Upgrade note: re-run `kikimimi init` after updating** (`brew upgrade kikimimi`, or
`kikimimi self-update` for shell-installer installs). `init` installs the daemon as a user
service and mints the OTLP bearer token. Until you do, the daemon keeps running the old way,
the local OTLP receiver stays unauthenticated (fail-open) and `kikimimi status` warns about it.

- If you started `kikimimi agent` by hand (nohup, a shell alias, a login item), `init` stops
  that process before installing the service, so the service takes over cleanly. Remove the
  alias or login item afterwards, or the next login starts a second daemon that exits on the
  socket liveness check.
- Claude Code sessions that were already open when you ran `init` still export OTel without
  the new header, so their telemetry is rejected until you restart them. `kikimimi status`
  warns with the rejected count.

### Added

- `kikimimi service install|uninstall|status`: runs the daemon as a user service
  (macOS LaunchAgent / Linux `systemd --user`) so it survives reboots and crashes.
  `kikimimi init` installs and starts it (`--no-service` to skip, `--dry-run` to preview),
  `kikimimi uninstall` removes it, `kikimimi status` reports it, and self-update hands
  the restart to the service manager.
- Per-install bearer token on the local OTLP receiver: `init` mints it once (re-running
  `init` keeps it), writes `OTEL_EXPORTER_OTLP_HEADERS` into the Claude Code settings and
  persists it in `config.json`. The daemon requires `Authorization: Bearer` on `/v1/logs`,
  `/v1/metrics` and `/v1/traces`, reloads the token on the `r` control byte and counts
  rejections; `kikimimi status` warns when the receiver is unauthenticated or has rejected
  requests.
- `configured_mcp_servers` on Claude Code `session.start` (additive `kikimimi.v1` column):
  the names of the MCP servers configured in `~/.claude/settings.json`, `~/.claude.json`
  (top level and the per-project entry) and `<cwd>/.mcp.json`. The cloud `unused-mcp`
  query and `/web/q/unused-mcp` use that snapshot to list servers that are configured but
  never called, even with zero events; the web endpoint carries a `configured_from_snapshot`
  flag and, on the cloud, falls back to the 30-day observed set for sessions without a
  snapshot. The web MCP page shows an **Unused** badge and a Configured column. Cloud
  migration `0009_configured_mcp_servers`. (`kikimimi query unused-mcp` on a local install
  keeps reading the live config files.)
- One-shot backfill of Claude Code transcripts that ended before collection began
  (`~/.claude/projects`, metadata only, `source=log`). The boundary is the earliest local
  `dt=` partition, else the first daemon start, and is persisted once. Progress is
  checkpointed per batch in `~/.kikimimi/claude-backfill.json`; the handoff to the cloud
  and s3 sinks is throttled so a long history drains gradually instead of overflowing the
  pending buffer. Opt out with `claude_backfill = false` in `config.json` or
  `KIKIMIMI_NO_CLAUDE_BACKFILL=1`. `kikimimi status` shows a `claude backfill` block.

### Changed

- MCP servers whose name contains `playwright`, `browser`, `chrome`, `webfetch` or
  `puppeteer` (case-insensitive; covers Playwright, claude-in-chrome and puppeteer) are
  classified as `tool_kind = browser`, so `bypass`, `thrash` (deny_detour) and `reach` see
  browser detours that travel over MCP. `mcp_server` / `mcp_tool` are still set, so MCP
  health and `unused-mcp` are unaffected.
- `kikimimi init` prints a NOTE when the `duckdb` CLI is missing; the README and docs
  disclose the dependency. `kikimimi query skills` is documented as reporting invoked
  skills only (unused-skill detection is not built).

### Fixed

- Hook and OTel rows for the same `tool.result` are deduplicated in every aggregation
  (`tools`, `mcp`, `skills`, `today`, `bypass`, `thrash`, `sessions`), in local DuckDB and
  in the cloud SQL. The OTel row wins, the hook row is the fallback.
- Cloud: the `kikimimi_app` role password is reconciled with `KIKIMIMI_APP_DB_PASSWORD`
  on every boot (#3).
- The "another instance is already listening" startup error no longer prints the
  `kikimimi agent:` prefix twice.

## 0.4.0 - 2026-09-01

- Thrash detection (`kikimimi query thrash`: repeat_failure / deny_detour).
- Member usage view (admin/owner only, audited, alphabetical).
- `events.repo` populated for Claude Code hooks from the cwd git remote (#4).
- `reach` crash fix (hive partitioning disabled in `read_parquet`).
- Documentation site.

## 0.3.0 - 2026-09-01

- Skill usage recording for Claude Code and Codex (`kikimimi query skills`).
- Account model: GitHub OAuth sign-in, personal and team orgs with invite links,
  owner/admin/member/viewer roles with an audit log, one active org per machine with a
  repo-pattern filter, device management (`kikimimi orgs`, `kikimimi devices`,
  `kikimimi repos`).
- Parquet tmp-file fix.

## 0.2.0 - 2026-09-01

- Codex CLI collection: rollout JSONL tailer and normalizer (shell, MCP and token events).
- `kikimimi self-update` (Homebrew installs are detected and deferred to `brew`),
  daemon restart after update, 24-hour update notice in `kikimimi status`.

## 0.1.1 - 2026-08-31

- First public release: Homebrew tap and shell installer, Claude Code collection,
  local Parquet + DuckDB queries, kikimimi cloud sink and web UI.
