# kikimimi

**See what your AI coding agents actually do.**

kikimimi records the activity of the coding agents you already use — Claude Code and Codex CLI today — through their **own native mechanisms** (hooks, OpenTelemetry, session logs). No proxy, no TLS interception, no per-tool setup beyond one `kikimimi init` — plus the `duckdb` CLI for local queries and the dashboard. Everything lands in local Parquet first; sharing anything is opt-in.

**Full manual: [isamisushi.github.io/kikimimi](https://isamisushi.github.io/kikimimi/)**

## Why

- **"The invoice is the first signal."** Token spend surprises show up days later on a bill. kikimimi records every request's tokens and cost locally, live.
- **Agents complete tasks, but *how*?** A stuck agent gets creative — retrying the same failing tool, or working around a permission denial with bash. `kikimimi query thrash` finds those sessions.
- **Context is a budget.** MCP servers you configured but never use still ship their schemas with every request. `unused-mcp` and `schema-tax` show what that costs; `skills` shows which skills actually get invoked (detecting *unused* skills isn't built yet).

## Install

```bash
brew install isamisushi/tap/kikimimi
# or:
curl -fsSL https://github.com/isamisushi/kikimimi/releases/latest/download/kikimimi-installer.sh | sh
```

`kkmm` is installed alongside as a short alias. `kikimimi self-update` keeps script installs current.

Also needed for queries and the local dashboard: the [`duckdb`](https://duckdb.org) CLI on `PATH` (`brew install duckdb`, or a release binary from duckdb.org). Hooks, the daemon, and cloud sync all work without it — `kikimimi init` just warns if it's missing.

## Quickstart

```bash
kikimimi init  # writes hooks + OTel env into your agent settings (backs up first);
               # also installs the daemon as a user service (launchd on macOS,
               # systemd --user on Linux) so it survives reboots and crashes
kikimimi web   # local dashboard on 127.0.0.1 — nothing leaves your machine; needs duckdb on PATH
kikimimi query thrash      # stuck-agent incidents
kikimimi query unused-mcp  # context tax you pay for nothing
```

No need to run `kikimimi agent` yourself — `init` starts it as a service that comes back after a crash or reboot on its own. `kikimimi service status` shows whether it's installed and running; `kikimimi agent` still works for a one-off foreground/background run (e.g. while debugging). `kikimimi uninstall` reverts exactly what `init` added, service included. Its first-ever start also backfills existing Claude Code session history from `~/.claude/projects`, so sessions that finished before this machine started collecting show up right away instead of a blank dashboard.

## What it records — and what it never does

Metadata only, by default and by schema: tool names, MCP server/tool, skill names, durations, success/failure, tokens, cost, session/repo identifiers. **Prompts, tool arguments, file contents, and command lines are not collected**, and the hosted sink nulls those fields server-side too. The hook shim always exits 0 — kikimimi failing must never break or slow your agent.

## All your machines, one place (free)

`kikimimi login` (GitHub device-code flow) syncs metadata-only events from every machine you use — laptops, VMs, CI — into your personal org on [kikimimi.dev](https://kikimimi.dev). Free for individuals. The local `kikimimi web` dashboard only ever shows this machine; the hosted one shows all of them. Nothing beyond metadata leaves the machine, same as everything above.

## Teams (optional)

Create a **team** org on kikimimi.dev to add: invite links, roles with audited admin drilldowns, and a per-machine repo allowlist so personal repos never reach the company org. Prefer your own storage over kikimimi's cloud? `kikimimi sink add s3 s3://bucket/prefix` writes the same Parquet through your own `aws` CLI — kikimimi never holds credentials.

## Status

Early and moving fast; the `kikimimi.v1` schema is additive-only but may still grow. Built in the open — issues and war stories welcome.

## License

[FSL-1.1-Apache-2.0](LICENSE.md) — free for personal and internal (including commercial) use; you may not offer it as a competing product or service. Each release converts to Apache-2.0 after two years.
