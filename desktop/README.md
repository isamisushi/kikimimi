# kikimimi desktop (macOS preview)

Tauri desktop app with a single-window dashboard, scoped settings, first-run setup,
and a menu-bar entry, sharing the existing Rust collector and React dashboard. No account or Apple membership is needed for local development. This
is a development preview, not yet a signed public release.

The `Desktop preview` GitHub workflow builds an ad-hoc-signed app on a Mac runner
and saves a ZIP as a CI artifact. It does not publish a release or notarize the app.

## Activity filters

Your activity includes your own synchronized devices. The shared Device filter
switches between all your devices, this Mac, and a specific device. Repository
can be combined with Device, including “No repository” for activity without a
repository. These display filters apply across activity pages and survive page
navigation in the current app session; they do not limit collection or cloud sync.

## Build on a Mac

### Local updates on this Mac

From the repository root, run:

```sh
npm run install:local --workspace desktop
```

This builds only this Mac's architecture, using the existing local Rust/Node
caches, and installs `/Applications/kikimimi.app` atomically. It downloads and
caches the official DuckDB 2.0.0 CLI with SHA-256 verification. Rust, Node and
Xcode Command Line Tools are required; missing npm packages are installed once.
The first build can take longer while Rust dependencies compile; later builds
reuse the checkout's caches.
The previous app is kept in `desktop/.cache/local-backups/`. An open installed
app is closed and reopened. A running app-owned collector is restarted; stopped
collection stays stopped. A failed collector restart restores the previous app.
`npm run build:local --workspace desktop` builds without installing.

Local builds use ad-hoc signing for this Mac and manual local updates. They skip
Apple notarization, DMG generation and the public automatic-update channel.
Signed/public distribution remains a separate workflow; Intel CI is not part of
this local command.

### Development and manual builds

Install Xcode Command Line Tools, Rust, Node.js and DuckDB on the **build machine**.
Run `npm ci` at the repository root. Then run `npm run prepare-binaries --workspace desktop -- --debug` once, followed by `npm run dev --workspace desktop`. Build the app with `npm run build --workspace desktop`.
Use Rust 1.93.1 or newer for the test dependencies. Development uses Vite with live reload and the prepared debug collector; release builds embed static assets. Re-run preparation after changing Rust CLI code.
Set `DUCKDB_BINARY` to an absolute executable path if DuckDB is not on PATH.
The preparation script builds both React entry points and the CLI, checks DuckDB's CPU
architecture, and bundles both executables. End users do not install DuckDB.
The signed release workflow pins the official DuckDB 2.0.0 CLI for each CPU and verifies its published SHA-256 before bundling. Source builds are validated with DuckDB 2.0.0; older 1.x CLIs differ in empty-result JSON output.
DuckDB must be a standalone executable linked only to system libraries; the
script rejects Homebrew/private dylib dependencies that would break on a clean Mac.
Build natively on each architecture, or use `release:local -- --arch x86_64`
on Apple Silicon with Rosetta. Universal bundles are not supported.

For a release build, move `kikimimi.app` into `/Applications` before enabling
collection. The app refuses setup from a DMG/translocated location. Do not move
the installed app afterward: hooks and the launchd service use its absolute path.
The development build uses paths inside the checkout; disconnect before deleting
the checkout or switching to the packaged app.

First-run setup requires checking consent and clicking Start collecting. It backs
up Claude settings, installs absolute hook commands, and registers the existing
user service. Existing CLI installs share the same data/config and service;
existing cloud/S3 settings remain active. Subsequent launches open the dashboard
without changing collection settings. A stopped, configured collector offers
Resume, which starts the existing service without rewriting agent settings.

Dashboard and App Settings share one window. The collection bar shows running/stopped
state and the last recorded activity. App Settings contains updates and Disconnect;
use the tabs, the menu-bar menu, or Cmd+, to open it. Closing the window keeps
collection running; reopening shows the dashboard with its navigation retained.
Disconnect removes the integration and service, while saved history remains
available. Reconnecting requires consent again. Update checks never block history
navigation; installation still serializes collection/settings changes.

