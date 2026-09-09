---
title: Team dashboard from S3
description: Read a shared S3 export in the Mac app or local web dashboard, without PostgreSQL or Kikimimi Cloud.
---

Team members can view the same S3 export in their own Mac app or local web dashboard. Collection, S3 upload, and dashboard access are separate choices. A reader does not need to collect their own activity or have a Kikimimi Cloud account.

## 1. Collect into a shared prefix

On each collecting machine, configure the same output destination:

```sh
kikimimi sink add s3 s3://team-bucket/shared --profile team-writer
kikimimi status
```

The collector writes beneath `shared/kikimimi.v1/events/dt=YYYY-MM-DD/`. Each machine keeps its local history too. S3 receives all recorded fields and repositories; Cloud repository filters do not restrict this output. Only include machines whose exported activity belongs in this shared dataset.

## 2. Give readers their own AWS access

Install AWS CLI v2 on each viewing machine and sign in through your existing profile, SSO or role. For an SSO profile, for example:

```sh
aws sso login --profile team-reader
```

The reader needs `s3:ListBucket` for the export prefix and `s3:GetObject` for its objects. It does not need upload or delete permission. The bucket owner can attach a policy like this to a reader role, substituting their bucket and prefix:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "s3:ListBucket",
      "Resource": "arn:aws:s3:::team-bucket",
      "Condition": { "StringLike": { "s3:prefix": "shared/kikimimi.v1/events/*" } }
    },
    {
      "Effect": "Allow",
      "Action": "s3:GetObject",
      "Resource": "arn:aws:s3:::team-bucket/shared/kikimimi.v1/events/*"
    }
  ]
}
```

Storage encrypted with KMS can require additional key permissions. See the AWS references for [listing permissions](https://docs.aws.amazon.com/cli/latest/reference/s3api/list-objects-v2.html) and [object reads and encryption](https://docs.aws.amazon.com/cli/latest/reference/s3api/get-object.html).

All data in this prefix is visible to anyone with this read access. This mode does not implement Cloud member roles or per-session restrictions. Use different export prefixes and AWS permissions for different audiences. Revoking AWS access prevents new reads; it cannot retract copies already downloaded by a reader.

## 3. Connect the dashboard

In the Mac app, choose **View without collecting** if you only want to read team activity. Choose **Team · S3** from the workspace menu, open **Viewing Connection**, enter `s3://team-bucket/shared`, optionally choose the AWS profile and S3-compatible endpoint, then select **Connect and view**.

For a browser without the Mac app, run:

```sh
kikimimi web --read-only
```

Open the local URL it prints, then open **Storage & sharing → What to show in your dashboard** and select **Connect and view S3**. Keep the process running. This command does not install hooks, start collection, or register a login service. The CLI viewer needs DuckDB on PATH; the Mac app bundles it. Both need AWS CLI v2 available to the viewing process.

The dashboard shows the S3 destination and the snapshot timestamp. Overview combines machines; Sessions and the other analysis pages use the same S3 snapshot. MCP configuration comes from exported session snapshots, never from the viewing machine's settings. Personal subscription limits and improvement-mark editing are unavailable in S3 mode.

In local web, choose **View this machine instead** to return to local history. In the Mac app, choose **Personal** and set its Viewing Connection to **This Mac**. Connecting a read source does not change your S3 upload destination or enable uploads.

## Refresh, caching and limits

**Refresh from S3** checks every object under the export layout, reuses unchanged objects, downloads changed ones, and creates a complete new snapshot. Repeated `event_id` values are counted once. Deleted objects disappear after a successful refresh. Every download uses the listed object ETag; a version change during download fails the refresh instead of publishing a partially refreshed dataset.

Queries continue using the previous immutable snapshot during refresh. If listing, access, download or Parquet validation fails, the previous snapshot and timestamp remain visible with an error. A failed first load is an error, not an empty dashboard. Refresh is manual; the timestamp indicates how current the displayed data is.

The current reader accepts up to **512 MiB of source Parquet and 10,000 objects**, with a three-minute refresh limit. Larger exports fail explicitly; they are not silently truncated. Connect a smaller export prefix when needed. Materialization and DuckDB working files need additional local disk space.

Snapshots are stored in private temporary directories beneath `~/.kikimimi/s3-reader-cache`. Normal viewer shutdown removes them; an interrupted process can leave a temporary directory behind. Cached files and connection metadata are local to each reader. AWS credentials stay with AWS CLI and are not copied into kikimimi settings or sent to Kikimimi Cloud.

This is **a shared dataset viewed from each member's app**, not a team-hosted URL. A centrally hosted dashboard with browser sign-in and member-level authorization remains a separate deployment mode.
