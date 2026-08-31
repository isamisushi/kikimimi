//! 小さな値取り出しヘルパー。パニックしない (欠損・型不一致は None を返す。推定で埋めない — 原則 7)。
//! `kikimimi-adapter-claude::util` と同じ思想だが、OTLP `AnyValue` ではなく素の
//! `serde_json::Value` (hook JSON・rollout JSONL 共通) のみを相手にする分、より単純。

use serde_json::Value;

/// 現在時刻を UNIX epoch ミリ秒で返す (システム時計が UNIX_EPOCH より前でも panic しない)。
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// serde_json::Value の数値フィールドを i64 として読む (整数・小数どちらの JSON number も許容)。
pub(crate) fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// hook payload 直下の "timestamp" フィールド (epoch ミリ秒。数値 or 数値文字列) を読む。
/// Claude Code の hook payload と同じ形を試す (`kikimimi-adapter-claude::util::extract_hook_ts`
/// と同じロジック) — Codex の hook payload に無くても実害はない (呼び出し側で now_ms() に
/// フォールバックする)。
pub(crate) fn extract_hook_ts(raw: &Value) -> Option<i64> {
    let v = raw.get("timestamp")?;
    if let Some(n) = as_i64(v) {
        return Some(n);
    }
    v.as_str()?.parse::<i64>().ok()
}

/// RFC3339 文字列 (rollout JSONL の "timestamp" — 例 "2026-08-31T12:01:28.448Z") を
/// UNIX epoch ミリ秒に変換する。パースできなければ None (呼び出し側で now_ms() に
/// フォールバックする — 実測 (2026-08-31, codex-cli 0.151.0) では rollout の全行に
/// この形式の "timestamp" が付いているが、将来のバージョンでドリフトしても panic しない)。
pub(crate) fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_i64_handles_int_and_float() {
        assert_eq!(as_i64(&serde_json::json!(42)), Some(42));
        assert_eq!(as_i64(&serde_json::json!(42.9)), Some(42));
        assert_eq!(as_i64(&serde_json::json!("nope")), None);
    }

    #[test]
    fn extract_hook_ts_handles_number_and_string() {
        assert_eq!(
            extract_hook_ts(&serde_json::json!({"timestamp": 1700000000000_i64})),
            Some(1700000000000)
        );
        assert_eq!(
            extract_hook_ts(&serde_json::json!({"timestamp": "1700000000000"})),
            Some(1700000000000)
        );
        assert_eq!(extract_hook_ts(&serde_json::json!({})), None);
    }

    #[test]
    fn parse_rfc3339_ms_matches_real_rollout_timestamp_shape() {
        // Real shape observed 2026-08-31 on codex-cli 0.151.0 rollout JSONL.
        let ms = parse_rfc3339_ms("2026-08-31T12:01:28.448Z").unwrap();
        // Sanity: round-trips back to the same instant via chrono.
        let back = chrono::DateTime::from_timestamp_millis(ms).unwrap();
        assert_eq!(back.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(), "2026-08-31T12:01:28.448Z");
    }

    #[test]
    fn parse_rfc3339_ms_none_on_garbage() {
        assert_eq!(parse_rfc3339_ms("not a timestamp"), None);
    }
}
