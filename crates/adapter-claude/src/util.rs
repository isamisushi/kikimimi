//! JSON / OTLP AnyValue の値取り出しヘルパー。
//! パニックしない (欠損・型不一致は None を返す。推定で埋めない — 原則 7)。

use opentelemetry_proto::tonic::common::v1::{
    any_value::Value as AnyValueEnum, AnyValue, KeyValue,
};

/// 現在時刻を UNIX epoch ミリ秒で返す (システム時計が UNIX_EPOCH より前でも panic しない)。
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// serde_json::Value の数値フィールドを i64 として読む (整数・小数どちらの JSON number も許容)。
pub(crate) fn as_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// hook payload 直下の "timestamp" フィールド (epoch ミリ秒。数値 or 数値文字列) を読む。
/// 無い・型が合わない場合は None (呼び出し側で now_ms() にフォールバックする)。
pub(crate) fn extract_hook_ts(raw: &serde_json::Value) -> Option<i64> {
    let v = raw.get("timestamp")?;
    if let Some(n) = as_i64(v) {
        return Some(n);
    }
    v.as_str()?.parse::<i64>().ok()
}

/// KeyValue のリストから key に一致する最初の値を返す。
pub(crate) fn find_kv<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a AnyValue> {
    attrs
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| kv.value.as_ref())
}

/// ログレコード属性を優先し、無ければ resource 属性を見る (§4.1: session.id / user.email / model)。
pub(crate) fn find_attr<'a>(
    record_attrs: &'a [KeyValue],
    resource_attrs: &'a [KeyValue],
    key: &str,
) -> Option<&'a AnyValue> {
    find_kv(record_attrs, key).or_else(|| find_kv(resource_attrs, key))
}

/// AnyValue may be string OR int (エクスポータ実装により intValue が文字列 "150" のことがある) — 両方を文字列として読む。
pub(crate) fn av_as_str(v: &AnyValue) -> Option<String> {
    match v.value.as_ref()? {
        AnyValueEnum::StringValue(s) => Some(s.clone()),
        AnyValueEnum::IntValue(i) => Some(i.to_string()),
        AnyValueEnum::DoubleValue(d) => Some(d.to_string()),
        AnyValueEnum::BoolValue(b) => Some(b.to_string()),
        _ => None,
    }
}

