//! Claude Code OTel logs export (`claude_code.*` イベント) → kikimimi_schema::Event (architecture.md §4.1)。
//! Stage 0: metrics は未対応 (トークンは logs 経由で取れるため。TODO Stage 1: token.usage 等の直接集計)。

use kikimimi_schema::{dt_of, event_id, event_type, Event};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::logs::v1::LogRecord;

use crate::classify::classify_tool;
use crate::util::{av_as_bool, av_as_f64, av_as_i64, av_as_str, find_attr, find_kv, now_ms};
use crate::Normalizer;

impl Normalizer {
    /// OTLP ExportLogsServiceRequest を正規化する。`claude_code.*` 以外・未対応のイベント名は
    /// スキップして `skipped()` を進める。
    pub fn otlp_logs(&mut self, req: &ExportLogsServiceRequest) -> anyhow::Result<Vec<Event>> {
        let mut out = Vec::new();
        for rl in &req.resource_logs {
            let resource_attrs: &[KeyValue] = rl
                .resource
                .as_ref()
                .map(|r| r.attributes.as_slice())
                .unwrap_or(&[]);
            for sl in &rl.scope_logs {
                for lr in &sl.log_records {
                    match self.map_log_record(lr, resource_attrs) {
                        Ok(ev) => out.push(ev),
                        Err(reason) => self.mark_skipped(&reason),
                    }
                }
            }
        }
        Ok(out)
    }

    /// Stage 0: メトリクスは未対応、常に空を返す。
    /// トークン/コストは OTel logs (`claude_code.api_request`) 経由で取得する。
    pub fn otlp_metrics(
        &mut self,
        _req: &ExportMetricsServiceRequest,
    ) -> anyhow::Result<Vec<Event>> {
        Ok(vec![])
    }

