//! Codex `[otel]` logs export → kikimimi_schema::Event.
//!
//! Observed 2026-11-03 (Codex.app 26.1027.x, engine 0.166.0-alpha.5.1): the CLI
//! (`codex_exec`), the desktop app (`Codex_Desktop`) and Work (`codex_work_desktop`)
//! all export the same records. Every attribute value is a string, `time_unix_nano`
//! is 0 and the real instant is the `event.timestamp` attribute, and the log
//! record's `event_name` is a source location, so the name comes from the
//! `event.name` attribute.
//!
//! Only `codex.sse_event` with `event.kind = response.completed` is normalized: it
//! carries one API call's usage. Turns and tools still come from the rollout,
//! whose identities the OTel records cannot reproduce. `codex.tool_result`
//! carries the tool arguments and output, which are never copied.

use kikimimi_schema::{dt_of, event_type, Event};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value as AnyValueEnum, KeyValue};
use opentelemetry_proto::tonic::logs::v1::LogRecord;

use crate::util::{now_ms, parse_rfc3339_ms};
use crate::{usage_event_id, CodexNormalizer, CodexUsage};

const PREFIX: &str = "codex.";

/// Split one export into (everything else, Codex records), keeping each record's
/// resource and scope. The Claude Code normalizer would count every Codex record
/// as an unsupported event otherwise.
pub fn split_codex_logs(
    req: ExportLogsServiceRequest,
) -> (ExportLogsServiceRequest, ExportLogsServiceRequest) {
    let mut rest = ExportLogsServiceRequest::default();
    let mut codex = ExportLogsServiceRequest::default();
    for mut resource in req.resource_logs {
        let mut ours = resource.clone();
        ours.scope_logs.clear();
        let scopes = std::mem::take(&mut resource.scope_logs);
        for mut scope in scopes {
            let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut scope.log_records)
                .into_iter()
                .partition(is_codex_record);
            if !mine.is_empty() {
                let mut copy = scope.clone();
                copy.log_records = mine;
                ours.scope_logs.push(copy);
            }
            if !others.is_empty() {
                scope.log_records = others;
                resource.scope_logs.push(scope);
            }
        }
        if !ours.scope_logs.is_empty() {
            codex.resource_logs.push(ours);
        }
        if !resource.scope_logs.is_empty() {
            rest.resource_logs.push(resource);
        }
    }
    (rest, codex)
}

fn is_codex_record(record: &LogRecord) -> bool {
    attr_str(&record.attributes, "event.name").is_some_and(|name| name.starts_with(PREFIX))
}

impl CodexNormalizer {
    /// Normalize the Codex records of an export (see [`split_codex_logs`]).
    pub fn otlp_logs(&mut self, req: &ExportLogsServiceRequest) -> Vec<Event> {
        let mut out = Vec::new();
        for resource in &req.resource_logs {
            for scope in &resource.scope_logs {
                for record in &scope.log_records {
                    match self.otlp_usage(record) {
                        Ok(event) => out.push(event),
                        Err(reason) => self.mark_skipped(&reason),
                    }
                }
            }
        }
        out
    }

    fn otlp_usage(&self, record: &LogRecord) -> Result<Event, String> {
        let attrs = &record.attributes;
        let name = attr_str(attrs, "event.name").unwrap_or_default();
        if name != "codex.sse_event" {
            return Err(format!("otlp:{name}"));
        }
        if attr_str(attrs, "event.kind") != Some("response.completed") {
            return Err("otlp:codex.sse_event:other_kind".into());
        }
        let count = |key| attr_i64(attrs, key);
        let (Some(input), Some(output)) = (count("input_token_count"), count("output_token_count"))
        else {
            return Err("otlp:codex.sse_event:no_usage".into());
        };
        let usage = CodexUsage {
            input,
            cached: count("cached_token_count").unwrap_or(0),
            cache_write: count("cache_write_token_count").unwrap_or(0),
            output,
            reasoning: count("reasoning_token_count").unwrap_or(0),
        };
        if usage.is_prewarm() {
            return Err("otlp:codex.sse_event:prewarm".into());
        }
        let session = attr_str(attrs, "conversation.id")
            .filter(|s| !s.is_empty())
            .ok_or("otlp:codex.sse_event:no_conversation")?;
        let ts = attr_str(attrs, "event.timestamp")
            .and_then(parse_rfc3339_ms)
            .or_else(|| nanos_to_ms(record.time_unix_nano))
            .or_else(|| nanos_to_ms(record.observed_time_unix_nano))
            .unwrap_or_else(now_ms);
        let text = |key| attr_str(attrs, key).map(str::to_owned);
        let email = text("user.email").filter(|s| !s.is_empty());
        Ok(Event {
            event_id: usage_event_id(&self.host_id, session, &usage),
            ts,
            dt: dt_of(ts),
            user_id_source: email.as_ref().map(|_| "agent_email".to_string()),
            user_id: email,
            host_id: self.host_id.clone(),
            agent: "codex".into(),
            agent_version: text("app.version"),
            client: attr_str(attrs, "originator")
                .and_then(kikimimi_schema::codex_client)
                .map(str::to_owned),
            source: "otel".into(),
            session_id: Some(session.to_owned()),
            model: text("model"),
            effort: text("model_reasoning_effort"),
            input_tokens: Some(usage.input),
            output_tokens: Some(usage.output),
            cache_read_tokens: Some(usage.cached),
            cache_write_tokens: Some(usage.cache_write),
            reasoning_tokens: Some(usage.reasoning),
            usage_source: Some("otel".into()),
            correlation_confidence: Some("none".into()),
            event_type: event_type::API_REQUEST.into(),
            ..Default::default()
        })
    }
}

