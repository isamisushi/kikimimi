"""Lifecycle probe. No network, transcript reads, prompts, or credential access."""
import datetime
import json
import os
from pathlib import Path
import sys


def main():
    raw = sys.stdin.buffer.read(1048577)
    if len(raw) > 1048576:
        raise ValueError("oversized hook input")
    payload = json.loads(raw)
    if not isinstance(payload, dict):
        raise ValueError("invalid hook input")
    record = {
        "probe_version": 1,
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "transcript_path_present": bool(payload.get("transcript_path")),
        "usage_field_present": any(k in payload for k in ("usage", "token_usage")),
    }
    for field in ("hook_event_name", "session_id", "turn_id", "model"):
        value = payload.get(field)
        if isinstance(value, str):
            record[field] = value[:256]
    # Use an isolated HOME for script-level tests; real host invocations use the
    # user's own local diagnostic log. Never read the transcript at this stage.
    directory = Path.home() / ".kikimimi" / "diagnostics"
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(directory / "chatgpt-hook-probe.jsonl",
                 os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    with os.fdopen(fd, "w") as stream:
        stream.write(json.dumps(record) + "\n")
    print(json.dumps({"systemMessage": "KIKIMIMI_HOOK_PROBE: " +
                     record.get("hook_event_name", "unknown")}))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Do not echo host input or private filesystem paths on failures.
        print(json.dumps({"systemMessage": "KIKIMIMI_HOOK_PROBE: diagnostic write failed"}))
