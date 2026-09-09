---
title: Storage & sharing
description: Choose local analysis, personal or team Cloud, and optional S3 export independently.
---

Every collecting machine keeps local Parquet history. No account is needed to use it. Cloud and S3 are optional additional destinations.

| What you want | Use |
|---|---|
| Analyze one machine without sharing | Mac app or `kikimimi web` |
| Combine your own laptops, VMs and CI | Connect each collector to the same Personal workspace in Cloud |
| Share activity with a team | Connect each collector to a team workspace in Cloud |
| Keep a copy in your own infrastructure | Add an S3 sink, with or without Cloud |
| View a team's shared S3 export | Connect the export in Storage & sharing from each member's Mac app or local web |

The Mac app toolbar identifies the selected workspace and viewing connection. In local web, Storage & sharing identifies the viewing source; Cloud shows Personal or the team name. Switching the workspace in Cloud changes what you view, not where a collector sends data. One collector sends to one Cloud workspace at a time.

## Local

Use the Mac app's dashboard or run `kikimimi web`. Both use the same analysis interface. History is stored as files on the collector's machine, not in your browser's localStorage. The Mac app can show saved history with collection stopped.

Open **Storage & sharing** in the dashboard to see configured destinations. “Configured” does not mean the last upload succeeded; use `kikimimi status` for pending uploads and errors.

## Personal and team Cloud

Run `kikimimi login` on each collecting machine and approve the code in your browser. For a team, create or join the team in Cloud, then specify the repositories to share as part of the connection:

```sh
kikimimi login --org your-team --repo 'github.com/your-team/*'
```

Repeat `--repo` for multiple patterns. Specifying patterns replaces the saved allowlist before the connection becomes active. Omitting the flag preserves the existing list. With no filter, a team receives all repositories; Personal ignores this filter. The approved workspace, not the `--org` hint alone, determines the destination.

`kikimimi logout` removes the Cloud connection and notifies the running collector. Local history remains. Read [Teams](/kikimimi/teams/) for roles, invitations and repository matching.

Cloud still needs a collector wherever your agents run. Opening Cloud from the Mac app shows it inside the app and does not enable sharing.

## S3 export

```sh
kikimimi sink add s3 s3://your-bucket/prefix
kikimimi status
kikimimi flush
kikimimi sink remove s3
```

S3 exports every locally recorded field and repository, without the Cloud filter or masking. Adding S3 does not automatically replay existing Parquet history. Removing it stops future uploads and keeps uploaded objects. Each team member can view the shared export in the same dashboard using their own AWS read access; see [Team dashboard from S3](/kikimimi/s3-dashboard/). See [Bring your own bucket](/kikimimi/sinks/) for upload credentials, compatible endpoints, and retry limits.

## Mac app installation

If you only installed the Mac app, replace `kikimimi` in the commands above with `/Applications/kikimimi.app/Contents/MacOS/kikimimi`. Run setup on the collecting machine; a remote collector's settings cannot be changed from Cloud.

## Destination changes and older retry files

Pending Cloud uploads are separated by destination, workspace, sharing policy, and machine. S3 pending files are separated by bucket, endpoint, and machine. Changing or removing a destination stages pending data for its original destination, without uploading it elsewhere. Reconnecting to that destination resumes its queue.

Older `cloud-pending.jsonl` and `s3-staging/dt=*` queues have no reliable destination binding. Upgrades leave them untouched and do not send them automatically. Review the original destination before recovering those files; automatic recovery of legacy queues is not provided. Local history is unaffected.