fn nanos_to_ms(nanos: u64) -> Option<i64> {
    (nanos != 0).then_some((nanos / 1_000_000) as i64)
}

fn attr<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a AnyValueEnum> {
    attrs
        .iter()
        .find(|kv| kv.key == key)?
        .value
        .as_ref()?
        .value
        .as_ref()
}

fn attr_str<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a str> {
    match attr(attrs, key)? {
        AnyValueEnum::StringValue(s) => Some(s),
        _ => None,
    }
}

/// Codex sends counts as strings; accept real integers too.
fn attr_i64(attrs: &[KeyValue], key: &str) -> Option<i64> {
    match attr(attrs, key)? {
        AnyValueEnum::StringValue(s) => s.parse().ok(),
        AnyValueEnum::IntValue(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::AnyValue;
    use opentelemetry_proto::tonic::logs::v1::{ResourceLogs, ScopeLogs};

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(AnyValueEnum::StringValue(value.into())),
            }),
            ..Default::default()
        }
    }

    /// The shape Codex.app exported on 2026-11-03 (values abbreviated).
    fn completed(input: &str, output: &str) -> LogRecord {
        LogRecord {
            event_name: "event otel/src/events/session_telemetry.rs:1132".into(),
            observed_time_unix_nano: 1_793_693_195_563_983_000,
            attributes: vec![
                kv("event.name", "codex.sse_event"),
                kv("event.kind", "response.completed"),
                kv("input_token_count", input),
                kv("output_token_count", output),
                kv("cached_token_count", "12800"),
                kv("cache_write_token_count", "0"),
                kv("reasoning_token_count", "0"),
                kv("event.timestamp", "2026-11-03T08:16:59.000Z"),
                kv("conversation.id", "01a1a07b-8793-7333-864b-62cc2a73dc23"),
                kv("app.version", "26.1027.51404"),
                kv("originator", "Codex_Desktop"),
                kv("user.email", "dev@example.com"),
                kv("model", "gpt-6-astra"),
            ],
            ..Default::default()
        }
    }

    fn request(records: Vec<LogRecord>) -> ExportLogsServiceRequest {
        ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: records,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    #[test]
    fn a_completed_response_becomes_one_usage_row_with_the_shared_identity() {
        let mut n = CodexNormalizer::new("host".into());
        let events = n.otlp_logs(&request(vec![completed("23436", "35")]));
        assert_eq!(events.len(), 1);
        let e = &events[0];
        let usage = CodexUsage {
            input: 23436,
            cached: 12800,
            cache_write: 0,
            output: 35,
            reasoning: 0,
        };
        assert_eq!(
            e.event_id,
            usage_event_id("host", "01a1a07b-8793-7333-864b-62cc2a73dc23", &usage)
        );
        assert_eq!(e.ts, parse_rfc3339_ms("2026-11-03T08:16:59.000Z").unwrap());
        assert_eq!(e.client.as_deref(), Some("codex_desktop"));
        assert_eq!((e.input_tokens, e.output_tokens), (Some(23436), Some(35)));
        assert_eq!(e.cache_read_tokens, Some(12800));
        assert_eq!(e.usage_source.as_deref(), Some("otel"));
        assert_eq!(e.event_type, event_type::API_REQUEST);
    }

    #[test]
    fn warm_ups_and_other_events_are_skipped_without_content() {
        let mut n = CodexNormalizer::new("host".into());
        let mut prewarm = completed("2", "0");
        prewarm.attributes[4] = kv("cached_token_count", "0");
        let mut tool = completed("1", "1");
        tool.attributes = vec![
            kv("event.name", "codex.tool_result"),
            kv("output", "SECRET"),
        ];
        assert!(n
            .otlp_logs(&request(vec![prewarm, tool]))
            .is_empty());
        assert_eq!(n.skipped_by_reason()["otlp:codex.sse_event:prewarm"], 1);
        assert_eq!(n.skipped_by_reason()["otlp:codex.tool_result"], 1);
    }

    #[test]
    fn split_separates_codex_records_and_keeps_the_rest_intact() {
        let claude = LogRecord {
            event_name: "api_request".into(),
            ..Default::default()
        };
        let (rest, codex) = split_codex_logs(request(vec![
            claude.clone(),
            completed("23436", "35"),
        ]));
        assert_eq!(rest, request(vec![claude]));
        assert_eq!(codex, request(vec![completed("23436", "35")]));
        let (rest, codex) = split_codex_logs(request(vec![completed("23436", "35")]));
        assert!(rest.resource_logs.is_empty());
        assert_eq!(codex.resource_logs.len(), 1);
    }
}
