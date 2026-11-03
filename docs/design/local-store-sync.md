# Local store and synchronization

Status: runtime implementation and integration tests complete; the installed Mac
collector and UI now read the persistent DuckDB. After restart, the original
conversation resolves Haiku (2 input / 16 output) and Sonnet (2 / 15) measured
usage. Production synchronization is authenticated to the isami team: uploads,
self downloads with repository metadata, and the admin analytics query succeeded.
The historical backlog completed: 35,281 acknowledged records, 35,281 downloaded
records, zero pending uploads and no sync error at verification. The cloud-returned
Haiku response msg_011CgcECFf7WwdZjQwvjmXCr matches claude-haiku-5-5,
2 input / 16 output, 99,900 cache read / 13,143 cache write; the Sonnet response
msg_011CgcEF2drbp8ZiTf5dnfr5 matches 2 input / 15 output. Both retain hook usage
provenance, distinct from estimates. The Cloud API was deployed
to kikimimi.dev and its unauthenticated sync endpoint returns 401. This document
supersedes the fan-out and best-effort cloud queue in architecture.md.

## Cloud sync controls

Local persistence is always enabled. Cloud sign-in enables synchronization by
default (`cloud_sync_paused` defaults to false); an explicit pause survives
restart and retains the device credential. Resume queues only locally stored
records not already acknowledged or queued for that destination. S3 is an
independent one-way export and is preserved when Cloud is connected or paused.
The desktop displays the configured account and collector-reported pending/error
state instead of calling a configured destination proof of successful sync.

## Ownership and access

- The local application shows only the signed-in person's data: this device and,
  when enabled, that same person's other devices.
- Cloud team analytics require a current team owner/admin membership.
- Self synchronization is a separate permission from team analytics. An ordinary
  team member can synchronize their own records, never another member's records.
- A client cannot select an account to download. The server derives it from the
  authenticated device. Organization membership and device revocation are checked
  on every request. Even administrators use the self-only sync endpoint.
- Local downloaded records and cursors are partitioned by endpoint and account
  (all of that person’s devices across workspaces). Upload queues also bind the
  workspace and device. Repository filters are not applied. Signing out detaches that partition from all queries. Switching
  accounts must never reuse a cursor, queued upload or downloaded partition.
- Repository metadata (`Event.repo`) remains in local records and synchronized
  payloads for subsequent filtering and aggregation. It is not an intake or
  upload authorization boundary; records without a repository are retained too.

## Data flow

All producers (CLI hooks, OTel, Code transcripts, Cowork transcripts, Claude
plugin receiver and Claude history cache) feed the same ingestion operation.
The collector is the sole owner of a persistent local DuckDB connection.
UI and CLI use its authenticated API; neither opens the writable database file.
Parquet is an export/analysis format, not another independent writable authority.

One transaction stores an immutable source observation and its authorized,
content-redacted upload payload. The sharing decision is captured at ingestion;
it is not inferred later from whatever account happens to be signed in.
Delivery workers read only committed outbox rows. After a matching server
acknowledgment, receipt insertion and queue removal commit together. A server
commit followed by a lost acknowledgment causes safe replay, never extra usage.

Do not acknowledge an upstream upload or delete a hook spool entry before the
local transaction commits. Backpressure or storage errors must remain visible;
queue size limits must reject new intake, not silently trim unsent records.

## OpenTelemetry is retained

The storage refactor does not remove OTLP ingestion. The current implementation
accepts OTLP/HTTP JSON and protobuf and normalizes Claude Code logs. It does not
yet implement a general GenAI spans pipeline; the metrics normalizer is currently
a no-op. Do not describe this as full GenAI semantic-convention compliance.

The target keeps OTLP as a first-class ingress alongside hooks and transcript
adapters. Version-pin the GenAI semantic conventions and maintain an explicit
mapping table with provenance, source schema version and measured/estimated
status. Keep observed trace/span identifiers when supplied; never manufacture a
claim that content-based conversation matching is a distributed trace identity.

Token accounting needs an explicit normalization rule: upstream input counts may
include cache usage, while Claude's input_tokens may exclude cache reads/writes.
Preserve original counters and avoid both dropping and double-counting cache
usage. Estimated visible text is not a measured token metric. Unknown remains
unknown. Prompt/response content stays opt-in and is not needed to preserve OTel
interoperability.

OTLP is the telemetry transport, not the account-scoped replication protocol.
Synchronization additionally needs acknowledgments, cursors, ownership and
correction/deletion semantics. These belong in the sync layer, not fabricated
OTel measurements.

References checked 2026-11-01:
- https://opentelemetry.io/docs/specs/semconv/
- https://github.com/open-telemetry/semantic-conventions-genai

## Logical relations

- observations: source scope, stable event ID, versioned event payload.
- conversation links: observed source/API identity and local chat/message
  identity, matching method and rejection evidence. Content matches remain
  explicitly heuristic. Rejection must override earlier positive evidence.
- resolved_events: shared projection used by all query consumers. Link records
  are excluded from usage totals; estimates never become measured usage.
- outbox / receipts: destination scope and stable event identity.
- downloaded observations / sync_cursors / tombstones: remote account scope,
  server position and deletion state. Never automatically re-upload downloads.

