# Claude collector

## Recommended: account-scoped Cloud receiver (0.5.0)

Each user signs in at https://kikimimi.dev/settings and creates their own ZIP.
The ZIP sends usage directly to their Cloud account; no per-user tunnel or Mac
receiver is needed. Creating a new ZIP invalidates the previous key. Never share
personalized ZIPs: they contain a send-only credential for the downloading account.
The Settings page shows the response count, last received time and HTTP status.

Install/enable the ZIP in Claude, allow `kikimimi.dev` under Additional allowed
domains, and start a new conversation with cloud code execution. Responses before
the cloud workspace starts remain unmeasured. Cloud-only records appear under
separate plugin sessions; 0.5.0 does not perform local history fingerprint linking.

To view this private usage in Desktop 0.8.3, sign in to the same kikimimi account
and enable Cloud sync. Local-only collection does not download Cloud records.
The Cloud receiver never automatically shares plugin usage with a team.
Disable the old tunnel plugin when switching; its key/destination are unchanged.

## Optional: Mac-hosted receiver (0.4.1)

Uses Claude's supported Stop hook and a dedicated HTTPS endpoint. Does not modify
Claude.app. Only completed transcript records with real usage fields qualify;
reconstructed history without usage is never presented as measured usage.

## Credentials

- `kikimimi claude-plugin enable --url https://HOST/v1/claude-plugin` generates
  a 256-bit send-only credential in macOS Keychain, without an expiry. Run again
  to rotate. The receiver stores only its SHA-256 digest in a private config.
- `kikimimi claude-plugin export --url https://HOST/v1/claude-plugin --output /private/tmp/collector-export.json`
  creates a private packaging input. Use `package_collector.py --url ...
  --config /private/tmp/collector-export.json --output /private/tmp/collector.zip`.
  Upload the archive as a standard Claude plugin, then delete both temporary files.
- `kikimimi claude-plugin revoke` rejects subsequent uploads without restarting.
  Rotation requires exporting and uploading a new plugin. The credential remains valid until revoked or rotated; no periodic re-upload is required.
- The plugin's credential is available inside Claude's execution environment.
  It grants ingestion only, never reading the dashboard or existing records.
  Do not put Cloudflare management credentials in the plugin or chat.

## Tunnel

Use a named Cloudflare Tunnel with an exact hostname and only the ingestion path.
Route to `http://127.0.0.1:43192`; all other paths/hosts must return 404. Do not
route to the dashboard. Use a separate user LaunchAgent for cloudflared with
RunAtLoad and KeepAlive. Keep its tunnel-specific credentials private (0600).
The account certificate created by cloudflared login is broader than the runtime
credential; remove/revoke setup access when no longer needed. Never overwrite an
existing DNS record or another cloudflared service.

Allow only the exact ingestion hostname in Claude's Additional allowed domains.
For the current deployment, add `ingest.kikimimi.dev` manually under Settings >
Capabilities, keeping Package managers only. Installation does not grant network
access. Start a new conversation after changing the setting. The collector sends
an honest `kikimimi-collector/<version>` User-Agent because CDN integrity checks
may reject Python's default User-Agent; this does not bypass Claude's egress policy.
TLS terminates at Cloudflare, so Cloudflare can see the telemetry. This is not
end-to-end encryption. No prompt or response content is sent.

## Receiver defenses and limits

Loopback-only listener, one POST route, authentication before body consumption,
constant-time digest comparison, 64 KiB bodies, 5-second body deadline, at most
8 concurrent requests and 120 requests/minute globally. All fields are strictly
validated; unknown fields (including conversation text) are rejected. Durable,
private atomic writes precede acknowledgment. Replay never adds usage; conflicting
records with the same identity are rejected. The inbox is capped at 20,000 files
and requires a future retention/compaction feature for indefinite use. A full
inbox rejects new data instead of silently discarding it.

The hook retries unacknowledged records on the next Stop, reads the last 8 MiB of
the transcript, serializes concurrent hooks, never follows redirects, and has a
7-second delivery budget. Offline delivery is not guaranteed if the remote
execution environment disappears before another Stop. A valid upload proves
possession of the credential, not an Anthropic-signed attestation of token usage.

## Measurement boundaries

Current Claude cloud execution must initialize before hooks run. A first response
that never starts that environment may be missing. Plugin internal session IDs
are not the chat URL ID. Version 0.4.1 links uniquely matching response content
to cached chat messages within 120 seconds, explicitly labeled content matching.
Ambiguous or unavailable matches stay in a separate plugin session. Do not claim
all ordinary Claude chats are covered until verified on the actual supported UI.
Reconstructed history can also have numeric zero usage and a placeholder model;
all-zero usage records are excluded because they do not establish a measured call.
Input tokens exclude cache reads/writes, which are stored separately. Synthetic
fixtures are tests only; end-to-end acceptance requires real generated records,
Claude restart, durable inbox/Parquet evidence, and visible kikimimi UI records.

### Which turns are measured (2026-11-03, unified Chat/Cowork)

Chat and Cowork are one conversation type now; the Stop hook runs inside the
conversation's cloud workspace. On claude.ai (Opus 5.5, collector 0.4.1):

