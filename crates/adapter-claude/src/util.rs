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