    /// ログレコードを Event に正規化する。未対応のレコードは、"otlp:" プレフィックス付きの
    /// レコード識別子 (event_name が無ければ "otlp:no_event_name") を理由として `Err` で返す
    /// (呼び出し側 `otlp_logs` がこれを `mark_skipped` に渡す)。
    fn map_log_record(&mut self, lr: &LogRecord, resource_attrs: &[KeyValue]) -> Result<Event, String> {
        let name = record_event_name(lr).ok_or_else(|| "otlp:no_event_name".to_string())?;
        // 実測 (2026-08-31, claude 2.1.251 対話セッション): event 名は "api_request" のように
        // "claude_code." プレフィックス無しで届く。ドキュメント上の "claude_code.api_request"
        // 形式も受ける (両対応)。
        let short = name.strip_prefix("claude_code.").unwrap_or(name.as_str());
        let event_type_str: &str = match short {
            "api_request" => event_type::API_REQUEST,
            "api_error" => event_type::API_ERROR,
            "tool_result" => event_type::TOOL_RESULT,
            "tool_decision" => event_type::HOOK_DECISION,
            "user_prompt" => event_type::TURN,
            // hook-telemetry-daemon.md line 36: one of the ≥25 documented OTel log events.
            // Stage 1's context_bloat pattern ("compaction 連発", architecture.md §7.2) needs
            // this raw data, so it must not be silently dropped as unmapped.
            "compaction" => event_type::COMPACTION,
            _ => return Err(format!("otlp:{name}")),
        };

        let attrs = &lr.attributes;
        let session_id = find_attr(attrs, resource_attrs, "session.id").and_then(av_as_str);
        let (user_id, user_id_source) =
            match find_attr(attrs, resource_attrs, "user.email").and_then(av_as_str) {
                Some(email) => (Some(email), Some("agent_email".to_string())),
                None => (None, None),
            };
        let org_id = find_attr(attrs, resource_attrs, "organization.id").and_then(av_as_str);
        let model = find_attr(attrs, resource_attrs, "model").and_then(av_as_str);
        let effort = find_attr(attrs, resource_attrs, "effort").and_then(av_as_str);
        let tool_use_id = find_attr(attrs, resource_attrs, "tool_use_id").and_then(av_as_str);

        let primary_key = self.primary_key(tool_use_id.as_deref(), session_id.as_deref());
        let eid = event_id(&self.host_id, "otel", event_type_str, &primary_key);
        // exact | fuzzy | none (architecture.md §5.1) — always explicit, never NULL.
        let correlation_confidence = Some(if tool_use_id.is_some() {
            "exact".to_string()
        } else {
            "none".to_string()
        });

        let ts_ns = if lr.time_unix_nano != 0 {
            lr.time_unix_nano
        } else {
            lr.observed_time_unix_nano
        };
        let ts = if ts_ns != 0 {
            (ts_ns / 1_000_000) as i64
        } else {
            now_ms()
        };
        let dt = dt_of(ts);

        let mut ev = Event {
            event_id: eid,
            ts,
            dt,
            org_id,
            user_id,
            user_id_source,
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "otel".to_string(),
            session_id,
            correlation_key: tool_use_id,
            correlation_confidence,
            event_type: event_type_str.to_string(),
            model,
            effort,
            ..Default::default()
        };

        match event_type_str {
            event_type::API_REQUEST => {
                ev.input_tokens =
                    find_attr(attrs, resource_attrs, "input_tokens").and_then(av_as_i64);
                ev.output_tokens =
                    find_attr(attrs, resource_attrs, "output_tokens").and_then(av_as_i64);
                ev.cache_read_tokens =
                    find_attr(attrs, resource_attrs, "cache_read_tokens").and_then(av_as_i64);
                ev.cache_write_tokens =
                    find_attr(attrs, resource_attrs, "cache_creation_tokens").and_then(av_as_i64);
                ev.cost_usd = find_attr(attrs, resource_attrs, "cost_usd")
                    .or_else(|| find_attr(attrs, resource_attrs, "cost"))
                    .and_then(av_as_f64);
                ev.duration_ms =
                    find_attr(attrs, resource_attrs, "duration_ms").and_then(av_as_i64);
                ev.usage_source = Some("otel".to_string());
            }
            event_type::API_ERROR => {
                ev.error_type = find_attr(attrs, resource_attrs, "error")
                    .or_else(|| find_attr(attrs, resource_attrs, "error.type"))
                    .or_else(|| find_attr(attrs, resource_attrs, "message"))
                    .and_then(av_as_str);
                ev.duration_ms =
                    find_attr(attrs, resource_attrs, "duration_ms").and_then(av_as_i64);
            }
            event_type::TOOL_RESULT => {
                let tool_name = find_attr(attrs, resource_attrs, "tool_name").and_then(av_as_str);
                if let Some(tn) = &tool_name {
                    let class = classify_tool(tn);
                    ev.tool_kind = Some(class.kind.to_string());
                    ev.mcp_server = class.mcp_server;
                    ev.mcp_tool = class.mcp_tool;
                }
                ev.tool_name = tool_name;
                ev.success = find_attr(attrs, resource_attrs, "success").and_then(av_as_bool);
                ev.duration_ms =
                    find_attr(attrs, resource_attrs, "duration_ms").and_then(av_as_i64);
            }
            event_type::HOOK_DECISION => {
                ev.decision = find_attr(attrs, resource_attrs, "decision").and_then(av_as_str);
                ev.decision_source = find_attr(attrs, resource_attrs, "decision_source")
                    .or_else(|| find_attr(attrs, resource_attrs, "source"))
                    .and_then(av_as_str);
                ev.tool_name = find_attr(attrs, resource_attrs, "tool_name").and_then(av_as_str);
            }
            _ => {}
        }

        Ok(ev)
    }
}

/// ログレコードのイベント名を特定する: `event_name` フィールド → `event.name` 属性 →
/// (フォールバック) `claude_code.` で始まる body 文字列、の順に見る。
fn record_event_name(lr: &LogRecord) -> Option<String> {
    if !lr.event_name.is_empty() {
        return Some(lr.event_name.clone());
    }
    if let Some(s) = find_kv(&lr.attributes, "event.name").and_then(av_as_str) {
        return Some(s);
    }
    let body = lr.body.as_ref()?;
    let s = av_as_str(body)?;
    s.starts_with("claude_code.").then_some(s)
}
