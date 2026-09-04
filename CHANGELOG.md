# Changelog

All notable changes to kikimimi. The GitHub release for each tag reproduces the matching section below.

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
