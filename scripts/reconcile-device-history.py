#!/usr/bin/env python3
"""Plan or install a device-ID recovery from private JSON arrays of Event records.

Use the previous local ledger for matching (Cloud masks fields), and the
authenticated Cloud snapshot to prove coverage. Output is a one-to-one alias
list. Only --apply-to writes to the stopped ledger. No network access.
"""
import argparse
import json
import math
import subprocess
from collections import Counter, defaultdict
from pathlib import Path


def fields(event, *names):
    return tuple(event.get(name) for name in names)


def index(events, key):
    groups = defaultdict(list)
    for event in events:
        value = key(event)
        if value is not None:
            groups[value].append(event)
    return groups


class Recovery:
    def __init__(self, current, previous):
        self.current = current
        self.previous = previous
        self.aliases = {}
        self.used = set()

    def pair(self, local, remote, basis):
        lid, rid = local["event_id"], remote["event_id"]
        if lid in self.aliases or rid in self.used:
            raise ValueError("An observation has two proposed counterparts")
        self.aliases[lid] = {"local_id": lid, "remote_id": rid, "basis": basis}
        self.used.add(rid)

    def match(self, key, basis, *, unique=False, previous_key=None):
        before = index((r for r in self.previous if r["event_id"] not in self.used),
                       previous_key or key)
        after = index((r for r in self.current if r["event_id"] not in self.aliases), key)
        for identity in before.keys() & after.keys():
            left, right = after[identity], before[identity]
            if unique and (len(left) != 1 or len(right) != 1):
                raise ValueError(f"Ambiguous {basis} in session {left[0]['session_id']}")
            # For genuinely indistinguishable observations preserve multiplicity.
            for local, remote in zip(sorted(left, key=lambda r: r["event_id"]),
                                     sorted(right, key=lambda r: r["event_id"])):
                if (local.get("reasoning_tokens") is not None and remote.get("reasoning_tokens") is not None
                        and local["reasoning_tokens"] != remote["reasoning_tokens"]):
                    raise ValueError("Conflicting reasoning usage for matched request")
                self.pair(local, remote, basis)

    def run(self, match_prompt_times):
        ignored = {"event_id", "host_id", "client", "user_id", "org_id", "team_id",
                   "user_id_source", "redaction_applied"}

        def observation(event, excluded):
            if event.get("session_id"):
                return tuple(sorted((k, v) for k, v in event.items()
                                    if k not in excluded and v is not None))

        self.match(lambda e: observation(e, ignored), "same source observation")

        def codex_context(e):
            if (e["agent"], e["source"]) == ("codex", "log") and e["event_type"] in ("turn", "api.request"):
                return observation(e, ignored | {"model", "effort"})
        self.match(codex_context, "same Codex record with different session context")

        def claude(e, source, kinds):
            return (e["agent"] == "claude-code" and e["source"] == source
                    and e["event_type"] in kinds and e.get("session_id"))

        def call_key(e, source):
            if claude(e, source, ("tool.call", "tool.result", "subagent.stop")) and e.get("correlation_key"):
                return fields(e, "session_id", "event_type", "correlation_key")

        for source in ("hook", "otel"):
            self.match(lambda e: call_key(e, "log"), "same Claude operation ID",
                       previous_key=lambda e: call_key(e, source), unique=True)

        def api_key(e, source):
            if (claude(e, source, ("api.request",)) and e.get("model")
                    and e.get("input_tokens") is not None and e.get("output_tokens") is not None):
                return fields(e, "session_id", "model", "input_tokens", "output_tokens",
                              "cache_read_tokens", "cache_write_tokens")
        self.match(lambda e: api_key(e, "log"), "unique Claude model and usage within session",
                   previous_key=lambda e: api_key(e, "otel"), unique=True)

        # Transcript boundaries describe the whole file. Live hooks can record
        # several resumes. Keep every live boundary; map only the file's first
        # start or its final close when that close covers the last observed line.
        old = index(self.previous, lambda e: fields(e, "session_id", "event_type")
                    if claude(e, "hook", ("session.start", "session.end")) else None)
        for local in self.current:
            if local["event_id"] in self.aliases or not claude(local, "log", ("session.start", "session.end")):
                continue
            candidates = old.get(fields(local, "session_id", "event_type"), [])
            if candidates:
                if local["event_type"] == "session.start":
                    target = min(candidates, key=lambda e: e["ts"])
                else:
                    target = max(candidates, key=lambda e: e["ts"])
                    if target["ts"] < local["ts"]:
                        continue
                self.pair(local, target, "transcript boundary covered by live session")

        previous = index(self.previous, lambda e: e["session_id"] if claude(e, "otel", ("turn",)) else None)
        current = index(self.current, lambda e: e["session_id"] if claude(e, "log", ("turn",))
                        and e["event_id"] not in self.aliases else None)
        for session in current.keys() & previous.keys():
            left, right = current[session], sorted(previous[session], key=lambda e: e["ts"])
            last_position = -1
            for local in sorted(left, key=lambda e: e["ts"]):
                matches = [(i, e) for i, e in enumerate(right) if abs(e["ts"] - local["ts"]) <= 100]
                if not matches:
                    continue
                i, target = matches[0]
                if (not match_prompt_times or len(matches) != 1 or i <= last_position
                        or sum(abs(e["ts"] - target["ts"]) <= 100 for e in left) != 1):
                    raise ValueError(f"Unresolved Claude turn in session {session}")
                self.pair(local, target, "ordered unique Claude turn within 100ms")
                last_position = i


