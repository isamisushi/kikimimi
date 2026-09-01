---
title: Development
description: Workspace layout, building and testing kikimimi, running your own kikimimi cloud, and the docs site.
---

## Workspace layout

A Cargo workspace plus one Vite/React SPA:

```
crates/
  schema           kikimimi.v1 event schema
  spool            fail-open on-disk queue the hook shim writes to
  otlp             OTLP/HTTP receiver (Claude Code's OTel export)
  adapter-claude   Claude Code hook payload -> Event
  adapter-codex    Codex rollout session log tailer -> Event
  sink             file / cloud / BYO S3 sinks
  cli              the kikimimi/kkmm binary (hook, agent, query, login, sink, repos, ...)
  cloud            kikimimi cloud: the kikimimi-cloud binary (Postgres, GitHub OAuth, /v1 + /web APIs)
web/               Vite + React SPA — embedded by both `kikimimi agent` (local web UI)
                   and kikimimi cloud (kikimimi.dev), same codebase either way
```

## Build and test

```sh
cargo build --workspace
cargo test --workspace
```

Two things `cargo test --workspace` needs on `PATH`/reachable, or the relevant tests fail:

- **Postgres**, for `crates/cloud`'s tests. They default to `postgres://postgres:guru-dev@127.0.0.1:5433/guru` if `DATABASE_URL` isn't set — point `DATABASE_URL` at your own instance instead if you'd rather not run one on port 5433.
- **The `duckdb` CLI**, for `crates/cli`'s query tests (`kikimimi query` shells out to it, same as it does at runtime).

The web UI has its own build, independent of `cargo`:

```sh
cd web && npm run build   # tsc --noEmit && vite build
```

## Running your own cloud

`kikimimi cloud` (crate `crates/cloud`, binary `kikimimi-cloud`) is a self-contained server: it runs its own Postgres migrations on startup — connecting is enough, nothing separate to invoke first. Configuration is environment variables, the ones you'll actually need to set:

- `DATABASE_URL` — Postgres connection string (superuser; the server also opens a second, RLS-scoped pool internally).
- `BIND_ADDR` — defaults to `127.0.0.1:8787`.
- `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` — GitHub OAuth app credentials. Without both set, GitHub sign-in 503s.
- `KIKIMIMI_INVITE_CODE` — enables a legacy email+invite-code login as a bootstrap path when you haven't wired up a GitHub OAuth app yet.

Point a device at it (see [Teams](/kikimimi/teams/#self-hosting)):

```sh
kikimimi login --endpoint https://kikimimi.your-company.internal
```

The repo's own `Dockerfile` and `fly.toml` are the example deploy: `cargo build --release -p kikimimi-cloud`, a slim runtime image, `BIND_ADDR=0.0.0.0:8787`, deployed to Fly.io. Neither is required — they're just what's checked in and actually used to run kikimimi.dev.

## The docs site

This site (`docs/`, Astro + Starlight) builds and deploys to [isamisushi.github.io/kikimimi](https://isamisushi.github.io/kikimimi/) automatically on a push to `main` that touches `docs/**`.

```sh
cd docs && npm run dev      # local preview, live reload
cd docs && npm run build    # astro build, same as the CI deploy step
```

## Contributing

Built in the open — issues and PRs welcome on [github.com/isamisushi/kikimimi](https://github.com/isamisushi/kikimimi).

## License

[FSL-1.1-Apache-2.0](https://github.com/isamisushi/kikimimi/blob/main/LICENSE.md) — free for personal and internal (including commercial) use; you may not offer it as a competing product or service. Each release converts to Apache-2.0 two years after it ships.
