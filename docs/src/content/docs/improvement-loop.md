---
title: The improvement loop
description: Check collection, investigate a recurring problem, record a change, and compare observed recurrence without claiming unmeasured savings.
---

The local dashboard's **Improvements** page connects evidence, a concrete change, and follow-up observations. It identifies investigation leads, not proven waste, root causes, or guaranteed savings. Start with repeated tool failures or permission-denial loops.

## First check collection

The collection panel above the activity filters shows the state of **this machine**, independently of the devices and repositories selected for browsing. It refreshes every 10 seconds and displays:

- Collector connectivity and a heartbeat written independently of agent activity. A service-manager entry alone is not proof that collection is working. A missing heartbeat, including one from an older collector, is shown as unconfirmed rather than healthy.
- The last recorded event and Parquet batch, and the collector backlog. An old event can mean the agent is idle; it does not by itself mean collection has stopped. Event timestamps may refer to backfilled history.
- Cloud/S3 sync status, pending counts and the last successful push. Old counters are historical when the heartbeat is stale. A saved destination must be confirmed by the collector before its sync can be described as current.
- Reported ingestion, storage or parsing problems. These indicate potential gaps; the dashboard cannot measure activity that was never captured.

Use **Open collection settings** to inspect the setup. Merely viewing a page never starts collection or changes a sync destination. An S3 snapshot is read-only, and the viewing machine's health does not establish the health of the exporting devices.

## 1. Find and inspect a candidate

Open **Improvements**. Repeated failures and permission-denial loops appear first, then candidates are ordered by affected sessions. Each row identifies a tool or MCP server, never an individual. Select a row to see the detection's meaning, suggested checks, limitations and affected sessions. Open a session to inspect its event metadata; use the agent's own history for message contents.

| Signal | Suggested investigation | Limitation |
|---|---|---|
| Retry spiral | Tool authentication, inputs and availability | Metadata alone cannot establish the failure's cause |
| Permission denied loop | Intended operation, instructions and narrowly scoped permissions | A denial may be intentional; do not broaden permissions just to clear a signal |
| MCP bypass | Connection/authentication failures and the chosen fallback | A shell/browser fallback may be a good choice |
| Denied, then detoured | Whether the later operation respected the permission boundary | Nearby events do not prove a bypass |
| Context bloat | Whether oversized responses can be narrowed | The preceding tool need not have caused the jump |
| Slow MCP call | Response size, connection and server latency | Latency is not a measured LLM charge |
| Configured, never called | Whether this workflow needs the configured server | Unused does not prove billed overhead; token allocation is a proxy |

Associated token counts are estimates of usage around the detected events, not an amount that can necessarily be avoided. Unknown quantities stay unknown.

## 2. Make and record a change

Change the relevant agent, tool or repository configuration outside the dashboard. In the candidate's details, use **Record a change** to save the UTC date and a description of what you changed. This records an intervention; it does not mark the issue as resolved.

Records are saved on this machine, scoped to the selected device and repository filters. They survive dashboard/server restarts and remain in **Changes to follow up** even when no candidate appears in the last 30 days. Return with the same filters to find the record. Existing records without a scope belong to the all-devices/all-repositories view. Records are not uploaded to Cloud or S3; hosted Cloud workflows are maintained in the separate `kikimimi-cloud` repository.

## 3. Check what happened next

Resume comparable work, then open the recorded change from **Changes to follow up**. The latest change date defines the comparison:

- The change day is excluded because its exact time is unknown. Today is excluded because it is incomplete. All boundaries use UTC, matching stored event partitions.
- Both sides use the same number of calendar days, up to 14. For changes older than two weeks the comparison uses the two weeks immediately before and after the change, within the available 60-day timeline.
- The denominator is observed eligible session-days, rather than all unrelated sessions. For example, repeated-failure comparisons require an observed result or denial for that tool. Removing a server entirely gives no later eligible activity for its unused-server rate; it does not fabricate a zero.
- At least five eligible session-days over two active days on **each** side are required to display a trend. These thresholds are an observation guard, not a statistical significance test.
- Without later activity the result is **Waiting**. Small samples show **More observations needed**. Unconfirmed collection prevents a positive interpretation. An observed lower rate or no recurrence is described as an observation, never an automatic resolution.

Inspect **Daily evidence (UTC)** for the underlying counts. Missing days are absent observations, not successful work. A currently healthy collector does not prove complete collection during an earlier period. The comparison also cannot verify equivalent workload, model mix or task quality. Check these independently before attributing a change to the intervention.

To substantiate a cost reduction, compare actual bills at comparable workload and quality, accounting for subscription terms and other changes. API-equivalent estimates and fewer detected problems are not invoice savings.

## Check detection quality

Sample affected sessions from several candidates and compare the signal with what actually happened in the agent's own transcript. Also check known problematic sessions that were not detected. Record false positives and missed cases with the collector version. Detection thresholds and their limitations are documented in [Queries](/kikimimi/queries/).