def load(path):
    rows = json.loads(path.read_text())
    if not isinstance(rows, list) or len({r["event_id"] for r in rows}) != len(rows):
        raise ValueError(f"Expected unique event IDs in {path}")
    return rows


def install(plan, database, identity, duckdb, current_host):
    # DuckDB takes its normal exclusive lock. The collector must be stopped.
    # The production account and the destination (which includes this host)
    # were captured from the authenticated sync response and device grant.
    scope, destination = identity["scope"], identity["destination"]
    origin, account = json.loads(scope)
    endpoint, org, email, kind, host = json.loads(destination)
    if not account or not org or not email or not kind or endpoint != origin or host != current_host:
        raise ValueError("Destination does not match the recovered ledger")
    if not database.is_file():
        raise ValueError("Expected an existing ledger")

    def literal(value):
        return "'" + value.replace("'", "''") + "'"

    sql = f"""
    BEGIN;
    CREATE TEMP TABLE recovery AS SELECT local_id,remote_id
      FROM read_json_auto({literal(str(plan.resolve()))});
    SELECT CASE WHEN EXISTS (
      SELECT 1 FROM recovery r LEFT JOIN observations o
        ON o.scope='local' AND o.event_id=r.local_id
      WHERE o.event_id IS NULL OR o.payload->>'host_id' <> {literal(host)}
    ) THEN error('Plan is not from this local ledger') END;
    SELECT CASE WHEN EXISTS (
      SELECT 1 FROM recovery r JOIN device_event_aliases a ON a.local_id=r.local_id
      WHERE a.scope={literal(scope)} AND r.remote_id<>a.remote_id
    ) THEN error('Conflicting previous reconciliation') END;
    INSERT INTO device_event_aliases
      SELECT {literal(scope)},{literal(destination)},local_id,remote_id FROM recovery
      ON CONFLICT(scope,destination,local_id) DO NOTHING;
    COMMIT;
    """
    subprocess.run([duckdb, "-batch", "-bail", str(database)], input=sql,
                   text=True, stdout=subprocess.DEVNULL, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("local", "previous", "cloud", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--match-prompt-times", action="store_true")
    parser.add_argument("--apply-to", type=Path, help="stopped collector's DuckDB ledger")
    parser.add_argument("--identity", type=Path, help="authenticated scope and destination JSON")
    parser.add_argument("--duckdb", default="duckdb")
    args = parser.parse_args()
    if args.apply_to and not args.identity:
        parser.error("--apply-to requires --identity")
    current, previous, cloud = map(load, (args.local, args.previous, args.cloud))
    current_hosts, old_hosts = ({e["host_id"] for e in items} for items in (current, previous))
    if len(current_hosts) != 1 or len(old_hosts) != 1 or current_hosts == old_hosts:
        raise ValueError("Expected the two local ledgers of one Mac with distinct host IDs")

    cloud_by_id = {e["event_id"]: e for e in cloud}
    verified = []
    masked = {"user_id", "user_id_source", "org_id", "prompt_text", "tool_input_json",
              "tool_output_excerpt", "redaction_applied"}
    for e in previous:
        remote = cloud_by_id.get(e["event_id"])
        if remote is None:
            raise ValueError("Previous history is not fully present in the Cloud baseline")
        # DuckDB and the server can round a serialized double one ULP apart.
        cost, cloud_cost = e.get("cost_usd"), remote.get("cost_usd")
        cost_matches = cost == cloud_cost or (cost is not None and cloud_cost is not None
                          and abs(cost - cloud_cost) <= math.ulp(cost))
        if not cost_matches or any(e.get(k) != remote.get(k)
                                   for k in (e.keys() | remote.keys()) - masked - {"cost_usd"}):
            raise ValueError(f"Cloud and backup differ for {e['event_id']}")
        verified.append(e)
    planner = Recovery(current, verified)
    planner.run(args.match_prompt_times)
    if planner.aliases.keys() & cloud_by_id.keys():
        raise ValueError("Both counterparts already exist in Cloud; requires server-side repair")
    args.output.write_text(json.dumps(list(planner.aliases.values())))
    if args.apply_to:
        identity = json.loads(args.identity.read_text())
        _, account = json.loads(identity["scope"])
        if not cloud or any(row.get("user_id") != account for row in cloud):
            raise ValueError("Cloud snapshot does not belong to the target account")
        install(args.output, args.apply_to, identity,
                args.duckdb, next(iter(current_hosts)))
    print("Matched by rule:", dict(Counter(a["basis"] for a in planner.aliases.values())))
    print("Represented by previous IDs:", len(planner.aliases),
          "current IDs retained separately:", len(current) - len(planner.aliases))


if __name__ == "__main__":
    main()
