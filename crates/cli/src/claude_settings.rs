//! `~/.claude/settings.json` の読み書きヘルパー (architecture.md §4.2)。
//! `guru init` / `guru uninstall` / `guru status` の 3 コマンドが共有する。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::Value;

/// guru が書き込む hooks イベントとその timeout (秒)。§4.2 の設定例に準拠。
pub const HOOK_EVENTS: &[(&str, u64)] = &[
    ("PreToolUse", 5),
    ("PostToolUse", 5),
    ("PostToolUseFailure", 5),
    ("PermissionDenied", 5),
    ("SubagentStop", 5),
    ("SessionStart", 5),
    ("SessionEnd", 1),
];

/// guru が書き込む env キーと期待値。`OTEL_EXPORTER_OTLP_ENDPOINT` は OTLP ポートに依存する。
pub fn expected_env(otlp_port: u16) -> Vec<(&'static str, String)> {
    vec![
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "1".to_string()),
        ("OTEL_METRICS_EXPORTER", "otlp".to_string()),
        ("OTEL_LOGS_EXPORTER", "otlp".to_string()),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf".to_string()),
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://localhost:{otlp_port}"),
        ),
    ]
}

/// `~/.claude/settings.json` の場所。テスト/smoke 用に `GURU_CLAUDE_SETTINGS_PATH` で上書きできる
/// (本番の既定は常に `$HOME/.claude/settings.json` — architecture.md §4 の記載どおり)。
pub fn settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("GURU_CLAUDE_SETTINGS_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claude").join("settings.json")
}

/// `<settings>.guru-backup` — 初回変更前にだけ作る一度きりのバックアップ。
pub fn backup_path(settings: &Path) -> PathBuf {
    let mut s = settings.as_os_str().to_os_string();
    s.push(".guru-backup");
    PathBuf::from(s)
}

/// 存在すれば読んでパースし、無ければ空オブジェクトを返す。
pub fn load_settings(path: &Path) -> anyhow::Result<Value> {
    match fs::read_to_string(path) {
        Ok(s) => {
            let v: Value = serde_json::from_str(&s)
                .with_context(|| format!("parsing {} as JSON", path.display()))?;
            Ok(v)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Value::Object(Default::default())),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// tmp ファイル + rename で atomic に書く。
pub fn write_settings_atomic(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value).context("serializing settings.json")?;
    crate::state::write_atomic(path, &bytes)
}

/// hooks.<event> の中に command が "guru hook" で始まるエントリが既にあるか。
pub fn has_guru_hook(value: &Value, event: &str) -> bool {
    value
        .pointer(&format!("/hooks/{event}"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().any(entry_has_guru_hook))
        .unwrap_or(false)
}

/// 1 つの hooks エントリ (`{"hooks": [{"type":"command","command":...}]}`) が
/// guru の command を含むかどうか。
pub fn entry_has_guru_hook(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .map(|c| c.starts_with("guru hook"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// hooks.<event> の末尾に `{"hooks":[{"type":"command","command":"guru hook <event>","timeout":N}]}`
/// を追加する。既存の hooks.<event> 配列 (ユーザー自身の hook を含む) は順序も内容もそのまま残す。
pub fn add_hook_entry(value: &mut Value, event: &str, timeout: u64) -> anyhow::Result<()> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json root is not an object"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_obj = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("\"hooks\" is not an object in settings.json"))?;
    let arr = hooks_obj
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let arr = arr
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("\"hooks.{event}\" is not an array in settings.json"))?;
    arr.push(serde_json::json!({
        "hooks": [
            { "type": "command", "command": format!("guru hook {event}"), "timeout": timeout }
        ]
    }));
    Ok(())
}

/// env.<key> = val を設定する (呼び出し側が「上書きしてよい」と判断した後にのみ呼ぶ)。
pub fn set_env(value: &mut Value, key: &str, val: &str) -> anyhow::Result<()> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json root is not an object"))?;
    let env = root
        .entry("env")
        .or_insert_with(|| Value::Object(Default::default()));
    let env_obj = env
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("\"env\" is not an object in settings.json"))?;
    env_obj.insert(key.to_string(), Value::String(val.to_string()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_loads_as_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let v = load_settings(&path).unwrap();
        assert_eq!(v, Value::Object(Default::default()));
    }

    #[test]
    fn add_hook_entry_preserves_existing_user_hooks_and_order() {
        let mut v: Value = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "my-own-linter", "timeout": 3 } ] }
                ]
            }
        });
        add_hook_entry(&mut v, "PreToolUse", 5).unwrap();
        let arr = v.pointer("/hooks/PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr[0].pointer("/hooks/0/command").unwrap().as_str(),
            Some("my-own-linter")
        );
        assert_eq!(
            arr[1].pointer("/hooks/0/command").unwrap().as_str(),
            Some("guru hook PreToolUse")
        );
        assert!(has_guru_hook(&v, "PreToolUse"));
    }

    #[test]
    fn add_hook_entry_creates_missing_structure() {
        let mut v = Value::Object(Default::default());
        add_hook_entry(&mut v, "SessionEnd", 1).unwrap();
        assert!(has_guru_hook(&v, "SessionEnd"));
        assert_eq!(
            v.pointer("/hooks/SessionEnd/0/hooks/0/timeout")
                .unwrap()
                .as_u64(),
            Some(1)
        );
    }

    #[test]
    fn set_env_creates_missing_structure() {
        let mut v = Value::Object(Default::default());
        set_env(&mut v, "FOO", "bar").unwrap();
        assert_eq!(v.pointer("/env/FOO").unwrap().as_str(), Some("bar"));
    }

    #[test]
    fn write_settings_atomic_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let v = serde_json::json!({"a": 1});
        write_settings_atomic(&path, &v).unwrap();
        let loaded = load_settings(&path).unwrap();
        assert_eq!(loaded, v);
    }
}
