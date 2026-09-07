---
title: The improvement loop
description: How to go from a struggle ranking to a fixed MCP server and prove it worked — the loop kikimimi exists for, and how to run it with one team.
---

kikimimi's cost views are the entrance. The product is the loop: detect where agents struggle, point at the MCP server or tool responsible, fix it, and see the same pattern go down on the same screen. This page is the runbook for running that loop once with a real team, and for checking that the detections are worth trusting.

## Prerequisites

- Every machine on the team runs kikimimi 0.6.0 or later with `kikimimi init` done (hooks + OTel, so tool calls and token usage both arrive).
- The machines are logged in to the same team org (`kikimimi login --org <slug>`; see [Teams](/kikimimi/teams/)). Personal orgs work too for a single person.
- On the hosted cloud the pattern scanner is running (it is on by default; `kikimimi status` shows nothing about it — it lives server-side and rescans every 5 minutes). Locally, `kikimimi web` computes the same views on demand.

Give it a week of normal work before reading the ranking. Patterns need sessions.

## Step 1 — read the ranking

Open **Struggles** in `kikimimi web` (or the hosted web). Every row is one pattern pointing at one subject — an MCP server or a tool, never a person — sorted by `wasted tokens × sessions`.

| Pattern | What it means | What to do about the subject |
|---|---|---|
| MCP bypass | An MCP tool failed and the agent reached for Bash or a browser within 5 events | The server is missing a tool, a permission, or a usable error message. Or the CLI is simply better here: decide which |
| Configured, never called | The server was loaded in the session and never used; you paid its schema on every request | Remove it from the config for that repo, or fix its tool descriptions so the model picks it |
| Retry spiral | The same tool failed 3+ times in a row | The error text doesn't tell the model what to change. Improve it, or tighten the input schema |
| Denied, then detoured | A permission denial followed by Bash or a browser | The allowlist is fighting the workflow. Allow the tool, or make the detour the official path |
| Permission denied loop | The same tool denied repeatedly | Same as above; the model didn't get the message |
| Context bloat | The context jumped by 20k+ tokens right after this tool's result | The tool returns too much. Add pagination, filters, or a summary mode |
| Slow MCP call | A call took 10 s+ and 3× the tool's median that day | Server performance |

Click a row for the sessions behind it. The drilldown is metadata only (session id, time, incident count, estimated cost, a small detail object). In a team org, members see their own sessions there; admins and owners see everyone's and leave an audit trail.

`unknown` in the cost column means no token usage landed inside those incidents — usually a session without OTel. It is never shown as zero.

## Step 2 — pick one subject and fix it

Take the top row whose subject your team owns. Typical fixes, in order of cheapness:

1. Rewrite the tool's description so the model knows when to use it and what the arguments mean.
2. Make the error message say what to change ("`repo` must be `owner/name`", not "bad request").
3. Add the missing tool or parameter the detour was compensating for (look at `detour_tool` and the Bash command shape in the session).
4. Remove the server from the repos where it is never called.

Ship the change. Then, on the same Struggles row, open the drilldown, scroll to **Trend**, and record an improvement mark with the date and a one-line note. Marks are per (pattern, subject); in a team org they need admin or owner.

## Step 3 — check the same screen a week later

The Trend section shows, per day, how many sessions ran, how many hit the pattern, the hit rate, and the estimated wasted tokens. With a mark set it also shows the session-weighted hit rate and total cost before versus after the mark. That before/after is the whole result: if the rate dropped, the fix worked; if it didn't, the subject is not the real cause, or the fix didn't reach the model.

Watch for the two ways this can lie:

- **Too few sessions.** A rate over 3 sessions is noise. Compare weeks, not days.
- **The behaviour moved, not disappeared.** If MCP bypass for server X drops but a new row appears for its CLI equivalent under Context bloat, the agents switched detour. That may still be the right outcome — the goal is a decision, not a zero.

## Measuring false positives and negatives

Detections are proxies over metadata. Before trusting a ranking enough to spend a sprint on it, sample it:

1. From the drilldown of the top three rows, open 10 sessions each in the agent's own transcript (Claude Code: `~/.claude/projects/<repo>/<session>.jsonl`; Codex: `~/.codex/sessions/`).
2. For each, answer one question: *did the agent actually struggle with this subject?* Count the noes. That is your false-positive rate per pattern.
3. For false negatives, take 10 sessions you know were painful (ask the team) and check whether they appear under any row.

Record both numbers in your own notes with the kikimimi version. kikimimi's own measurements are in the [Queries](/kikimimi/queries/#patterns) honesty notes, and each threshold is a constant in `crates/cloud/src/patterns.rs` and `crates/cli/src/query_cmd.rs`; the first two real-data corrections were:

- `context_bloat` compares a request with the previous one *of the same model in the same conversation stream* (main conversation or one subagent) and requires the jump to be sustained by the next request. Claude Code interleaves small Haiku helper calls with the main model, and parallel subagents share the session id and alternate between two context sizes; on one machine's real data the naive rule flagged 35 jumps in one session and the same-model rule 30 (OTel rows only); with the per-stream rule over a full transcript backfill the busiest session has 16 and 60 sessions together 27 (see [subagents](/kikimimi/queries/#subagents)).
- `long_tool_tail` uses the tool's daily median, not p95: with a day's worth of samples the outlier is its own p95.

If you find another systematic false positive, that is the most useful issue you can open.

## What is not detected yet

The full `mcp_bypass` (a resource-to-MCP map, so a raw `curl` to a host that has an MCP server counts) is Stage 2. Subagent fan-out is a query and a page ([subagents](/kikimimi/queries/#subagents)), not a ranked pattern yet: it needs a threshold worth arguing about first. Sessions that straddle midnight UTC are priced within each day separately.
