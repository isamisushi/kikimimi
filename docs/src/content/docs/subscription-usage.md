---
title: Subscription usage by account
description: Track Claude and Codex subscription windows separately for each account.
---

The local Overview shows **Subscription usage by account**: each contract's usage
percentage, window duration, reset time, and observation time. Filter by account
or compare all accounts. These are subscription limits, separate from token totals
and estimated API costs.

Assign an explicit label such as `personal` or `work` to each subscription. Labels
are scoped to the provider: Claude `work` and Codex `work` are separate accounts.
Use different labels for different contracts with the same provider. Labels are
user-assigned, not verified provider identities. When switching the account logged
into a profile, also change its label. Historical session logs are not assigned to
the account currently logged in.

## Codex

Fetch current limits from an already authenticated, dedicated profile:

```sh
kikimimi usage codex --account personal --profile /absolute/path/to/personal-codex
kikimimi usage codex --account work --profile /absolute/path/to/work-codex
```

The profile is the Codex home directory (normally `~/.codex`, expanded to an
absolute path). The command starts `codex app-server`, uses its documented
[`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server) method,
and closes it after the response. It does not start a conversation. Codex handles
authentication; kikimimi does not copy credentials. Only the normalized limits and
the supplied label are saved. The command times out after 30 seconds.

Run the command again to update the observation, or schedule it with your usual
task scheduler. The dashboard's Refresh button reloads saved observations; it does
not contact the provider. API-key profiles may not expose subscription limits.

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