/// AnyValue may be string OR int — 両方を i64 として読む。
pub(crate) fn av_as_i64(v: &AnyValue) -> Option<i64> {
    match v.value.as_ref()? {
        AnyValueEnum::IntValue(i) => Some(*i),
        AnyValueEnum::DoubleValue(d) => Some(*d as i64),
        AnyValueEnum::StringValue(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// AnyValue may be string OR number — 両方を f64 として読む (cost_usd 等)。
pub(crate) fn av_as_f64(v: &AnyValue) -> Option<f64> {
    match v.value.as_ref()? {
        AnyValueEnum::DoubleValue(d) => Some(*d),
        AnyValueEnum::IntValue(i) => Some(*i as f64),
        AnyValueEnum::StringValue(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// AnyValue may be bool OR string ("true"/"false") — 両方を bool として読む。
pub(crate) fn av_as_bool(v: &AnyValue) -> Option<bool> {
    match v.value.as_ref()? {
        AnyValueEnum::BoolValue(b) => Some(*b),
        AnyValueEnum::StringValue(s) => s.trim().parse().ok(),
        AnyValueEnum::IntValue(i) => Some(*i != 0),
        _ => None,
    }
}

/// Claude Code transcript の "timestamp" (ISO-8601 UTC 文字列、例
/// "2026-08-28T13:51:32.471Z") を UNIX epoch ミリ秒に変換する。
///
/// `kikimimi-adapter-claude` は chrono に依存していない (hook / OTLP どちらも
/// epoch ミリ秒 or OTel の time_unix_nano しか扱わないため) — 本クレートに
/// chrono を新規追加せずに済むよう、暦計算 (Howard Hinnant の
/// `days_from_civil`) を手書きする。年月日の区切り位置・小数秒・"Z" または
/// "+HH:MM"/"-HH:MM" オフセットを許容し、想定外の形は None を返す
/// (fail-open。呼び出し側で now_ms() にフォールバックする — 原則7)。
pub(crate) fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();
    let seps_ok = b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b'T' || b[10] == b' ')
        && b[13] == b':'
        && b[16] == b':';
    if !seps_ok {
        return None;
    }

    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut rest = &s[19..];
    let mut millis: i64 = 0;
    if let Some(frac_and_tail) = rest.strip_prefix('.') {
        let digit_len = frac_and_tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(frac_and_tail.len());
        let (frac, tail) = frac_and_tail.split_at(digit_len);
        if frac.is_empty() {
            return None;
        }
        let mut frac3 = frac.to_string();
        frac3.truncate(3);
        while frac3.len() < 3 {
            frac3.push('0');
        }
        millis = frac3.parse().ok()?;
        rest = tail;
    }

    let offset_ms: i64 = if rest.is_empty() || rest == "Z" {
        0
    } else if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) {
        let sign: i64 = if rest.starts_with('-') { -1 } else { 1 };
        let oh: i64 = rest.get(1..3)?.parse().ok()?;
        let om: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (oh * 3_600_000 + om * 60_000)
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day);
    let ms = days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis;
    Some(ms - offset_ms)
}

/// 1970-01-01 からの日数 (Howard Hinnant "chrono-Compatible Low-Level Date
/// Algorithms", `days_from_civil`)。プロレプティック・グレゴリオ暦、外部依存なし。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // [0, 11] : Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value as V;

    fn av(v: V) -> AnyValue {
        AnyValue { value: Some(v) }
    }

    #[test]
    fn av_as_i64_handles_string_and_int() {
        assert_eq!(av_as_i64(&av(V::IntValue(42))), Some(42));
        assert_eq!(av_as_i64(&av(V::StringValue("42".into()))), Some(42));
        assert_eq!(av_as_i64(&av(V::StringValue("nope".into()))), None);
    }

    #[test]
    fn parse_iso8601_ms_matches_known_epoch_values() {
        // Cross-checked against Python's datetime (UTC) for these values.
        assert_eq!(
            parse_iso8601_ms("2026-08-28T13:51:32.471Z"),
            Some(1_787_925_092_471)
        );
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso8601_ms("2000-03-01T00:00:00.000Z"),
            Some(951_868_800_000)
        );
    }

    #[test]
    fn parse_iso8601_ms_computes_correct_millisecond_deltas() {
        let a = parse_iso8601_ms("2026-09-02T10:00:05.000Z").unwrap();
        let b = parse_iso8601_ms("2026-09-02T10:00:07.500Z").unwrap();
        assert_eq!(b - a, 2500);
    }

    #[test]
    fn parse_iso8601_ms_accepts_numeric_utc_offset() {
        // "+00:00" must be equivalent to "Z".
        let z = parse_iso8601_ms("2026-08-28T13:51:32.471Z").unwrap();
        let offset = parse_iso8601_ms("2026-08-28T13:51:32.471+00:00").unwrap();
        assert_eq!(z, offset);
    }

    #[test]
    fn parse_iso8601_ms_none_on_garbage() {
        assert_eq!(parse_iso8601_ms("not a timestamp"), None);
        assert_eq!(parse_iso8601_ms(""), None);
        assert_eq!(parse_iso8601_ms("2026-13-01T00:00:00Z"), None); // month 13
    }

    #[test]
    fn find_attr_prefers_record_over_resource() {
        let record = vec![KeyValue {
            key: "model".into(),
            value: Some(av(V::StringValue("record-model".into()))),
            ..Default::default()
        }];
        let resource = vec![KeyValue {
            key: "model".into(),
            value: Some(av(V::StringValue("resource-model".into()))),
            ..Default::default()
        }];
        let found = find_attr(&record, &resource, "model").and_then(av_as_str);
        assert_eq!(found, Some("record-model".to_string()));
    }
}
