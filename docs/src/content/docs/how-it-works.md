---
title: How it works
description: How kikimimi collects agent activity per agent, stays fail-open under any failure, and normalizes everything into one local schema.
---

kikimimi never sits in the network path. It reads each agent's own instrumentation — hooks, OpenTelemetry export, session logs — and normalizes what it finds into one schema. No proxy, no TLS interception, no per-tool setup beyond `kikimimi init`.

## Collection model

### Claude Code

Two independent sources, both wired up by `kikimimi init`:

**Hooks.** `kikimimi init` writes a `kikimimi hook <EVENT>` command into `~/.claude/settings.json` for `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionDenied`, `SessionStart`, `SessionEnd`, `SubagentStart`, and `SubagentStop` — backing up the existing file first, and idempotent on repeat runs. `kikimimi uninstall` removes exactly what `init` added, nothing more. Hooks give tool name, MCP server/tool, duration, success/failure, and permission decisions. They do not give tokens, cost, or model — the one exception is `SubagentStop`, whose payload sometimes carries its own `usage` block. Inside a subagent every hook carries `agent_id` and `agent_type`, which is how kikimimi tells a subagent's tool calls from the parent's while keeping them in the parent's session ([subagents](/kikimimi/queries/#subagents)). `SessionStart` additionally carries the names of every MCP server configured for that session — the basis for the [unused-mcp](/kikimimi/queries/#unused-mcp) query's configured-vs-called distinction.

**OTel.** `kikimimi init` also sets `CLAUDE_CODE_ENABLE_TELEMETRY=1` and points `OTEL_EXPORTER_OTLP_ENDPOINT` at the daemon's own local OTLP/HTTP receiver (`http://localhost:4318` by default — `init` checks for a port conflict first and rewrites every affected agent config together if it has to pick a different one). This is where model, input/output/cache tokens, and cost come from, via `api_request`, `api_error`, `tool_result`, `tool_decision`, `user_prompt`, and `compaction` log records. (kikimimi accepts both the documented `claude_code.api_request`-style event names and the unprefixed `api_request` form actually seen on the wire.) The receiver only accepts requests carrying the per-install bearer token `init` writes into `OTEL_EXPORTER_OTLP_HEADERS` (`Authorization: Bearer <token>`) — otherwise anything else on the machine could POST fake OTLP data and have it recorded as a real session. Claude Code sessions already running when you (re-)run `init` need a restart to pick up a newly-set or rotated token.

