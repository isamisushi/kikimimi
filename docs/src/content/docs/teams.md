---
title: Teams
description: GitHub sign-in, personal vs. team orgs, invite links, roles, the Members usage view, and the per-machine repo allowlist.
---

Signing in and joining a team org are both optional — `kikimimi web` and local Parquet work with no account at all. This page covers what changes once you do sign in.

## Sign in

```sh
kikimimi login
```

`kikimimi login` never opens a browser itself — it prints a code and waits for you to approve it on the web:

```
To authorize this device, open:
  https://kikimimi.dev/device
and enter code: ABCD-1234
waiting for approval...
```

Approving on [kikimimi.dev](https://kikimimi.dev) goes through GitHub OAuth sign-in. On success the CLI prints which account and org it landed on:

```
logged in as you@example.com (org you [personal])
```

`kikimimi logout` revokes the token on the server (best-effort) and forgets it locally either way.

## Personal vs. team orgs

Your first GitHub sign-in auto-creates a **personal** org — just you, always owner, never filtered (see [repo allowlist](#repo-allowlist) below). Anyone can also create a **team** org on kikimimi.dev; the creator becomes its owner and invites the rest of the team.

`kikimimi orgs` lists every org your account belongs to, across both kinds, and marks the one this device is currently bound to:

```
orgs:
  you       personal  owner   (this device)
  acme      team      admin
```

## Invite links

An admin or owner mints an invite link for a team org from the web UI — role, an optional expiry, and an optional use-count limit are all set at creation time. Following the link and signing in with GitHub joins that org at the invited role; a revoked, expired, or exhausted invite is rejected with the specific reason. An admin can't mint an invite for a role above their own (an admin can't invite another owner).

## Roles

Four roles, in order: **owner**, **admin**, **member**, **viewer**. They govern what a team org's drilldowns show:

- **member** / **viewer** in a team org see only their own sessions when they open a per-session drilldown.
- **admin** / **owner** see every member's sessions — and every one of those drilldowns writes a row to the org's audit log (who looked, when).
- A **personal** org has no "other members" to scope away from, so it always behaves like the unscoped, unaudited admin path — it's just you.

## Members usage view

The "Members" view on kikimimi.dev is a per-member usage rollup — sessions, API requests, tool calls/failures, token totals, estimated cost — over the trailing 30 days. In a team org it's admin/owner-only (a member or viewer gets turned away, both in the UI and if they hit the API directly) and every view of it writes an audit log row, same as an admin's session drilldown.

It's sorted alphabetically by member, on purpose — not by cost or usage. This is meant to explain what's driving a number (loops, heavy cache re-reads), not to rank people by spend.

A **loop-suspect sessions** column flags members with at least one session that made 50+ API requests — worth a look for a runaway loop, not proof of one. Any nonzero count surfaces both an inline badge on that row and a summary callout above the table.

## Per-machine org binding

A device is bound to exactly one org's cloud sink at a time — there's no per-request switching. Bind (or re-bind) a machine with:

```sh
kikimimi login --org acme
```

`--org` is only a hint for pre-selecting the org on the approval page; the org a device actually ends up bound to is whatever gets approved there, server-side. Signing in again keeps this machine's locally-configured [repo allowlist](#repo-allowlist) — it isn't reset just because you re-authenticated.

## Repo allowlist

```sh
kikimimi repos allow 'github.com/acme/*'
kikimimi repos list
kikimimi repos remove 'github.com/acme/*'
```

This allowlist only ever matters for a **team** org's cloud sink: an event is pushed to the team cloud only when its repo matches one of these globs. A personal org is never filtered, even if patterns are configured — `kikimimi repos` says so directly when you're on one:

```
(repo filter only applies to team orgs; this org's events are never filtered)
```

The **local Parquet file** and any **BYO sink** (S3, see [Sinks](/kikimimi/sinks/)) always receive every event, unfiltered — this allowlist only controls what leaves your machine for the shared team cloud.

Leaving the list empty doesn't mean "block everything" — it means "send everything," and `kikimimi status` warns about it:

```
repo filter: none configured -- every event is sent to the team cloud unfiltered (run `kikimimi repos allow <glob>` to restrict)
```

**SSH vs. HTTPS form.** Claude Code and Codex don't record a repo URL the same way: Claude Code keeps whatever form the git remote actually uses (often SSH, `git@github.com:acme/api.git`), while Codex records HTTPS. A glob anchored to one scheme will silently miss the other. Shape it to match both instead:

```sh
kikimimi repos allow '*acme/api*'
```

An event with no repo at all (hooks outside a git tree, OTel events, which never carry `cwd`) is treated as **not matching** a non-empty allowlist — conservative by design, since there's no repo to confirm against.

## Self-hosting

`kikimimi login` talks to `https://kikimimi.dev` by default. Point it at your own instance instead:

```sh
kikimimi login --endpoint https://kikimimi.your-company.internal
# or, to avoid repeating the flag:
export KIKIMIMI_ENDPOINT=https://kikimimi.your-company.internal
kikimimi login
```

`--endpoint` always wins if both are set; a previously-saved login is used if neither is. See [Development](/kikimimi/development/) for running `kikimimi cloud` yourself.
