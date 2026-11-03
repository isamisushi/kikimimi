# Chat / Codex lifecycle probe

Temporary diagnostic plugin, not a collector. Registers SessionStart,
UserPromptSubmit and Stop. It writes only time, event, session/turn IDs, model,
and booleans indicating transcript/usage availability to
`~/.kikimimi/diagnostics/chatgpt-hook-probe.jsonl` in the execution environment.
No network, credentials, transcript reads or conversation content.

Install via the personal marketplace, enable, and review/trust the hooks in the
host UI. Start a fresh ordinary Chat and a fresh Codex conversation separately.
Match recorded session IDs to each test; Codex success does not prove Chat support.
Missing records are inconclusive until installation, hook trust and a working
Codex control are verified. Cloud execution may write to a different filesystem;
the `KIKIMIMI_HOOK_PROBE` system message is a second signal in that case.

Do not ask the model to run the script manually; that is not a hook test.
Remove the plugin after the experiment.
