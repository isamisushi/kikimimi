# kikimimi desktop (macOS preview)

Tauri desktop app with a single-window dashboard, scoped settings, first-run setup,
and a menu-bar entry, sharing the existing Rust collector and React dashboard. No account or Apple membership is needed for local development. This
is a development preview, not yet a signed public release.

The `Desktop preview` GitHub workflow builds an ad-hoc-signed app on a Mac runner
and saves a ZIP as a CI artifact. It does not publish a release or notarize the app.

## Build on a Mac

Install Xcode Command Line Tools, Rust, Node.js and DuckDB on the **build machine**.
Run `npm ci` in this directory, then `npm run dev` or `npm run build`.
Use Rust 1.93.1 or newer for the test dependencies. Development startup waits
for preparation and uses a debug collector build; release builds remain optimized.
Set `DUCKDB_BINARY` to an absolute executable path if DuckDB is not on PATH.
The preparation script builds the existing web UI and CLI, checks DuckDB's CPU
architecture, and bundles both executables. End users do not install DuckDB.
DuckDB must be a standalone executable linked only to system libraries; the
script rejects Homebrew/private dylib dependencies that would break on a clean Mac.
Build separately on Apple Silicon and Intel; cross/universal builds are not yet supported.

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

Native commands accept no arbitrary executable, arguments or URLs. Only the
local shell webview can invoke management commands (label and origin checked).
The dashboard is a separate, unprivileged child webview in the same window,
restricted to its authenticated loopback origin. Tauri's multi-webview support
requires the `unstable` feature; verify native resize and window lifecycle on macOS.

The toolbar offers **Dashboard** and **App Settings**. App Settings applies to
this Mac: collection, updates and actual upload destinations read from
`/web/storage`. Read failures show unknown, not disconnected. **Workspace Settings**
is an action in the workspace menu; it shows the selected workspace and links to
Cloud membership management under the existing server permissions.
**Viewing Connection** is separate and explicitly local to this Mac. It selects
This Mac / S3 / Cloud and stores a per-workspace S3 bucket, AWS profile and endpoint.
It neither changes upload destinations nor distributes settings to other members.
The source label and both settings pages link to this connection screen.

Cloud opens `https://kikimimi.dev/overview` inside an unprivileged child webview.
GitHub login uses the hosted UI. The native toolbar always shows the workspace
selector: Personal, authenticated Cloud teams, and Team · S3. Personal remembers
This Mac / S3 / Cloud; Cloud teams use Cloud and Team · S3 has its own reader
connection. Team workspaces cannot use This Mac. These are per-Mac viewing
preferences, not a server-side owner policy; S3 IAM and Cloud memberships are
separate identities. Cloud cookies persist across restarts. Native requests to
fixed Cloud endpoints read memberships and select the active organization without
exposing cookies to the shell. The hosted selector is hidden only in the desktop
webview, so there is one selector for all sources. Workspace and connection state
are committed after a successful switch. Unconfigured S3 stays in Viewing Connection. Only HTTPS Cloud
and GitHub top-level navigation is permitted; pop-ups are blocked. The local
shell alone can change local source settings, using the app-owned loopback server.
The shared dashboard's **Storage & sharing** page shows local destinations and
setup instructions. S3 **read connections** can be configured directly in that page.
First-run **View without collecting** opens it without setting up agents. A saved
S3 read connection also makes subsequent launches open the dashboard.
Use **Collect into this workspace** to change this Mac’s upload destination after reviewing its sharing scope. The bundled CLI also supports upload configuration;
see [Team dashboard from S3](../docs/src/content/docs/s3-dashboard.md).

## Signing and public distribution

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

The workflow and real signed upgrade E2E still need to run on macOS before public
distribution. Creating this workflow does not publish a release or activate the
update endpoint by itself.

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

### Collection destination versus viewing workspace

The green Workspace marker means this Mac's collector has applied that destination;
viewing another workspace does not redirect collection. A banner links to
App Settings → Collection, where the destination and sharing scope are reviewed.
Switching replaces both Cloud/S3 uploads atomically, retaining local files and reader
preferences. Team Cloud requires repository patterns or explicit all-repository consent.
S3 sends all recorded fields/repositories. Stopped collection stays stopped.
The native bridge provisions Cloud device authorization without exposing tokens to JS
or process arguments. Configuration changes after review reject stale confirmation.
The collector reports its applied destination; saved-but-unconfirmed changes remain pending.