**Print mode.** `claude -p` (headless/non-interactive) is not a blind spot: verified on Claude Code 2.1.251, both hooks and OTel export flow in `-p` runs the same as interactive sessions, so CI and scripted invocations show up in kikimimi like any other session. (An earlier Claude Code release shipped a bug where `-p` sent no OTel at all — see [anthropics/claude-code#46338](https://github.com/anthropics/claude-code/issues/46338) — that no longer reproduces on current versions.)

**Backfill.** The very first time `kikimimi agent` starts, it also reads through `~/.claude/projects/**/*.jsonl` — the session transcripts Claude Code already keeps on disk — and normalizes whatever it finds there into the same schema, metadata only, exactly like every other source (no prompt/tool-argument/response text is ever copied out, the one standing skill-name exception). This is what makes a heavy user's existing history show up in the dashboard right away instead of a blank slate that only fills in from that point forward. It's one-shot and conservative: only sessions that finished *before* hooks/OTel started collecting on this machine are backfilled — a session still open past that point risks being double-counted (the query layer only dedups the hook/OTel `tool_result` pair, not `tool_call`/`api_request` rows across sources), so it's skipped instead, permanently, once detected. The cutoff itself is the oldest date kikimimi already has local Parquet for, or (on a truly fresh install with nothing local yet) the very first moment `kikimimi agent` ran. Each file's outcome is recorded in `~/.kikimimi/claude-backfill.json` as soon as it's decided, so restarting the daemon never re-reads a file it already finished. A file still being backfilled when the daemon stops (a restart, a crash, an OOM-kill) checkpoints its progress every batch rather than only at the end, so a restart resumes close to where it left off instead of re-reading the whole file from the start. A backfilled session also gets a `configured_mcp_servers` snapshot (from the MCP tool names Claude Code listed in the transcript, so claude.ai connectors count too) and a `configured_skills` snapshot (Claude Code's own skill listing) on its `session.end` — the transcript equivalent of what the `SessionStart` hook reads from the settings files, and the basis of [unused-skills](/kikimimi/queries/#unused-skills). Turn it off with `claude_backfill: false` in `~/.kikimimi/config.json`, or `KIKIMIMI_NO_CLAUDE_BACKFILL=1`. `kikimimi status` shows its progress. Same fail-open rule as everywhere else: Anthropic documents this transcript format as internal, not a stable contract, so a line kikimimi doesn't recognize is skipped and counted, never a crash.

### Codex CLI

No hooks are installed for Codex. `kikimimi init` deliberately does not touch `~/.codex/config.toml` — the installed `codex-cli`'s hooks/`[otel]` config schema couldn't be confirmed from `codex --help`/`codex doctor` on the machines this was tested on, so Stage 0 relies entirely on tailing `~/.codex/sessions/**/rollout-*.jsonl`, which `kikimimi agent` watches with zero setup once it detects `~/.codex`. Tool calls come from completed `event_msg.item_completed` records (`item.type == "CommandExecution"`, arriving as one finished record rather than a begin/end pair); session identity, agent version, model provider, and `repo` come from the `session_meta` record each rollout file starts with — `repo` specifically from that record's `git.repository_url`.

**Codex MCP servers.** Every Codex `session.start` carries the same `configured_mcp_servers` snapshot Claude Code's does, read from the `[mcp_servers.<name>]` tables in `$CODEX_HOME/config.toml` (names only; commands, URLs and env values are never read). So [unused-mcp](/kikimimi/queries/#unused-mcp) and the `unused_mcp_server` pattern work for Codex sessions too.

**Codex denials — not detected yet.** Codex has no `tool.denied` in kikimimi, so `deny_detour` and `permission_denied_loop` are Claude Code only. What was checked (codex-cli 0.151.0, 2026-09-07): the binary's approval vocabulary is `ReviewDecision` = `approved` / `approved_for_session` / `approved_execpolicy_amendment` / `approved_mcp_policy_amendment` / `denied` / `abort` / `timed_out`, with `ExecCommandApprovalParams` / `ApplyPatchApprovalParams` request shapes, and `thread_settings_applied` records the session's `approval_policy` (`on-request` here) and `permission_profile`. But the two rollout files on the test machine contained no denial at all — every `CommandExecution` item has `status: "completed"` — so which record a denied command leaves on disk (a `CommandExecution` with another `status`, or nothing) is unconfirmed, and kikimimi does not guess. The `PermissionRequest` hook stays unmapped for the same reason: it fires when approval is *asked*, not when it is refused. The way to close this is one real session where a command is denied in the TUI, then reading the rollout it left.

**Hook ↔ rollout correlation (§15-3).** Not testable yet: `kikimimi init` installs no Codex hooks, so there are no Codex hook rows to correlate with rollout rows. Codex tool events therefore have `correlation_confidence = none` and are never deduped against another source.

### Skills

Both agents' skill invocations are recorded as metadata only — the skill's name, never its arguments or its instructions:

- **Claude Code**: pulled from the hook payload's `tool_input.skill` field, the one deliberate exception to "never copy `tool_input`."
- **Codex**: Codex has no dedicated skill-invocation event, so kikimimi infers one. Codex's filesystem skills work by having the agent read a `SKILL.md` before following it, so kikimimi scans each executed shell command for a path ending in `/SKILL.md`; if it finds one, the parent directory name becomes `skill_name`. A `SKILL.md` read is a reliable stand-in for "a skill just started."

### Repo attribution

Claude Code hook events carry a `cwd`, not a repo URL, so kikimimi resolves one itself: it reads `.git/config` directly from `cwd` (walking up through worktree `gitdir:`/`commondir:` indirection where needed) rather than shelling out to `git`, and caches the result per `cwd` for the life of the daemon. Codex gets `repo` for free from the rollout's own `git.repository_url`. The two don't necessarily agree on *form* — Claude Code keeps whatever a remote's URL happens to be (often SSH, `git@github.com:org/repo.git`), while Codex records HTTPS — so a [repo allowlist](/kikimimi/privacy/) glob meant to match both should be shaped like `*org/repo*`, not anchored to one scheme.

## What is missing, measured

Every number kikimimi shows is a proxy over metadata, and some of the metadata never arrives. Rather than hide that, the web UI carries a **coverage** badge in the top bar and a "Data coverage" panel on Overview (`/web/q/coverage`, local and hosted), and `kikimimi status` prints the hook shim's own latency. The counts behind each rate are in the response so the percentages stay auditable; a rate with nothing to divide shows "–", never 0%.

| Rate | What it means | One machine, 30 days (2026-09-07) |
|---|---|---|
| Sessions with usage | sessions with at least one `api.request` that carried tokens. The rest are hooks-only (OTel not restarted after `init`, Codex without usage, or `-p` runs on an old version) | 24 of 37 (65%) |
| Hook ↔ OTel match | hook `tool.result`s whose `tool_use_id` OTel also reported — the correlation every dedup rests on | 19,058 of 22,937 (83%) |
| Dedup dropped | `tool.result` rows folded away because hook and OTel reported the same call | 19,058 of 42,069 (45%) |
| Subagents priced | subagents whose usage was recorded anywhere ([subagents](/kikimimi/queries/#subagents)) | 0 of 0 on a hooks-only window; see the query page for a backfilled machine |
| Attributed to a person | events with an account id. Local data has none; on the cloud, rows a device sent without a login | 55,578 of 102,981 locally carry the OTel `user.email` |
| Hosts silent > 24h | devices whose last event is older than a day | 0 of 1 |
| Hook shim latency | what Claude Code waited for per hook (`kikimimi hook`: read stdin, write the spool file, poke the daemon). Measured over 300 invocations on this machine | p50 0.5 ms, p99 0.9 ms |

The shim writes one line per invocation to `~/.kikimimi/shim-latency.log` (rotated at 512 KiB); `kikimimi status` summarizes the newest 2,000. If your p99 is over a few milliseconds, the disk under `$XDG_RUNTIME_DIR` (or `~/.kikimimi`) is the first suspect.

## Fail-open spool design

The hook shim (`kikimimi hook <event>`) is built to never slow down or fail the agent that calls it:

1. Reads stdin, capped at 10 MB.
2. Writes the payload to a temp file and fsyncs it — bounded to 200ms; a write that's still hanging past that is abandoned rather than allowed to block the shim — then publishes it into the spool directory with an atomic rename (one file per hook call, so parallel subagents and concurrent sessions never collide).
3. Makes a best-effort, 50ms-timeout, non-blocking connection to the daemon's Unix socket to say "something's waiting." If the daemon isn't running, this just fails silently.
4. Exits 0. Always. The whole call runs inside `catch_unwind`; even a panic is caught, logged to `~/.kikimimi/shim-errors.log`, and never surfaced to the agent.

The daemon (`kikimimi agent`) drains the spool on its own schedule, independent of any single hook call — so if the daemon is offline, mid-restart, or just slow, hook payloads still land on disk and get picked up once it's back. An entry the daemon can't parse (malformed JSON, an unreadable file) is moved aside into a `.poisoned/` quarantine directory instead of being retried forever or silently dropped, so it stays available for forensics without blocking the rest of the queue. `kikimimi init` registers the daemon as a user-level service (a macOS LaunchAgent, or a Linux `systemd --user` unit) so it comes back on its own after a crash or a reboot, instead of collection silently stopping until someone notices and re-runs `kikimimi agent &` by hand.

## Normalization

Every event — from a hook, from OTLP export, from a rollout line — is normalized into one row of the `kikimimi.v1` schema before it touches disk. The schema is additive-only: a breaking change (dropping or renaming a column) becomes `kikimimi.v2` in a separate module rather than mutating this one in place. Its columns fall into a handful of groups:

- **Identity** — event id, timestamps, org/team/user/host ids, which agent and version, session/turn/parent-session ids, repo, a hashed cwd.
- **Tool** — tool name and kind (`mcp` / `bash` / `browser` / `skill` / `builtin`), MCP server/tool, skill name, duration, success, and permission decisions. A browser-automation MCP server (Playwright MCP, claude-in-chrome MCP, ...) is classified `browser` rather than `mcp` — it's the "alternative channel" the bypass/thrash/reach queries look for — while `mcp_server`/`mcp_tool` stay populated.
- **Model** — provider, model, effort, thinking. Shown on the web **Models** page (per model × effort, org-wide) and in the session detail page's per-session model table, subagent rows and event list ([queries](/kikimimi/queries/#models-web-only)).
- **Usage** — input/output/cache/reasoning tokens, cost, and where those numbers came from (`usage_source`, since not every agent or event type reports them).
- **Body** — `tool_input_json`, `tool_output_excerpt`, `prompt_text`, `redaction_applied`. Off by default; see [Privacy](/kikimimi/privacy/).

`event_id` is computed on the machine that produced the event — a SHA-256 of `host_id`, `source`, `event_type`, and a per-event primary key (the tool's own `tool_use_id` when there is one, otherwise a session-scoped counter), truncated to 32 hex characters — so re-sending the same event after a daemon restart or a retried upload produces the same id every time, and anything downstream can dedup on a plain `event_id` uniqueness check. Because `event_id` hashes in `source`, the same `tool_use_id` reported by both the hook and OTel for one Claude Code tool call keeps both rows on disk as `correlation_key`-matched siblings rather than colliding into one — see [Queries](/kikimimi/queries/)' honesty note for how the queries collapse that pair back into one logical result at read time, preferring OTel.

## Local Parquet layout

The `file` sink is always on, independent of whether you've run `kikimimi login`: every normalized event is written to `~/.kikimimi/data/events/dt=YYYY-MM-DD/*.parquet`, zstd-compressed, partitioned by day only — not by agent or host, since host-partitioning would explode the file count on throwaway VMs and containers. `kikimimi query` and the local web UI (`kikimimi web`) both read this layout directly with DuckDB's `read_parquet(..., union_by_name=true)`, so a Parquet file written before a column existed gets that column filled with `NULL` rather than breaking the read.
