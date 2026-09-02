---
title: Installation
description: Homebrew, the install script, the kkmm alias, self-update behavior, and uninstalling kikimimi.
---

Every method below installs two identical binaries: `kikimimi` and the short alias `kkmm`. Every command and flag in these docs works the same under either name.

## Homebrew

```sh
brew install isamisushi/tap/kikimimi
```

## Install script

```sh
curl -fsSL https://github.com/isamisushi/kikimimi/releases/latest/download/kikimimi-installer.sh | sh
```

Detects your platform and installs a prebuilt binary — no Rust toolchain needed. This also leaves a cargo-dist install receipt next to the binary, which is what lets `kikimimi self-update` (below) upgrade in place later.

## Platforms

Prebuilt binaries are published for:

| OS    | CPU             | libc          |
| ----- | --------------- | ------------- |
| macOS | Apple Silicon   | —             |
| macOS | Intel           | —             |
| Linux | x86_64          | glibc         |
| Linux | aarch64         | glibc         |
| Linux | x86_64          | musl (static) |
| Linux | aarch64         | musl (static) |

No Windows builds are published.

## duckdb CLI (optional but needed for `kikimimi query` and the local dashboard)

`kikimimi init`, `kikimimi agent` (hooks, the daemon, OTel/rollout-log collection, and cloud/S3 sync) all work with no dependencies beyond the `kikimimi` binary itself. Named queries (`kikimimi query <name>`) and the local dashboard's `/web/q/*` widgets are the exception: both shell out to the external `duckdb` CLI against local Parquet, so it needs to be on `PATH` for those specifically.

```sh
brew install duckdb
```

Or grab a prebuilt binary from a [GitHub release](https://github.com/duckdb/duckdb/releases) or [duckdb.org](https://duckdb.org) and put it on `PATH` — no package manager required (apt doesn't carry a `duckdb` package). `kikimimi init` checks for it and prints a one-line note if it's missing; `kikimimi status` reports the same thing under its warnings. Without it, `kikimimi query` fails with an explanatory error and the dashboard's widgets return `503`.

## Updating

`kikimimi self-update` figures out how this install got here and does the right thing for it:

- **Install script**: reads the install receipt the script left behind, downloads and installs the latest release in place.
- **Homebrew**: no receipt to read — Homebrew owns that binary. `self-update` detects the Homebrew/Linuxbrew install path and instead prints `brew upgrade isamisushi/tap/kikimimi` and exits `0`. Nothing was updated, but nothing failed either.
- **Anything else** (built from source, `cargo install`, a binary placed by hand): no receipt, not Homebrew-managed — `self-update` prints the same install-script one-liner from above and exits `0`.

```sh
kikimimi self-update            # upgrade in place, or print the right command for this install
kikimimi self-update --check    # report only — never installs, never restarts anything
```

If `kikimimi agent` is running under the old binary when a real update lands, `self-update` stops it (`SIGTERM`, then `SIGKILL` after 5s if it doesn't exit) and restarts it from the new binary automatically — no manual restart needed.

Separately, a running daemon checks quietly in the background: once a day (first check 10 minutes after startup), it polls GitHub's releases API and caches the result at `~/.kikimimi/update-check.json`. `kikimimi status` reads that cache — never a live network call of its own — and prints `update available: vX.Y.Z (run: kikimimi self-update)` when it's behind. Set `KIKIMIMI_NO_UPDATE_CHECK=1` to disable this entirely: no task is spawned, no request is ever made.

## Uninstall

```sh
kikimimi uninstall               # revert hooks/env only
kikimimi uninstall --purge-data  # also delete ~/.kikimimi (data + spool + state)
```

`kikimimi uninstall` reverts exactly what `kikimimi init` added to `~/.claude/settings.json`: the hook entries it wrote and the env vars it set, as long as nothing else has changed them since. Any hooks or settings you added yourself — or edited since `init` last ran — are left untouched.

By default your recorded data under `~/.kikimimi` is left in place. Pass `--purge-data` to delete it too.
