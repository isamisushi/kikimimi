---
title: kikimimi
description: Observability for AI coding agents — cost, thrash, and context tax, recorded through hooks and OpenTelemetry, never a proxy.
---

kikimimi records what your AI coding agents actually do — Claude Code today, Codex CLI too — through each agent's own native mechanisms: hooks and OpenTelemetry for Claude Code, session-log tailing for Codex. No proxy, no TLS interception, no per-tool setup beyond one `kikimimi init`.

Everything lands in local Parquet first. Sharing anything past your own machine — a team dashboard, your own S3 bucket — is opt-in.

## Why

### Cost visibility

The invoice is the first signal for most people, and it shows up days late. kikimimi reads the same token/cost numbers Claude Code's own OpenTelemetry export already carries and writes them locally as they happen — `kikimimi query today` is live, not a monthly surprise.

### Thrash

A stuck agent gets creative: retrying the same failing tool call over and over, or working around a permission denial with bash. `kikimimi query thrash` finds both patterns per session. It's a v0 proxy, not a certainty — see [Quickstart](/kikimimi/quickstart/) for exactly what it does and doesn't catch.

### Context and schema tax

MCP servers you configured but never use still ship their schemas with every request. `kikimimi query unused-mcp` shows which ones are configured but idle; `kikimimi query skills` shows which skills actually get invoked (detecting *unused* skills the way `unused-mcp` does for MCP servers isn't built yet). `kikimimi query schema-tax` estimates how much of a session's first-turn context is fixed overhead rather than the actual prompt — labeled a proxy in its own output, because it is one.

## Install

```sh
brew install isamisushi/tap/kikimimi
```

See [Installation](/kikimimi/installation/) for the install script, platform support, updating, and the `duckdb` CLI that `kikimimi query` and `kikimimi web` need on `PATH`.

## Quickstart

```sh
kikimimi init  # writes hooks + OTel env into your agent settings (backs up first);
               # also installs the daemon as a user service (launchd/systemd --user)
               # so it survives reboots and crashes -- kikimimi agent & still works too
kikimimi web   # local dashboard on 127.0.0.1 — nothing leaves your machine; needs duckdb on PATH
kikimimi query thrash      # stuck-agent incidents
kikimimi query unused-mcp  # context tax you pay for nothing
```

See [Quickstart](/kikimimi/quickstart/) for what each command actually does.

## What it never does

Metadata only, by default and by schema: tool names, MCP server/tool, skill names, durations, success/failure, tokens, cost, session/repo identifiers. Prompt text, tool arguments, and command lines have columns reserved for them in the schema, but nothing populates those columns yet — opt-in content collection is a later stage. Anything sent to the hosted cloud has those columns nulled server-side too, as a second layer, not just a client promise. The hook shim always exits `0`: kikimimi failing must never break or slow your agent.

## All your machines, one place (free)

`kikimimi login` (GitHub device-code flow) syncs metadata-only events from every machine you use — laptops, VMs, CI — into your personal org on [kikimimi.dev](https://kikimimi.dev). Free for individuals. The local `kikimimi web` dashboard only ever shows this machine; the hosted one shows all of them. Nothing beyond metadata leaves the machine — see [What it never does](#what-it-never-does) above.

## Teams (optional)

Create a **team** org instead of using your personal one to add invite links, roles with audited admin drilldowns, and a per-machine repo allowlist. See [Teams](/kikimimi/teams/) for the full picture.

## Status

Early and moving fast. The `kikimimi.v1` schema is additive-only but may still grow. Where kikimimi can't measure something, it reports that as unknown rather than estimating it — the `thrash` and `schema-tax` queries call out their own known blind spots directly in their descriptions rather than hiding them.

Licensed [FSL-1.1-Apache-2.0](https://github.com/isamisushi/kikimimi/blob/main/LICENSE.md) — free for personal and internal (including commercial) use; each release converts to Apache-2.0 after two years.