The desktop owns a separate `desktop serve-dashboard` child process that only
serves saved data. It does not start tailers, OTel, cloud sync, or a login service.
It binds an ephemeral loopback port, delivers its random auth URL over a private
pipe, and shuts down when the owning pipe closes. This keeps history readable
while collection is stopped or disconnected. Quitting the app stops this viewer,
while the independent collector continues.

When collection stops while the app is open, a viewer started during collection
is replaced on the next local history request so it can open the saved ledger.
History remains available without restarting the app.

The collector records structured Codex MCP calls and file edits (`apply_patch`),
including success/failure and duration, without retaining arguments, patches,
file paths, or output bodies. On its first start after this support is installed,
it repairs these two event kinds in previously consumed rollout prefixes, including
compressed logs. Existing Parquet event IDs are checked before writing, so a retry
does not duplicate saved events. The repair affects local history only; it does not
retroactively upload events to Cloud or S3. If a required old log is unavailable,
the collector logs the incomplete repair and retries on its next start.

Codex session, turn and API-usage IDs are based on the observed record rather
than rollout ordinals, which may be absent or reused. Before collecting, the
updated collector rekeys existing Codex metadata and removes exact duplicate
log metadata (including Claude turns). Distinct records are preserved. Changed
Parquet files retain a `*.parquet.before-metadata-v2` backup and are replaced
atomically; retries are idempotent. Unreadable or conflicting data stops repair
before writes. Local metadata replays are also filtered on subsequent flushes.
Consumed rollout prefixes restore turn/model context on both plain and compressed
logs without re-emitting history or inflating live skip counters.

Native commands accept no arbitrary executable, arguments or URLs. Only the
local shell webview can invoke management commands (label and origin checked).
The local history dashboard and settings render in one bundled React/TypeScript
webview. History uses a Rust command with an endpoint allowlist; its loopback
credentials stay in Rust. Cloud pages open in the default external browser; no remote content or Cloud
session cookies are hosted in the app.

Shared UI primitives and tokens live in `packages/ui`; the separate Cloud repo
is not part of this npm workspace. See [workspace boundaries](docs/workspace-boundaries.md)
for transport, rendering, and regression-test details.

Cloud opens `https://kikimimi.dev/overview` in the default browser. The app keeps
local history and Settings visible when opening Cloud. Authentication, team
management, billing and hosted analytics belong to the browser. The Cloud link
is an external action, not an app tab. Legacy saved Cloud viewing preferences
fall back to local history on startup without opening the browser.

Settings manages this Mac's collection, Codex connection, and app preferences.
Under **Settings → Codex**, enter your team's workspace slug and choose
**Authorize in browser**. Approve the same team in the browser while keeping the
app open. The app polls the existing device authorization API for up to five
minutes; tokens stay in Rust and are stored separately from upload credentials.
The authorized workspace must match the requested slug and be a team.
It does not switch Codex until **Use your team's kikimimi router** is enabled.
**Restore previous provider** restores the previous model and provider even
when signed out of Cloud or when key revocation fails. Start a new Codex chat
after switching or restoring; the desktop app does not need to restart.
Existing chats may retain their previous connection. A saved configuration
is not a successful inference check.

