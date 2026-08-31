# kikimimi

Observability for AI coding agents — see what your agents actually do, locally first.

kikimimi collects activity from the coding agents you already use (Claude Code today; Codex CLI, Gemini CLI, Cursor, Copilot next) through their **own native mechanisms** — hooks, OpenTelemetry export, session logs. No network interception, no MITM, no per-app configuration beyond one `kikimimi init`.

## What you get

- **Local dashboard** (`kikimimi web`): tool calls, failures, token/cost breakdown, MCP server health — served from `127.0.0.1`, reading local Parquet. In this mode, nothing ever leaves your machine.
- **Queries** (`kikimimi query`): `today`, `tools`, `mcp`, `bypass` (did the agent skip an available MCP tool and fall back to curl/Playwright?), `unused-mcp` (configured but never called — pure context tax), `schema-tax` (how much of your input tokens are fixed overhead).
- **Multi-machine sync** (optional): `kikimimi login` sends metadata-only events to a hosted backend so all your machines land in one place. Prompts and tool arguments are never sent.
- **Bring your own bucket** (optional): `kikimimi sink add s3 s3://bucket/prefix --profile work` writes the same Parquet to your own S3-compatible storage. Uploads go through your `aws` CLI — kikimimi never stores credentials.

## Quickstart

```bash
cargo build --release   # requires Rust; prebuilt binaries coming soon
cp target/release/kikimimi /usr/local/bin/
kikimimi init            # writes hooks + OTel env into your agent's settings (backs up first)
kikimimi agent &         # start the daemon
kikimimi web             # open the local dashboard
```

`kikimimi uninstall` reverts everything.

`kkmm` ships alongside `kikimimi` as a short alias for the same binary — both behave identically.

## Principles

1. Agent-native data sources only — no TLS interception.
2. Fail-open: the hook shim always exits 0; kikimimi must never slow down or break your agent.
3. Metadata by default: tool names, durations, tokens. Content is opt-in and off.
4. Honest numbers: what we can't measure is reported as `unknown`, never estimated.
5. Your data is yours: local mode, full export, BYO S3.

## Status

Early. Built in the open; APIs and schema (`kikimimi.v1`) may still change. Issues and feedback welcome.

## License

[FSL-1.1-Apache-2.0](LICENSE.md) — free for personal and internal (including commercial) use; you may not offer it as a competing product or service. Converts to Apache-2.0 two years after each release.
