# Desktop boundaries and rendering

The app has three independent operations, not one workspace switch.

| Boundary | Owns | May change |
| --- | --- | --- |
| Browsing | destination, history reader, navigation revision | visible page and per-request reader context |
| Connections | validated local/S3 preferences | saved local connections |
| CollectionDestination | confirmed change against an observed destination | collector/upload destination |

`src/workspace-domain.ts` defines separate ports. Browsing has no configuration
or collection port. `browsing-v1` stores the last view independently of
`workspaces-v1` connection preferences. Visiting This Mac never overwrites a
saved S3 connection. Collection changes require consent, an expected
current destination, and explicit repository scope for teams.

## Local webview and external browser

The bundled `main` webview renders the React/TypeScript desktop shell and the
history components from `web/`. History stays mounted while switching settings.
Cloud links launch the default browser without changing local view or reader.
React owns local visibility; there is no native hide/show command.
There is no remote child webview, injected presentation script, Cloud cookie
access, or email-return protocol. The native multi-webview feature is disabled. Legacy Cloud reading preferences migrate to
local history on startup.

Connection authorization uses the existing device-code API. The user specifies
a workspace slug and approves it in the browser; Rust polls with a five-minute
deadline and checks the returned slug and workspace type before applying it.
Collection still requires explicit consent and team repository scope. Tokens
stay in native memory and pass to the bundled CLI via stdin.

## Native data boundary

`history_request` validates the main webview's identity and origin, read source,
method, and an allowlist of dashboard endpoints. Rust starts/reuses the app-owned
loopback viewer and attaches its private credential. The URL/token never enters
JavaScript. Redirects and HTTP proxies are disabled. Each request carries its own
local/S3 read context; there is no global read-cookie race. This command cannot
configure connections, collect activity, or access arbitrary URLs.

The viewer remains a separate process so CLI/browser history and desktop reuse
the existing query service. It does not collect/upload data and exits with its
owner. `set_source` validates/persists connection settings only. Browser sign-in does not configure local connections.

React renders DOM and controls; the controller owns asynchronous effects and
subscriptions. Rust remains authoritative for persisted configuration, collection,
credentials, sidecar lifecycle, and updates. Native command names are checked by
TypeScript; Rust still validates inputs at runtime. CSP permits bundled scripts
only. Inline styles are permitted for React chart dimensions, not inline scripts.

## Shared UI

`packages/ui` owns design tokens and source-owned shadcn Button/Input components,
plus native select, card, and accessible modal primitives. Both entry points use
one npm workspace lockfile. The package has no Tauri imports. Cloud can consume
it through a separately versioned package workflow later; it does not currently
consume this local workspace package. No cross-repository relative imports exist.

## Verification

From the repository root:

- `npm run build:ui`: typecheck/build both frontends.
- `npm run check --workspace desktop`: domain and release contract tests.
- `npm run test:ui --workspace desktop`: built React UI with mock native bridge;
  retained history DOM, stale Cloud response, settings/failed writes, consent,
  team scope, S3/local separation, 760/1200px geometry, animation-frame checks.
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml`: native origin/path
  restrictions, device authorization, update safety.
- `npm run test:viewer --workspace desktop`: standalone HTTP viewer and UI.
- Real macOS app: verify external browser launch, device approval, local history,
  Codex connection/restore, and collection consent.

Mock browser tests do not verify actual browser sessions or live device approval.

## External Cloud verification (2026-11-01)

A separately identified macOS test app with a temporary `KIKIMIMI_DIR` opened
Cloud in the default Chrome profile while retaining its local Settings page.
After Google sign-in and user approval of the device for `isami`, the native
app displayed the authorized team. A fresh bundled CLI process confirmed the
persisted authorization with routing disabled; Cloud/S3 uploads remained
unconfigured and the device token was absent from the JSON configuration.
The live check did not enable routing or upload activity.

UI integration, real collector → Parquet → DuckDB → browser, S3 reader E2E
(with a read-only AWS fixture), 24 desktop checks and 17 native tests passed.
