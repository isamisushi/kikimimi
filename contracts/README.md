# Client / Cloud API boundary

OSS builds the CLI, desktop app and local/S3 dashboard. The private
`kikimimi-cloud` builds the paid dashboard, API, authentication, billing and Router.
Neither build checks out or imports the other repository. The shared boundary is
HTTP and versioned event payloads; there is no shared browser login implementation.

## HTTP contract

* `/web/*`: browser session API. `kikimimi_session` is an HttpOnly, Secure,
  SameSite=Lax, host-only cookie. `GET /web/me` returns `orgs` (slug, name, kind,
  role) and `active_org`; `POST /web/active-org` accepts `{ "slug": "..." }`.
* `/auth/*`, `/activate`, and `POST /join/:token`: server-owned authentication
  flows, callbacks and approval forms. Keep their query strings, request bodies,
  multiple Set-Cookie headers and redirects intact.
* `/v1/*`: device bearer-token API, including ingestion, export and Router keys.
  Browser workspace changes do not change a device token's organization.
* `/web/orgs/:slug/router` (GET/PUT), `/web/orgs/:slug/router/usage` (GET),
  `/web/router/keys` (GET/POST), `/web/router/keys/:id` (DELETE): hosted Router UI.
  The server checks organization membership and role on every request.
* `/v1/router/keys` (POST) and `/v1/router/keys/:id` (DELETE) use the device's org.
  New keys return `id`, `key`, `base_url`, `auto_model`, `models`.
* Errors use HTTP status plus JSON `{ "error": "..." }`; unknown API routes
  return 404 JSON, never the dashboard HTML. A missing browser session is 401.

## Event contract

`kikimimi-v1.json` is a fixed wire compatibility fixture, including field types
and Parquet column order. The client serializer and server DTO independently
round-trip it in tests. Keep a copy in both repositories; this is contract data,
not a source dependency. Exchange updated fixtures when extending the protocol.
New fields must be optional for old senders; receivers ignore unknown fields.
Breaking changes require a new schema version. Never rename/remove v1 fields or
reorder export columns. The fixture is a baseline, not a substitute for a live
client-to-server ingestion/export test.

## Authentication and origins

Cloud serves its private dashboard and browser APIs from the same origin.
`KIKIMIMI_PUBLIC_BASE_URL` identifies that origin for OAuth callbacks, device
activation, email links and invitations. The OSS client opens the Cloud device
approval page and stores a scoped device token; it does not copy browser cookies.
Local dashboard authentication uses a separate loopback-only token/cookie and
works without a Cloud account or connection. Browser workspace switches never
retarget an already registered device. API authorization stays server-side.

`/web/*` management endpoints are internal to the private dashboard. OSS clients
use the device authentication and `/v1/*` contracts. Neither app needs the other's
source tree at build time or runtime.