An event ID replay must not overwrite different source evidence. Corrections
need an explicit new revision/event, not silent last-write-wins. Source-local
IDs must be namespaced to prevent collisions across users and devices.

## Cloud protocol

Upload acknowledgments name accepted or already-present IDs. Validation and
ownership failures acknowledge nothing. Team analytics and self sync consume the
same stored observations and correction records.

Downloads use a monotonically increasing, account-scoped server cursor and bounded pages. Event
timestamps are not cursors. Commit-order safety is required: a database sequence
allocated before commit alone can skip concurrent late commits. Serialize change
position assignment per account inside the ingestion transaction, or use an
actual committed replication position. Inserts, revisions and deletions all enter
the change stream transactionally.

Apply the entire returned page and its next cursor in one local transaction.
A crash repeats a page safely. Deletions have durable tombstones. If a cursor is
older than retained changes, return an explicit reset-required result and rebuild
that remote partition from an authenticated snapshot; never silently skip data.

Retries use bounded exponential backoff with jitter and Retry-After. A 401 pauses
sync for reauthentication; a 403 pauses the affected scope. A new credential for
the same authorized scope can resume. A different scope cannot inherit its queue.

The Claude plugin's ingest.kikimimi.dev endpoint currently tunnels to the Mac.
It is a producer ingress, not the Cloud sync service, and cannot guarantee
collection while the Mac is offline. Server-side plugin ingestion can be added
later using the same event/sync contract and a restricted ingest credential.

## Cutover and acceptance

No backwards-compatible runtime schema or old API path is required. Keep a
read-only backup of existing Parquet/configuration before cutover; do not erase
measured history. Historical import runs once, transactionally, before the collector starts.
Existing Parquet files are retained untouched; they are not an ongoing second
ingestion path. Uploading existing history requires explicit user selection and
queues it from DuckDB through the authenticated collector API.

Before replacing the installed collector, verify:

1. Commit failure preserves upstream input and produces no acknowledged upload.
2. Restart and lost acknowledgment cannot lose or double-count an observation.
3. Account/workspace switching cannot expose or send another scope's records.
4. General members can self-sync but cannot query team analytics or sync others.
5. Admin self-sync also remains self-only; demotion/revocation takes effect now.
6. Late history links, ambiguity rejection and measured/estimated usage resolve
   identically through the local UI, CLI and cloud analytics.
7. Deletes, retries, stale cursors and concurrent server commits cannot resurrect
   or skip records.
8. Each actual supported Claude path produces a measured response in the local
   application and syncs through the same durable pipeline.

## Implementation status

- Added an embedded DuckDB store with atomic observation/outbox writes, durable
  receipts, scoped remote pages/cursors and tombstones; four store tests pass.
- Moved the local history-link query projection into a shared storage module and
  applied it to named CLI queries as well as the web dashboard.
- Collector ownership, durable ingestion acknowledgment, UI/CLI connection,
  cloud self-sync API, account-scoped download projection and installed-app
  cutover are not yet implemented. The new store is not the running authority.

## Implemented and verified (2026-11-01)

- `crates/sink/src/store.rs`: one persistent DuckDB, immutable observations,
  transactional redacted outbox/receipts, account-scoped downloads/cursors and
  shared history-link resolution. First startup imports archived Parquet once.
- Runtime CLI hooks, OTLP logs, Claude Code backfill, Cowork, Claude Desktop
  history/plugin usage and Codex all commit through the same local sink.
  OTLP success and tailer checkpoints follow successful local persistence.
- UI and CLI query the collector's authenticated local API/connection.
- Cloud repository migration 0021 and `/v2/self/events`, `/v2/self/sync`:
  current device/membership authentication, server-assigned ownership, account
  RLS, serialized per-account cursor allocation, idempotent acknowledgments.
  Private self-sync records do not enter the separate team analytics relation.
- Real PostgreSQL integration tests verify ordinary-member self-sync, two devices
  belonging to one person, cross-account denial, private/team separation,
  owner/admin analytics, membership revocation and conflicting replay.
- Cross-repository test launches the actual `CloudSink` with two DuckDB files:
  bidirectional sync, restart, Haiku 2 input / 16 output, conversation linking
  and OTel provenance pass. Synthetic test data stays in the local test service.
- Real-history snapshot import preserves 34,731 events, including the previously
  verified Haiku 2/16 and Sonnet 2/15 responses in the same Claude conversation.

Remaining boundaries: general GenAI spans/metrics normalization is not implemented.
The change stream and local store support tombstones, but a user-facing Cloud
history deletion endpoint and retained-cursor reset protocol are not yet shipped.
Cloud synchronization requires an explicitly configured Cloud account; local-only
collection does not silently enable uploads. Downloaded partitions are detached at
startup until identity has been verified, and immediately detached on 401/403.
Signing out hides cached data; it does not erase the local history (filesystem permissions apply; application-level encryption is not added).

Repository restrictions were removed by user request. All collected metadata is
synchronized; team analytics remain administrator-only. Prompt/tool body fields
are still redacted independently of repository membership.
