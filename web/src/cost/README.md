# Cost estimates

## Official price refresh

Run `npm run pricing:refresh --workspace web`, review the resulting `pricing.json` diff, and rebuild both UIs with `npm run build:ui`. The updater fetches official Anthropic and OpenAI Markdown tables, validates the supported rows, and atomically replaces the bundled snapshot only when both sources parse. Failed downloads leave the previous snapshot intact. It does not run automatically or claim live pricing. A snapshot older than 30 days hides API estimates. New model IDs must be explicitly reviewed and added to the allowlist.

## Scope and limitations

- Standard, first-party, global text-token rates in USD. No discounts, regional premiums, Fast/Priority/Batch, tools, tax, FX, or invoice reconciliation.
- Context tiers and cache-write TTL uncertainty yield ranges.
- Claude input excludes cached reads/writes. Codex input includes cached input; subtract cached amounts before applying uncached rates. Reasoning is already part of output.
- Missing, invalid, or unsupported quantities are excluded rather than guessed. Aggregated logs may have incomplete capture; totals cover captured quantities, not a complete invoice or monthly forecast.
Validation: `npm run test:cost --workspace web` and `npm run build:ui`.

## Request-level coverage and payment provenance

Models responses append six optional columns without reordering existing fields: `reported_cost_requests`, `complete_usage_requests`, and complete-only input/output/cache-read/cache-write sums. Completeness is evaluated before aggregation; SQL SUM of partially missing columns must never be used as proof of completeness. Missing/negative counters and invalid OpenAI cache subsets are excluded. Source preference is identical to the existing usage query. Old servers without coverage are explicitly unpriceable.

Overview and Models show API equivalents, agent-reported costs and actual payments separately. Actual payments are **not connected**: there is no invoice/payment feed and no amount is fabricated. Coverage counts only captured API requests; it cannot measure activity that was never collected.

## Collection → Cloud → dashboard E2E

`node web/scripts/check-cost-sync-e2e.mjs [cloud-checkout]` exercises the real
resident collector's authenticated OTLP intake, DuckDB storage/outbox, device
authorization, Cloud synchronization, Postgres queries and both built dashboards.
It creates a disposable PostgreSQL 18 Docker container, team, device and isolated
provider profiles; it does not write test records to production or call paid models.

Build this checkout's `kikimimi` debug binary and the Cloud checkout's
`kikimimi-cloud` debug binary, with both `web/dist` directories built first.
The default Cloud checkout is `../kikimimi-cloud`. Docker (`postgres:18-alpine`),
Chrome (override with `CHROME_BIN`) and desktop's `puppeteer-core` are required.
The script checks exact token/cost/coverage parity,
provider replay, collector restart and missing cost coverage.
It leaves logs, screenshots and `results.json` in a printed temporary directory;
its services and disposable database container are stopped automatically.

The test caught a collector startup panic when Cloud sync was already configured:
`reqwest::blocking::Client` was created directly inside the async agent runtime.
Startup now uses `block_in_place`, matching the existing sync flush/reload path.
The E2E deliberately starts and restarts with Cloud credentials already present
so this regression cannot be hidden by logging in only after startup.