| Conversation | Turn | Hook record |
| --- | --- | --- |
| `2f6eae6c` text only | 1 (text) | none |
| | 2 (text) | none |
| `880d71ca` cloud Bash first | 1 (Bash, then text) | yes |
| | 2 (text) | yes |

A conversation that never starts the cloud workspace sends nothing, including
replies that only use device tools on the Mac. Once the workspace has run, later
text-only turns are measured. Text-only conversations therefore stay estimated.

Intermediate tool-use responses are counted; the response that starts the
workspace is not. In `bf6d58f9` (text turn, then one turn with three sequential
cloud Bash calls), the workspace transcript
(`/root/.claude/projects/-home-claude/<session>.jsonl`) held:

| Response | Content | Transcript usage | Hook record |
| --- | --- | --- | --- |
| `msg_3710…` | first text turn | none (reconstructed) | none |
| `msg_f4ed…` | `echo step1` tool call | output 0 (reconstructed) | none |
| `msg_011CgfcvK…` | `echo step2` tool call | output 74 | yes |
| `msg_011CgfcvU…` | `echo step3` tool call | output 74 | yes |
| `msg_011Cgfcvc…` | final text | output 3 | yes |

Everything before the workspace starts, including the API response whose tool
call starts it, is rebuilt with placeholder IDs and no or zero usage, so it is
excluded. Every later response, whether a tool call or text, arrived. Each
conversation is therefore missing at least its pre-workspace responses.

Folder grants cannot be tested on claude.ai: the web composer's Devices menu
only runs tasks on a linked computer, and folder access is desktop-only.

## Verified delivery (2026-11-01)

On the user's personal Ultra account, the exact ingestion hostname was manually
allowed and collector 0.3.2 uploaded through Claude's standard plugin UI. Claude
4.4212.0 was quit and reopened without modifying its application bundle. A new
Haiku conversation (`224919cb-39f6-4215-b84d-98119bf255ea`) produced these completed
records automatically in the private inbox and Parquet:

| Response ID | Model | Input | Output | Cache read | Cache write |
| --- | --- | ---: | ---: | ---: | ---: |
| msg_011CgcDGpYXYya7EFYBMqBAc | claude-haiku-5-5 | 2 | 11 | 99927 | 13152 |
| msg_011CgcDJ5x1bGRZMcgsb2o1r | claude-haiku-5-5 | 2 | 12 | 113079 | 380 |

The second response used no tools after the first initialized cloud execution.
Internal plugin session: `6ad10ca7-b9d2-55eb-b34a-d10ce4297f5a`. This is evidence
for this execution path and account, not proof of first-turn text-only coverage
or coverage across all personal plan tiers. No collector script was run manually.

## Automatic history linking (0.4.1)

The hook sends an HMAC-SHA256 fingerprint of assistant text (not thinking or tool
content), using the ASCII SHA-256 digest of its send credential as the key and
`kikimimi-response-v1\0` as a domain separator. CRLF is normalized to LF. The
local collector computes the same value from the existing Claude history cache.
Plaintext is neither uploaded nor persisted by this feature. Fingerprints are
sensitive pseudonymous metadata and stay in the private ingestion inbox.

Both directions must have exactly one candidate within 120 seconds. A later
ambiguous candidate vetoes any prior match. This is heuristic content matching,
not a shared server identity or a guarantee against indistinguishable messages.
Metadata-only links are saved separately; dashboard queries resolve them without
copying or counting API usage twice. CLI/cloud query paths are not migrated by
this feature. Missing text, missing local history, or different representations
remain unlinked. Key rotation cannot newly match older-key fingerprints; already
saved links survive. Existing pre-0.4.1 uploads need a hook replay to gain a
fingerprint, so historical backfill is not guaranteed.

### Real response verification, 2026-11-01

After uploading 0.4.1 and normally restarting unmodified Claude 4.4212.0, chat
`0caac1cf-17d8-4552-8a66-d718adf6292c` generated two responses. The first initialized
cloud execution with a harmless printf; the second followed a Haiku → Sonnet
switch and used no tools. Both automatically reached the receiver and linked to
the actual chat's separate assistant messages:

| Model | API response | Chat message | Input | Output | Cache read | Cache write |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| claude-haiku-5-5 | msg_011CgcECFf7WwdZjQwvjmXCr | 857b6253-475c-4edc-a73f-1276040cab93 | 2 | 16 | 99900 | 13143 |
| claude-sonnet-5-5 | msg_011CgcEF2drbp8ZiTf5dnfr5 | 2aebebca-59b1-4488-a442-efbdda0f962c | 2 | 15 | 0 | 95297 |

Internal session: `618690f4-413c-5535-8c3e-d441c325a170`. No manual mapping, fake
usage, or manual hook execution was used. A local flush accelerated durable
storage during verification; it did not collect or link responses.

The installed kikimimi UI visibly showed both rows under Session `0caac1cf`,
with distinct chat message IDs, Unique content match labels, and measured totals
4 input / 31 output. Estimated visible-text totals remained separate. Rust
matching, ambiguity-veto, local-cache privacy, receiver, dashboard query tests
and Python collector tests passed; both desktop and web builds passed.
