# Desktop boundaries and rendering

The app has three independent operations, not one workspace switch.

| Boundary | Owns | May change |
| --- | --- | --- |
| Browsing | destination, history reader, navigation revision | visible page and per-request reader context |
| Connections | validated Cloud workspace/S3 preferences | saved connections; explicit Cloud workspace selection |
| CollectionDestination | confirmed change against an observed destination | collector/upload destination |

`src/workspace-domain.ts` defines separate ports. Browsing has no configuration
or collection port. `browsing-v1` stores the last view independently of
`workspaces-v1` connection preferences. Visiting This Mac never overwrites a
saved Cloud or S3 connection. Collection changes require consent, an expected
current destination, and explicit repository scope for teams.

## Two webviews with distinct trust boundaries

The bundled `main` webview renders the React/TypeScript desktop shell and imports
the same history React components used by `web/`. History stays mounted while
switching tabs. Only a change of reader remounts the history query context.
Routing is in memory; local navigation does not load documents. Scroll positions
are preserved independently. There is no local dashboard child webview.

The separate remote `cloud` webview owns its authentication session and has no
native capabilities. Remote content is never loaded in the privileged main
webview. Only HTTPS kikimimi.dev and GitHub authentication navigation is allowed.
The Cloud repository is separate and unchanged by this migration. No desktop
CSS or navigation mutation script is injected into its UI.

Cloud is hidden before navigation. A readiness generation rejects stale signals;
loaders/fonts/layout must settle before revealing it. A late completion after
switching to Settings cannot reveal Cloud. Timeouts remain hidden and allow retry.
This readiness adapter depends on Cloud's loading selectors: update its tests
when Cloud changes its rendering conventions.

## Native data boundary

`history_request` validates the main webview's identity and origin, read source,
method, and an allowlist of dashboard endpoints. Rust starts/reuses the app-owned
loopback viewer and attaches its private credential. The URL/token never enters
JavaScript. Redirects and HTTP proxies are disabled. Each request carries its own
local/S3 read context; there is no global read-cookie race. This command cannot
configure connections, collect activity, or access arbitrary URLs.

The viewer remains a separate process so CLI/browser history and desktop reuse
the existing query service. It does not collect/upload data and exits with its
owner. `set_source` validates/persists connection settings only. Only explicit
Cloud workspace selection invokes `select_cloud_workspace`.

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
- `npm run test:cloud-readiness --workspace desktop`: loading and layout gate.
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml`: native origin/path
  restrictions, readiness generations, timeout/retry, update safety.
- `npm run test:viewer --workspace desktop`: standalone HTTP viewer and UI.
- Real macOS app: cold start and repeated This Mac/Cloud/Settings transitions,
  query data, usage account controls, settings, and window resizing.

Mock browser tests do not verify WKWebView rendering. Native UI sampling cannot
prove absence of every sub-frame artifact; combine actual visual inspection with
state-transition and loading-gate regression tests.
