---
title: Subscription usage by account
description: Track Claude and Codex subscription windows separately for each account.
---

The local Overview shows **Subscription usage by account**: each contract's usage
percentage, window duration, reset time, and observation time. Filter by account
or compare all accounts. These are subscription limits, separate from token totals
and estimated API costs.

Codex accounts are identified automatically by their authenticated account ID.
Claude accounts use explicit labels such as `personal` or `work`. Accounts are
scoped to the provider. Historical session logs are never assigned to the account
currently logged in.

## Codex

While `kikimimi agent` is running, it fetches limits at startup and every five
minutes after each attempt. It reads `tokens.account_id` from `auth.json` under
`CODEX_HOME` (or `~/.codex`) and stores the observation under that ID. No account
label or scheduler setup is required. Only the currently authenticated account in
that profile is polled; switching accounts creates a separate observation. Old
accounts retain their last observation and become stale.

To fetch immediately, or use a different profile:

```sh
kikimimi usage codex
kikimimi usage codex --profile /absolute/path/to/work-codex
```

`--account work` remains available as an explicit storage/display label. Such
labels are user-assigned, not verified identities, and are separate from the
ID-based observations collected by the agent. Do not reuse one label across
contracts.

Each fetch starts `codex app-server`, calls its official
[`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server) method,
and closes it after the response. It does not start a conversation. Codex handles
authentication. The collector reads only the account ID from `auth.json`; it does
not store or log tokens. The request times out after 30 seconds. If the local
account ID changes during the request, or the response's account ID disagrees,
the observation is discarded and retried on the next cycle.

Missing `auth.json` or missing account IDs (including API-key-only profiles) are
skipped by automatic collection. Profiles using other credential stores without
an ID in `auth.json` require an explicit `--account` for manual collection.
Failures retain the previous observation and do not interrupt event ingestion.
Set `KIKIMIMI_NO_CODEX_USAGE=1` in the agent's environment to disable polling.
The Codex executable must be on the agent's `PATH`, and the agent must inherit
`CODEX_HOME` when using a custom profile.

The dashboard's Refresh button reloads saved observations; it does not contact
the provider. This change does not enable cloud sync.

## Claude Code

Claude's [status-line input](https://code.claude.com/docs/en/statusline) can include
`rate_limits.five_hour` and `rate_limits.seven_day`. In the settings for each
dedicated Claude profile, configure:

```json
{
  "statusLine": {
    "type": "command",
    "command": "kikimimi usage claude --account personal"
  }
}
```

Use `--account work` in the other profile. This command reads native status-line
JSON from stdin, stores only the usage windows, and prints a compact usage line.
If you already have a status-line script, pass the same JSON to this command from
that script and redirect its stdout to `/dev/null` to preserve your existing display.
`kikimimi init` does not replace your existing status-line configuration.

Claude supplies these fields for eligible subscriptions after the first API
response; individual windows may be absent. Missing windows remain unknown, never
zero. No prompt, transcript, or workspace fields from the input are saved.

## Freshness and storage

```sh
kikimimi usage
```

Prints the latest snapshots as JSON. Concurrent updates are serialized per account;
older observations cannot replace newer ones, and percentages are never summed.
The UI reloads every minute, flags observations older than 15 minutes, and marks
passed reset times as awaiting a new observation rather than assuming a zero balance.

Snapshots are stored under `~/.kikimimi/data/events/subscription-usage/` (or the
corresponding `KIKIMIMI_DIR`). This initial implementation is local only: no cloud
sync or cross-machine merging. Use the same labels consistently on your machines,
but each local dashboard currently shows only its own observations.