**Settings → OpenCode connection** independently authorizes a team and enables
**Use team router** for OpenCode. Restart OpenCode after switching or restoring.
This edits the global `~/.config/opencode/opencode.json` (or `opencode.jsonc` when
present), respecting `XDG_CONFIG_HOME`. JSONC comments and unrelated settings are
preserved; the original is backed up once. Existing `kikimimi` provider definitions
and incompatible provider allow/deny lists are reported instead of overwritten.
Project configuration, environment overrides, and explicitly selected models can
override this global default. See the [OpenCode configuration precedence](https://opencode.ai/docs/config/).

OpenCode uses a separate Router key, referenced through `{file:...}` in its
[custom provider configuration](https://opencode.ai/docs/providers/). The key file
is created with mode 0600 beside the global config; it never crosses the UI bridge.
Team authorization uses a separate Keychain entry. Disabling restores the previous
model if it has not been manually changed, removes the unmodified provider entry,
and deletes the private key file. Remote revocation failure does not block local
restoration. This does not change Codex routing or collection settings.
Automated checks cover configuration round trips, private credentials, failure
cleanup and UI interaction; live OpenCode inference has not been verified.

Live verification on 2026-10-31: a new desktop chat created after enabling
routing used `model_provider=kikimimi` and `model=kikimimi-auto` without a
Codex restart. Its request reached the router's `/v1/responses` endpoint but
failed with HTTP 403 (`Provider-stored resources are not supported`). This
confirms configuration reload, not working Codex inference through the router.
After restoring the previous provider, another new chat used `openai` and
completed successfully. The Codex app and backend process IDs and start times
were unchanged across both checks; the restored configuration matched the
pre-test file byte for byte.

The 403 was fixed later the same day in the shared router: local tool argument
schemas containing `file_id` were being mistaken for stored-resource access.
After deployment, a new desktop chat using `kikimimi-auto` completed successfully
without restarting Codex. Actual stored-resource references remained blocked.
Optional S3 readers and legacy per-workspace viewing connections live under
**Settings → Data sources & S3**. Selecting a viewing area never changes
collection or uploads. Unconfigured S3 stays in the connection editor.
Dashboard navigation is a compact Activity sidebar. Preferences are reached
through the Settings tab; Cloud management opens in the browser.

Collection changes likewise use browser device authorization after reviewing
the workspace slug, personal/team type and sharing scope. Both the slug and type
must match approval before the destination is applied. Browser sign-in alone
does not change collection or Codex settings. Native commands only open fixed
HTTPS Cloud pages and the device approval URL; they cannot open arbitrary URLs.
First-run **View without collecting** opens saved local history without setting
up agents. A saved S3 read connection remains available across restarts.
In **Settings → Collection & storage**, choose Cloud and load workspaces from your account. Existing collection or router credentials are used to fetch current memberships; otherwise the browser sign-in flow opens. Select the workspace and review its sharing scope. Local collection always continues alongside Cloud uploads.

Select **Also upload saved local history matching the sharing scope above** to send existing local Parquet events. The upload preserves event IDs and uses the same repository filter and body masking as new events. The UI waits for completion and reports failures; retrying replays local history, with already accepted IDs deduplicated by Cloud. To upload history later, select the same workspace again and enable this option. This is a foreground operation, not a persistent background job; keep the app open until it finishes.

Use the upload settings to change this Mac’s upload destination after reviewing its sharing scope. The bundled CLI also supports upload configuration;
see [Team dashboard from S3](../docs/src/content/docs/s3-dashboard.md).

## Collection coverage

Claude Code CLI and Claude Desktop's Code mode use the configured hooks/OTel
and local transcripts. Cowork uses its separate local VM transcripts under
`~/Library/Application Support/Claude/local-agent-mode-sessions`; the collector
reads historical records and follows new lines without changing the VM's settings.
It reads only `.claude/projects` transcripts, not the duplicate audit stream.
Prompts, tool arguments and outputs remain excluded. Incomplete writes are retried;
restarts replay context and suppress event IDs already saved locally. EOF does not
mean that an active Cowork session has ended.

Sessions and session details show the observed application separately from the agent.
Sessions offers period and application filters; choose Last year for older imported
Cowork history. Each period includes at most the 500 most recent sessions.
The optional `client` column is additive; old local history can use observed
entrypoint/Desktop session metadata while the original logs remain available.
Unknown applications remain explicitly unknown, including other machines' S3
history without a recorded client. CLI/Desktop attribution does not use version
numbers. Missing subagent duration is shown as unknown, including partial totals.

`npm run test:collection` runs the real collector, local Parquet, DuckDB, viewer and
Chrome against isolated producer fixtures. It verifies all five application labels,
Cowork usage and live tools, partial writes, replay after restart, authentication,
unknown durations and exclusion of content. It never changes real agent settings,
starts a login service or uses account credentials.

## Signing and public distribution

### Local signed releases

On the configured release Mac, from the repository root:

```sh
npm run release:local --workspace desktop -- --install
# Continue after a network interruption without rebuilding:
npm run release:local --workspace desktop -- --resume
# Publish only after both architectures are staged and source changes committed:
npm run release:local --workspace desktop -- --resume --publish
```

This uses the Developer ID private key and signing settings in login Keychain
(`dev.kikimimi.desktop.release`, account `signing-material-v1`), plus the
`kikimimi-desktop-release` notarization profile. Credentials are never printed
or passed as command arguments. The updater private key is available only to the
signing subprocess. `--setup-notary` refreshes the notarization profile.
Unlike `install:local`, this produces distributable signed apps with updates enabled.

The command signs all bundled executables, notarizes and staples both the app and
DMG, checks Gatekeeper, verifies the updater archive after extraction, and stages
downloads in `desktop/release-artifacts/`. DuckDB is pinned and its license included.
The native architecture builds locally. Apple Silicon can also build Intel with
`--arch x86_64`; this requires Rosetta for executable and startup checks and
installs the matching Rust target. The macOS SDK supplies the target system libraries.
`--arch x86_64 --reuse-archive /absolute/path/kikimimi.app.tar.gz` can repackage
an already built Intel archive with its adjacent `.sig`; the existing updater
signature, version, architecture and app signature are verified first. Use an
archive built from the release's intended source revision.

Per-version work and notarization receipts are retained under
`desktop/.cache/signed-release/`. `--resume` is for the same source/package only;
omit it after changing source. Publication requires both architectures' six files
and creates an immutable versioned release before advancing the update manifest.
Secrets and build caches are ignored by Git. Initial setup on another Mac requires
secure provisioning of the certificate/private keys; GitHub cannot return stored
secret values through its API. The one-time encrypted migration used for this Mac
has been disabled and removed.

### Certificate requirements

Join the [Apple Developer Program](https://developer.apple.com/support/compare-memberships/)
(99 USD/year, or local currency where available). For direct DMG distribution,
create a **Developer ID Application** certificate, sign the app including its
embedded executables with hardened runtime, and submit it to Apple's notarization
service. App Store publication is not required. A Developer ID Installer
certificate is only needed if shipping a signed PKG installer.

Tauri supports signing via `APPLE_SIGNING_IDENTITY`, using a certificate/private
key already installed in the build Mac's keychain. For notarization, configure
`APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), and `APPLE_TEAM_ID` through
your local secret manager or CI secrets, then run `npm run build`. Never commit
these values. See [Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/)
and [Apple distribution](https://developer.apple.com/macos/distribution/).

## Before public release

Validation commands: `cargo test -p kikimimi --lib` from the repository root;
`npm run check`, `npm run test:ui` (Chrome required), `npm run test:viewer`
(after building the CLI; set `KIKIMIMI_BINARY` if needed), and `npm run test:signing`
(minisign required) from `desktop`. The UI test uses the real browser with a
mock native bridge; it is **not** a macOS installation E2E test. Native macOS
Rust code can be cross-type-checked with Clang, a macOS Rust target, and
`TAURI_CONFIG='{"bundle":{"externalBin":[]}}' cargo check --target aarch64-apple-darwin`
from `src-tauri` (set `CC_aarch64_apple_darwin=clang` on Linux). This checks Rust
types but does not link, launch, bundle, or test a real app update.

- Build and test on a clean Mac for each supported architecture without Homebrew
  or a CLI install. Check fresh setup, history backfill, dashboard queries,
  login/reboot, close/reopen, and disconnect while keeping unrelated hooks/data.
- Test adopting an existing CLI install and switching back to it, including paths
  containing spaces and apostrophes. Verify launchd gets the bundled DuckDB PATH.
- Verify Developer ID signatures for all executables, successful notarization,
  stapling, and Gatekeeper acceptance of the downloaded DMG/app.
- Exercise a real signed N → N+1 update on a Mac: manual install, automatic
  install, auto-update disabled across restart, offline/404, invalid signature,
  unwritable Applications directory, interrupted download, and crash immediately
  before/after bundle replacement. Confirm collection resumes on the new binary,
  existing data/settings remain intact, and a disconnected collector stays off.
- Include DuckDB's license/notices in the
  distribution. Pin/audit the chosen DuckDB build before public release.

Tray usage summaries, agent-specific collection toggles, and
Linux/Windows desktop packaging are follow-up work. The setup UI currently uses
the collector's existing combined Claude/Codex collection behavior.

## Updates (included from the first signed release)

The Updates panel supports manual checks and Update and restart. Automatic
download/install/restart is enabled by default and can be disabled; the preference
is saved atomically in the desktop app config directory's `updates.json`. The app
checks 60 seconds after launch and every 24 hours while running. Closing its window
keeps it in the menu bar; explicitly quitting stops update checks until next launch.
Changing preferences and collection settings is serialized with installation.

The updater uses HTTPS and Tauri's mandatory signature verification. The trusted
public key is compiled from `KIKIMIMI_UPDATER_PUBLIC_KEY`; builds without a key and
debug builds show updates as unavailable and never check/download. Do not replace
this key between releases without a separately planned key migration. The key is
independent of Apple's Developer ID certificate.

Verified archives are unpacked beside the installed app. The app identifier and
embedded version must match the expected update; all three binaries must be regular
executable files supporting the running architecture. After `codesign` verification,
the bundle is swapped with `/Applications/kikimimi.app` using macOS `RENAME_SWAP`. A failed
swap leaves the old bundle intact; the collector continues during download and
staging. After replacement only the service owned by this app bundle is restarted.
An `update-pending` marker is written before installation and retained until the
collector restart succeeds. On the next app launch recovery runs even if automatic
updates have been disabled. Then the UI relaunches into the new version. Updates
require write permission to `/Applications`; they do not request elevation.

The bundled `kikimimi self-update` explicitly directs users to the desktop Updates
panel before consulting any CLI install receipt. Standalone CLI script/Homebrew
update behavior is unchanged; app bundles are never partially updated by it.

### Release setup

Create a persistent Tauri signing key with `npx tauri signer generate -w <secure-path>`.
Store the private key/password in your secret manager and GitHub secrets
`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Store the contents
of the generated `.pub` file in repository variable `KIKIMIMI_UPDATER_PUBLIC_KEY`.
No production signing keys are generated or committed by this implementation.
Before building, CI runs `node desktop/scripts/release-config.mjs` to generate
`src-tauri/tauri.release.generated.conf.json` from the release template and
`KIKIMIMI_UPDATER_PUBLIC_KEY`. This provides the required `plugins.updater.pubkey`
to the Tauri bundler, using the same key embedded into the app. Only the public key
is written; the generated file is ignored by Git. Local signed builds must generate
this file first and pass `--config src-tauri/tauri.release.generated.conf.json`.

The `Desktop signed release` workflow also requires GitHub secrets
`APPLE_CERTIFICATE` (base64 P12), `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_ID`, and `APPLE_PASSWORD` (an app-specific password, not your Apple Account
login password). `APPLE_SIGNING_IDENTITY` and `APPLE_TEAM_ID` are derived from the
P12 certificate on the runner; do not register them separately. The P12 must contain
exactly one current Developer ID Application signing certificate with its private
key. The derivation reads only public certificates through OpenSSL (including legacy
Keychain P12 encryption), rejects ambiguous/expired identities, and passes the
values to later steps via `GITHUB_ENV` without logging credentials. Certificate
trust and private-key usability are checked by the subsequent signing/verification
steps, not by metadata extraction alone.
It builds both native Mac architectures, signs/notarizes,
checks Gatekeeper/stapling, and verifies every update archive with minisign and
the same public key embedded in the app. Missing settings fail the build early.

Run the workflow with `publish=false` first to inspect its signed artifacts.
`publish=true` publishes immutable `desktop-vX.Y.Z` assets and then advances only
`desktop-updates/latest.json`, after both architectures succeed. This separate
channel avoids collisions with CLI releases. It refuses repeated/older versions
and refuses overwriting existing versioned releases. If publication fails after
creating the immutable release, inspect its assets and repair the channel explicitly;
rerunning will not overwrite those binaries. Bump root/desktop Cargo versions and
the Tauri config version together before a new release.

As of 2026-11-01, both architectures of 0.8.0 have built and passed collection
tests. The local release process produced Developer ID signed, Apple-notarized
and stapled apps and DMGs for both architectures, including fresh updater
signatures. The actual DMG contents and extracted updater payloads passed
signature and stapled-ticket verification. The Intel executable was reused from
GitHub Actions run `41275593026`; its packages were rebuilt locally to include
the ticket and licenses. Release downloads use `desktop-v0.8.0` and the update
manifest uses `desktop-updates`. A real signed N → N+1 automatic-update E2E
remains unverified.

The local 0.8.0 Apple Silicon package failed at startup because the local build
omitted the generated updater plugin configuration used by CI. Version 0.8.1
passes that configuration to Tauri and checks the extracted public app remains
running through startup before staging. The startup check reproduces the failure
in the published 0.8.0 app. Signature/notarization checks alone do not test startup.

Version 0.8.2 also stops reading Keychain credentials during periodic desktop
status checks, collection destination display and local viewer initialization.
These operations need configuration metadata only. Authenticated operations still
use Keychain normally. A regression test exercises repeated status/info commands
and reader initialization with four stored secrets, asserting zero Keychain reads.

## MinIO integration check

`npm run test:minio` exercises the production Rust S3 sink, real AWS CLI v2,
MinIO, a separate viewer state directory, and headless Chrome. It checks two
machines, duplicate events, read-only access, analysis queries, revoked access,
additions/deletions, and retention of the last snapshot during an outage.
The collector's hook ingestion and the signed Tauri shell are outside this test.

Prerequisites: Docker running, Chrome, DuckDB on PATH, and desktop npm dependencies.
Build the current binaries from the repository root, then run both viewer modes:

```sh
cargo build -p kikimimi --bin kikimimi
cargo build -p kikimimi-sink --example s3_smoke
cd desktop
npm run test:minio
S3_READER_STANDALONE=1 npm run test:minio
```

The test uses `minio/minio:RELEASE.2025-07-23T15-54-02Z`,
`minio/mc:RELEASE.2025-08-13T08-35-41Z`, and `amazon/aws-cli:2.34.7`.
It creates a unique Docker network with no published ports, temporary synthetic
credentials and data, and removes them at completion. The `aws` wrapper runs the
actual CLI in Docker; it does not simulate S3 responses. Existing AWS credentials
are excluded. Set `S3_SCREENSHOT` to an absolute path to keep a dashboard screenshot.
AWS IAM/SSO/KMS behavior still requires separate AWS testing.

### Collection and Cloud sync

Local activity is stored in DuckDB. Signing in enables Cloud sync; one available
workspace is selected automatically, and multiple workspaces require one choice.
Sync includes all repositories, retaining repository metadata for display filters.
The local view includes your own devices; team activity in Cloud is restricted to
owners and administrators. Device/repository filters affect viewing, not collection.
Turning sync off preserves authentication and local history. S3 export and viewing
are separate controls. Device authorization credentials stay in native code.

### OpenAI application attribution

Codex CLI, Codex Desktop, and ChatGPT Desktop Cowork use the local Codex
rollout collector. The observed `originator` identifies the surface:
`codex_exec` / `codex_cli_rs` / `codex-tui` → Codex CLI,
`Codex Desktop` → Codex Desktop, and `codex_work_desktop` → ChatGPT Desktop · Cowork.
The engine remains `agent=codex`; Cowork uses `client=chatgpt_desktop_cowork`.
Model names come from turn context and measured usage comes from token-count
records. Ordinary ChatGPT Chat has not been verified and is not marked supported.
No collection plugin or hook installation is required for these three local
surfaces. Cloud-only Work is outside the local rollout collector's coverage.
Claude's chat plugin is a separate integration; its capabilities must not be
assumed to apply to ordinary ChatGPT Chat.

Older local session labels are refined from this machine's retained metadata;
this display correction does not rewrite already-synced Cloud observations.
Live local dashboard requests use the collector's DuckDB service. The separate
viewer serves S3 and opens the local ledger when collection is stopped.

### Display language

Choose **Settings → General → Language → 日本語** to use the desktop app and
embedded dashboard in Japanese. English is also available. Changes apply
immediately and are saved on this Mac; the app and tray menus follow this setting.
The standalone local dashboard and Kikimimi Cloud each offer **Settings → Language**
and save the preference in that browser. This preference does not change collection,
sharing, or model settings.

## Experimental router connections

Codex and OpenCode router connections are disabled by default in the desktop app.
Their settings panels are omitted, status requests are skipped, and native router
commands reject calls. To enable them for an experimental build, set
`VITE_ENABLE_ROUTER=true` in the environment for the entire desktop build (both
Vite and Rust), for example `VITE_ENABLE_ROUTER=true npm run build --workspace desktop`.
Vite-only `.env` files do not enable the native commands.
This flag does not change the standalone CLI or Cloud router APIs, nor does it
restore provider settings previously changed by a router-enabled build.
Run UI checks against a build with the matching flag value.
