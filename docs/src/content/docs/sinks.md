---
title: Bring your own bucket
description: A full-fidelity copy of your local Parquet in your own S3 bucket, uploaded through your own aws CLI — kikimimi never stores credentials.
---

The local Parquet sink (`~/.kikimimi/data/events/`) is always on. A BYO sink is an additional, opt-in copy — currently S3 and S3-compatible storage (R2, MinIO, via `--endpoint-url`).

## Add an S3 sink

```sh
kikimimi sink add s3 s3://bucket/prefix
kikimimi sink add s3 s3://bucket/prefix --profile my-aws-profile
kikimimi sink add s3 s3://bucket/prefix --endpoint-url https://<account>.r2.cloudflarestorage.com
```

```sh
kikimimi sink list             # configured sinks (file/cloud/s3)
kikimimi sink remove s3        # stop writing to it (leaves what's already uploaded)
```

`kikimimi sink add s3` replaces any existing S3 sink config rather than adding a second one — one S3 destination per host.

## Your credentials, not kikimimi's

kikimimi never reads, stores, or transmits AWS credentials. Every upload shells out to your own `aws` CLI (`aws s3 cp ...`, plus `--profile`/`--endpoint-url` if you set them) — whatever `aws` on this machine is already configured to use: a profile, SSO, an instance role, environment credentials. If `aws` isn't on `PATH`, uploads fail with `aws CLI not found` and retry on the next flush once it is.

## What gets uploaded

The S3 sink writes the same `kikimimi.v1` Parquet schema as the local file sink, and it's a **full-fidelity, unfiltered copy** — every column that's populated locally goes to your bucket exactly as recorded. This is the opposite of the hosted cloud sink, which forces `tool_input_json`, `tool_output_excerpt`, and `prompt_text` to `NULL` before it ever leaves your machine (see [Privacy](/kikimimi/privacy/)). A BYO sink is your bucket — kikimimi doesn't mask anything on the way there.

Objects land under:

```
s3://bucket/prefix/kikimimi.v1/events/dt=YYYY-MM-DD/<host>-<seq>-<uuid>.parquet
```

## Staging directory and retry queue

A flush doesn't upload straight from memory. It writes the buffered events to a local staging directory first (partitioned by `dt=` the same way the file sink is), then runs `aws s3 cp` against **every** Parquet file currently sitting in staging — not just the ones from this flush, but any left over from a previous flush that failed to upload. The staging directory itself is the retry queue: nothing separate is tracked to remember what still needs to go out.

- Flushes on the same schedule as the other sinks: every 500 buffered events, or every 60 seconds, whichever comes first — plus an extra flush whenever staging has leftover files even if nothing new has arrived, so a fixed outage doesn't wait for new events to be retried.
- Each file gets up to 3 upload attempts, with a short backoff between them, before it's left for the next flush cycle.
- A successfully uploaded file is deleted from staging; a failed one stays and is retried later.
- Staging is capped at 64 MB total — if an extended outage lets it grow past that, the oldest files are deleted to make room rather than letting it grow unbounded. Those files are gone, not retried; this only bites during a sustained S3-side or network outage.

## Checking status

`kikimimi status` shows the sink's live state — configured URL, whether uploads are current, and the last error if any:

```
s3: not configured (run `kikimimi sink add s3 <s3://bucket/prefix>`)
```

Once configured, the same section reports pending count, last push time, and last error, the same shape as the cloud sink's status block.
