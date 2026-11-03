# Cost intelligence MVP

`/cost` uses the existing filtered `/web/q/models` response. It does not modify billing, run models, or transmit prompts. Evaluation plans are local text downloads. Billing mode is explicitly selected and never inferred from token logs.

## Official price refresh

Run `npm run pricing:refresh --workspace web`, review the resulting `pricing.json` diff, and rebuild both UIs with `npm run build:ui`. The updater fetches official Anthropic and OpenAI Markdown tables, validates the supported rows, and atomically replaces the bundled snapshot only when both sources parse. Failed downloads leave the previous snapshot intact. It does not run automatically or claim live pricing. A snapshot older than 30 days disables API recommendations. New model IDs must be explicitly reviewed and added to the allowlist.

## Scope and limitations

- Standard, first-party, global text-token rates in USD. No discounts, regional premiums, Fast/Priority/Batch, tools, tax, FX, or invoice reconciliation.
- Context tiers and cache-write TTL uncertainty yield ranges. Savings use current minimum minus candidate maximum for the lower bound. Alternatives from the same provider only; one alternative per model/effort group avoids double counting.
- Claude input excludes cached reads/writes. Codex input includes cached input; subtract cached amounts before applying uncached rates. Reasoning is already part of output.
- Missing, invalid, or unsupported quantities are excluded rather than guessed. Aggregated logs may have incomplete capture; totals cover captured quantities, not a complete invoice or monthly forecast.
- Tokenization, cache behavior, quality, capabilities, and retries can change. Candidates are price-based hypotheses, not suitability recommendations. Users evaluate representative work manually because prompt bodies are not collected.
- Billing comparison uses user-entered per-seat monthly/annual prices, seats, monthly extras, one-time transition costs and a 1–120 month horizon. USD and JPY are supported; JPY requires an explicit manual exchange rate. Currency changes clear prices instead of silently relabeling them. Annual bills are amortized, not a cash-flow forecast.
- API migration uses the current model mix, scaled by 30 / observed days and a user-entered usage percentage, plus monthly extras and migration/remaining-contract costs. It does not multiply token totals by seat count.
- Three-way ranking requires fresh rates, valid amounts and confirmation that contracts and captured usage cover the same people/work and a representative period. Partial capture cannot be inferred from request coverage alone. Missing usage, unknown models and stale rates prevent an API cheapest claim. With incomplete pricing, the API upper bound is unknown; a known subscription can still be ranked cheapest when it strictly beats the priced API lower bound. API savings and break-even remain unavailable for incomplete pricing. Subscription-only totals remain comparable.
- Ranking compares total costs over the horizon, with strict separation of uncertain ranges; exact ties are retained and overlapping ranges abstain. Break-even uses recurring costs only. Capability/limit suitability and cancellation terms require user verification.
- Inputs remain in this view (no persistence or sharing); a downloadable comparison preserves the assumptions, totals, coverage and official sources. Period changes clear the scope confirmation. Leaving the view/account/team/filter scope clears the scenario. No contract is changed and no paid API call is executed.

Validation: `npm run test:cost --workspace web` and `npm run build:ui`.

## Request-level coverage and payment provenance

Models responses append six optional columns without reordering existing fields: `reported_cost_requests`, `complete_usage_requests`, and complete-only input/output/cache-read/cache-write sums. Completeness is evaluated before aggregation; SQL SUM of partially missing columns must never be used as proof of completeness. Missing/negative counters and invalid OpenAI cache subsets are excluded. Source preference is identical to the existing usage query. Old servers without coverage are explicitly unpriceable.

Overview, Models and Cost intelligence show API equivalents, agent-reported costs and actual payments separately. Actual payments are **not connected**: there is no invoice/payment feed and no amount is fabricated. Subscription contract comparison remains a user-entered scenario, not a confirmed payment. Coverage counts only captured API requests; it cannot measure activity that was never collected.

## Collection → Cloud → comparison E2E

`node web/scripts/check-cost-sync-e2e.mjs [cloud-checkout]` exercises the real
resident collector's authenticated OTLP intake, DuckDB storage/outbox, device
authorization, Cloud synchronization, Postgres queries and both built dashboards.
It creates a disposable PostgreSQL 18 Docker container, team, device and isolated
provider profiles; it does not write test records to production or call paid models.

Build this checkout's `kikimimi` debug binary and the Cloud checkout's
`kikimimi-cloud` debug binary, with both `web/dist` directories built first.
The default Cloud checkout is `../kikimimi-cloud`. Docker (`postgres:18-alpine`),
Chrome (override with `CHROME_BIN`) and desktop's `puppeteer-core` are required.
The script checks exact token/cost/coverage parity, all three price decisions,
provider replay, collector restart and missing-token lower-bound behavior.
It leaves logs, screenshots and `results.json` in a printed temporary directory;
its services and disposable database container are stopped automatically.

The test caught a collector startup panic when Cloud sync was already configured:
`reqwest::blocking::Client` was created directly inside the async agent runtime.
Startup now uses `block_in_place`, matching the existing sync flush/reload path.
The E2E deliberately starts and restarts with Cloud credentials already present
so this regression cannot be hidden by logging in only after startup.
