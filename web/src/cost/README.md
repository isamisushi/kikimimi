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
- Subscription comparisons use explicitly entered monthly contract totals. No downgrade feasibility is inferred from a single usage-window snapshot. Extra usage and API credits must be checked with the provider; API equivalent spend is never presented as subscription savings.

Validation: `npm run test:cost --workspace web` and `npm run build:ui`.

## Request-level coverage and payment provenance

Models responses append six optional columns without reordering existing fields: `reported_cost_requests`, `complete_usage_requests`, and complete-only input/output/cache-read/cache-write sums. Completeness is evaluated before aggregation; SQL SUM of partially missing columns must never be used as proof of completeness. Missing/negative counters and invalid OpenAI cache subsets are excluded. Source preference is identical to the existing usage query. Old servers without coverage are explicitly unpriceable.

Overview, Models and Cost intelligence show API equivalents, agent-reported costs and actual payments separately. Actual payments are **not connected**: there is no invoice/payment feed and no amount is fabricated. Subscription contract comparison remains a user-entered scenario, not a confirmed payment. Coverage counts only captured API requests; it cannot measure activity that was never collected.
