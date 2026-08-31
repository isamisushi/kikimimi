//! `~/.codex/sessions/**/rollout-*.jsonl` の 1 行 → kikimimi_schema::Event
//! (architecture.md §4 「ログ tailer」, §4.1 Codex 行)。
//!
//! # 実データの形 (2026-08-31 実測, codex-cli 0.151.0, Linux) — crate ルートの doc comment も参照
//!
//! 各行: `{"timestamp": <RFC3339>, "ordinal": <u64>, "type": <envelope 種別>, "payload": {...}}`。
//! `type` (envelope 種別) は次のいずれかを実際に観測した:
//!
//! - `session_meta` (ファイル先頭 1 行のみ): `payload` に `session_id`/`id`, `cwd`,
//!   `originator`, `cli_version`, `source`, `thread_source`, `model_provider`,
//!   `base_instructions`, `history_mode`, `context_window`, `git.{commit_hash,branch,
//!   repository_url}`。
//! - `turn_context`: `payload` に `turn_id`, `cwd`, `model` (!), `sandbox_policy` 等
//!   ターンごとの設定スナップショット。**`model` はここにしか出てこない** —
//!   `token_count`/`task_started`/`task_complete` 自体には無い。
//! - `world_state`: セッション全体のワールドステート・スナップショット (今回のタスクの
//!   スコープでは使わない)。
//! - `event_msg`: `payload.type` でさらに分岐する。実際に観測したのは
//!   `task_started` (`turn_id`, `started_at`), `task_complete` (`turn_id`,
//!   `duration_ms`, `time_to_first_token_ms`, `last_agent_message` — 本文なので読まない),
//!   `token_count` (`info.last_token_usage.{input_tokens,cached_input_tokens,
//!   cache_write_input_tokens,output_tokens,reasoning_output_tokens}`, `rate_limits`),
//!   `item_completed` (`thread_id`, `turn_id`, `item`, `started_at_ms`, `completed_at_ms` —
//!   `item.type` でさらに分岐: `UserMessage`/`AgentMessage`/`Reasoning` は本文なので
//!   読まない。**`CommandExecution`** は `command`/`cwd`/`exit_code`/`duration
//!   {secs,nanos}`/`stdout`/`stderr` を持つ、シェル実行の完了済み 1 レコード)。
//! - `response_item`: `payload.type` が `message`/`custom_tool_call`/
//!   `custom_tool_call_output`/`reasoning`。`custom_tool_call` (`name`, `call_id`,
//!   `input`) と `custom_tool_call_output` (`call_id`, `output`) は `call_id` で厳密に
//!   対応するが、構造化された `duration`/`exit_code` を持たない (モデルへの/からの
//!   生のツール呼び出しテキスト)。**同じシェル実行が `event_msg.item_completed` の
//!   `CommandExecution` としても (構造化された形で) 記録されるため、二重計上を避けて
//!   `CommandExecution` 側だけを採用する** (`skip_by_reason` で
//!   `rollout:response_item:custom_tool_call` 等として可視化はする)。
//!
//! `research/hook-telemetry-daemon.md` や旧 architecture.md が述べる
//! `ExecCommandBegin`/`ExecCommandEnd`/`McpToolCallBegin`/`McpToolCallEnd`/
//! `PatchApplyBegin`/`PatchApplyEnd`/`TokenCount` という **トップレベルのフラットな
//! rollout エントリ種別は、このインストール済みバージョンのディスク上の rollout JSONL
//! には存在しない** (stale)。同名の文字列 (`exec_command_begin` 等) はバイナリ内には
//! 実在するが、`[otel]` エクスポート/アプリサーバー向けイベント名の語彙であり、
//! rollout ファイルの内容ではない。
//!
//! # 未確認 (Stage 0 の既知の欠損 — 原則 7: 実データが無いものは実装しない)
//!
//! このマシンでは MCP サーバーが 1 つも設定されておらず (`codex doctor`: "0 MCP
//! servers")、サンプルセッション中にファイル編集 (`apply_patch`) も発生しなかったため、
//! `item.type == "McpToolCall"` 相当・`item.type == "FileChange"`/`"PatchApply"` 相当の
//! 実データを採取できていない。これらの `item.type` は `skip_by_reason` で
//! `rollout:item:<type>` として可視化されるだけで、実データ (フィールド名) が確認できる
//! まで意図的に未実装のままにする (誤ったフィールド名を当て推量しない)。

use kikimimi_schema::{cwd_hash, dt_of, event_id, event_type, Event};
use serde_json::Value;

use crate::util::{as_i64, now_ms, parse_rfc3339_ms};
use crate::CodexNormalizer;

/// rollout ファイル 1 つ (= Codex セッション 1 つ) につき、tailer 側が 1 つ保持する
/// 行またぎの文脈。`rollout_line` の呼び出し間で使い回す (mutable)。
///
/// 1 rollout ファイル = 1 Codex セッションという実データ上の不変条件があるため、
/// (`kikimimi-adapter-claude::Normalizer` が同時に多数の Claude Code セッションを
/// 相手にするのとは違い) ここではセッションをまたいだマップを持つ必要がない —
/// tailer 側がファイルごとに 1 つ `RolloutSessionCtx::default()` を保持すればよい。
///
/// daemon 再起動でメモリ上のこの構造体は失われる。`rollout_line` はそれでも panic
/// せず「持ち回れなかった分は None のまま」で動く (原則 7: 推定で埋めない) — tailer
/// 側は再開時に該当ファイルの 1 行目 (常に `session_meta`) を読み直すことで
/// `session_id`/`cwd_hash`/`agent_version`/`provider`/`repo` を復元することを推奨する
/// (`crates/cli` 側の codex tailer 実装を参照。`current_turn_id`/`current_model` は
/// 次の `turn_context`/`task_started` が来るまで一時的に失われるが、これも原則 7
/// どおり None のまま送るだけで実害はない)。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RolloutSessionCtx {
    pub session_id: Option<String>,
    pub cwd_hash: Option<String>,
    pub agent_version: Option<String>,
    pub provider: Option<String>,
    pub repo: Option<String>,
    /// 直近の `task_started`/`item_completed`/`turn_context` から見えている turn_id。
    pub current_turn_id: Option<String>,
    /// 直近の `turn_context` から見えているモデル名。`token_count` 等、行自体に
    /// model を持たないイベントを補うために使う (`turn_context` 到着前は None)。
    pub current_model: Option<String>,
}

impl CodexNormalizer {
    /// rollout JSONL の 1 行を正規化する。空行は `Ok(vec![])` (tail -f で読みかけの
    /// 最終行に遭遇した場合など)。JSON として壊れている行は `Err` を返す — 呼び出し側
    /// (tailer) が `agent.rs::drain_spool` の `malformed_spool` と同じ考え方で
    /// 件数を数える想定 (本クレートの `skipped_by_reason` には含めない。壊れた JSON は
    /// 「対応していない種別」ではなく「読めなかった」ため、Claude 側の spool と同じ層で
    /// 扱うのが一貫する)。
    pub fn rollout_line(
        &mut self,
        ctx: &mut RolloutSessionCtx,
        line: &str,
    ) -> anyhow::Result<Vec<Event>> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(vec![]);
        }
        let raw: Value = serde_json::from_str(line)?;

        let Some(env_type) = raw.get("type").and_then(Value::as_str) else {
            self.mark_skipped("rollout:no_type");
            return Ok(vec![]);
        };
        let ordinal = raw.get("ordinal").and_then(as_i64).unwrap_or(0);
        let ts = raw
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_ms)
            .unwrap_or_else(now_ms);
        let payload = raw.get("payload");

        Ok(match env_type {
            "session_meta" => self.handle_session_meta(ctx, payload, ordinal, ts),
            "turn_context" => {
                self.handle_turn_context(ctx, payload);
                self.mark_skipped("rollout:turn_context");
                vec![]
            }
            "world_state" => {
                self.mark_skipped("rollout:world_state");
                vec![]
            }
            "event_msg" => self.handle_event_msg(ctx, payload, ordinal, ts),
            "response_item" => {
                let sub = payload
                    .and_then(|p| p.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("no_type");
                self.mark_skipped(&format!("rollout:response_item:{sub}"));
                vec![]
            }
            other => {
                self.mark_skipped(&format!("rollout:{other}"));
                vec![]
            }
        })
    }

    /// `session_meta` (ファイル先頭 1 行) → `session.start` + `ctx` の初期化。
    fn handle_session_meta(
        &mut self,
        ctx: &mut RolloutSessionCtx,
        payload: Option<&Value>,
        ordinal: i64,
        ts_envelope: i64,
    ) -> Vec<Event> {
        let Some(p) = payload else {
            self.mark_skipped("rollout:session_meta:no_payload");
            return vec![];
        };

        let session_id = p
            .get("session_id")
            .or_else(|| p.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let cwd = p.get("cwd").and_then(Value::as_str).map(cwd_hash);
        let agent_version = p
            .get("cli_version")
            .and_then(Value::as_str)
            .map(str::to_string);
        let provider = p
            .get("model_provider")
            .and_then(Value::as_str)
            .map(str::to_string);
        let repo = p
            .get("git")
            .and_then(|g| g.get("repository_url"))
            .and_then(Value::as_str)
            .map(str::to_string);

        ctx.session_id = session_id.clone();
        ctx.cwd_hash = cwd.clone();
        ctx.agent_version = agent_version.clone();
        ctx.provider = provider.clone();
        ctx.repo = repo.clone();

        // Prefer the payload's own timestamp (the session's actual start instant) over
        // the envelope's (when the line was flushed), falling back to the envelope/now
        // if unparseable.
        let ts = p
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_ms)
            .unwrap_or(ts_envelope);

        let primary_key = self.rollout_primary_key(session_id.as_deref(), ordinal);
        let eid = event_id(&self.host_id, "log", event_type::SESSION_START, &primary_key);

        vec![Event {
            event_id: eid,
            ts,
            dt: dt_of(ts),
            host_id: self.host_id.clone(),
            agent: "codex".to_string(),
            source: "log".to_string(),
            session_id,
            cwd_hash: cwd,
            agent_version,
            provider,
            repo,
            // architecture.md §5.1: hook 行と log 行の相関は Stage 0/1 で実証するまで
            // NULL のまま — ここは他エージェントの hook とすら相関を試みていないので
            // 常に "none"。
            correlation_confidence: Some("none".to_string()),
            event_type: event_type::SESSION_START.to_string(),
            ..Default::default()
        }]
    }

    /// `turn_context` → `ctx` の更新のみ (行自体は Event を生まない)。
    fn handle_turn_context(&mut self, ctx: &mut RolloutSessionCtx, payload: Option<&Value>) {
        let Some(p) = payload else { return };
        if let Some(t) = p.get("turn_id").and_then(Value::as_str) {
            ctx.current_turn_id = Some(t.to_string());
        }
        if let Some(m) = p.get("model").and_then(Value::as_str) {
            ctx.current_model = Some(m.to_string());
        }
    }

    /// `event_msg` → `payload.type` で分岐。
    fn handle_event_msg(
        &mut self,
        ctx: &mut RolloutSessionCtx,
        payload: Option<&Value>,
        ordinal: i64,
        ts: i64,
    ) -> Vec<Event> {
        let Some(p) = payload else {
            self.mark_skipped("rollout:event_msg:no_payload");
            return vec![];
        };
        let Some(sub) = p.get("type").and_then(Value::as_str) else {
            self.mark_skipped("rollout:event_msg:no_type");
            return vec![];
        };

        match sub {
            "task_started" => {
                if let Some(t) = p.get("turn_id").and_then(Value::as_str) {
                    ctx.current_turn_id = Some(t.to_string());
                }
                vec![self.turn_event(ctx, p, ordinal, ts, None)]
            }
            "task_complete" => {
                if let Some(t) = p.get("turn_id").and_then(Value::as_str) {
                    ctx.current_turn_id = Some(t.to_string());
                }
                let duration_ms = p.get("duration_ms").and_then(as_i64);
                vec![self.turn_event(ctx, p, ordinal, ts, duration_ms)]
            }
            "token_count" => vec![self.token_count_event(ctx, p, ordinal, ts)],
            "item_completed" => self.handle_item_completed(ctx, p, ordinal),
            other => {
                self.mark_skipped(&format!("rollout:event_msg:{other}"));
                vec![]
            }
        }
    }

    /// `task_started`/`task_complete` → `turn` イベント。
    fn turn_event(
        &mut self,
        ctx: &RolloutSessionCtx,
        p: &Value,
        ordinal: i64,
        ts: i64,
        duration_ms: Option<i64>,
    ) -> Event {
        let turn_id = p
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| ctx.current_turn_id.clone());
        let primary_key = self.rollout_primary_key(ctx.session_id.as_deref(), ordinal);
        Event {
            event_id: event_id(&self.host_id, "log", event_type::TURN, &primary_key),
            ts,
            dt: dt_of(ts),
            host_id: self.host_id.clone(),
            agent: "codex".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            provider: ctx.provider.clone(),
            repo: ctx.repo.clone(),
            turn_id,
            model: ctx.current_model.clone(),
            duration_ms,
            correlation_confidence: Some("none".to_string()),
            event_type: event_type::TURN.to_string(),
            ..Default::default()
        }
    }

    /// `token_count` → `api.request` イベント。`info.last_token_usage` (この 1 回分の
    /// 増分) を使う。`total_token_usage` (セッション累計) は使わない — `api.request` は
    /// 1 回の API 呼び出し分の使用量を表す列なので、累計を積むと二重計上になる。
    fn token_count_event(&mut self, ctx: &RolloutSessionCtx, p: &Value, ordinal: i64, ts: i64) -> Event {
        let usage = p.get("info").and_then(|i| i.get("last_token_usage"));
        let input_tokens = usage.and_then(|u| u.get("input_tokens")).and_then(as_i64);
        let cache_read_tokens = usage
            .and_then(|u| u.get("cached_input_tokens"))
            .and_then(as_i64);
        let cache_write_tokens = usage
            .and_then(|u| u.get("cache_write_input_tokens"))
            .and_then(as_i64);
        let output_tokens = usage.and_then(|u| u.get("output_tokens")).and_then(as_i64);
        let reasoning_tokens = usage
            .and_then(|u| u.get("reasoning_output_tokens"))
            .and_then(as_i64);

        let primary_key = self.rollout_primary_key(ctx.session_id.as_deref(), ordinal);
        Event {
            event_id: event_id(&self.host_id, "log", event_type::API_REQUEST, &primary_key),
            ts,
            dt: dt_of(ts),
            host_id: self.host_id.clone(),
            agent: "codex".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            provider: ctx.provider.clone(),
            // token_count 自体は turn_id を持たない (実測で確認済み) — ctx が直近の
            // turn_context/task_started から持ち回っている値で補う (無ければ None のまま)。
            turn_id: ctx.current_turn_id.clone(),
            model: ctx.current_model.clone(),
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            usage_source: Some("log".to_string()),
            correlation_confidence: Some("none".to_string()),
            event_type: event_type::API_REQUEST.to_string(),
            ..Default::default()
        }
    }

    /// `item_completed` → `item.type` で分岐。
    fn handle_item_completed(
        &mut self,
        ctx: &mut RolloutSessionCtx,
        p: &Value,
        ordinal: i64,
    ) -> Vec<Event> {
        if let Some(t) = p.get("turn_id").and_then(Value::as_str) {
            ctx.current_turn_id = Some(t.to_string());
        }
        let Some(item) = p.get("item") else {
            self.mark_skipped("rollout:item_completed:no_item");
            return vec![];
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            self.mark_skipped("rollout:item:no_type");
            return vec![];
        };

        match item_type {
            "CommandExecution" => self.command_execution_events(ctx, p, item, ordinal),
            other => {
                self.mark_skipped(&format!("rollout:item:{other}"));
                vec![]
            }
        }
    }

    /// `item.type == "CommandExecution"` → `tool.call` + `tool.result` の 2 行。
    ///
    /// 実データにはツール呼び出し単位の Begin/End ペアが無く (crate ドキュメント参照)、
    /// 完了済みの 1 レコードしか届かない。そこで `item_completed` envelope 自身が持つ
    /// `started_at_ms`/`completed_at_ms` を使って、Claude Code の PreToolUse/PostToolUse
    /// と同じ「呼び出し 1 行 + 結果 1 行」の形に揃える (同じ primary_key・event_type が
    /// 異なるので event_id は衝突しない — `kikimimi_schema::event_id` のドキュメント参照)。
    /// `duration_ms` は `item.duration` (secs/nanos, 実測で構造化フィールドとして確認済み)
    /// を優先し、無ければ `completed_at_ms - started_at_ms` にフォールバックする。
    fn command_execution_events(
        &mut self,
        ctx: &RolloutSessionCtx,
        envelope_payload: &Value,
        item: &Value,
        ordinal: i64,
    ) -> Vec<Event> {
        let started_ms = envelope_payload.get("started_at_ms").and_then(as_i64);
        let completed_ms = envelope_payload.get("completed_at_ms").and_then(as_i64);
        let duration_ms = item
            .get("duration")
            .and_then(duration_obj_to_ms)
            .or_else(|| match (started_ms, completed_ms) {
                (Some(s), Some(c)) if c >= s => Some(c - s),
                _ => None,
            });
        let success = item.get("exit_code").and_then(as_i64).map(|c| c == 0);
        let turn_id = envelope_payload
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string);

        let primary_key = self.rollout_primary_key(ctx.session_id.as_deref(), ordinal);
        let call_ts = started_ms.or(completed_ms).unwrap_or_else(now_ms);
        let result_ts = completed_ms.unwrap_or(call_ts);

        // Shared fields between the call and result rows, built once and spread into
        // both literals below (event_id/ts/dt/duration_ms/success/event_type differ
        // per row and are set explicitly, overriding the `..base` defaults).
        let base = Event {
            host_id: self.host_id.clone(),
            agent: "codex".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            provider: ctx.provider.clone(),
            repo: ctx.repo.clone(),
            turn_id,
            // task instruction: "tool_name=\"Bash\"相当 → \"shell\"、tool_kind=\"bash\"".
            tool_name: Some("shell".to_string()),
            tool_kind: Some("bash".to_string()),
            correlation_confidence: Some("none".to_string()),
            ..Default::default()
        };

        let call = Event {
            event_id: event_id(&self.host_id, "log", event_type::TOOL_CALL, &primary_key),
            ts: call_ts,
            dt: dt_of(call_ts),
            event_type: event_type::TOOL_CALL.to_string(),
            ..base.clone()
        };
        let result = Event {
            event_id: event_id(&self.host_id, "log", event_type::TOOL_RESULT, &primary_key),
            ts: result_ts,
            dt: dt_of(result_ts),
            duration_ms,
            success,
            event_type: event_type::TOOL_RESULT.to_string(),
            ..base
        };

        vec![call, result]
    }

    /// rollout 由来イベントの一次キー: `<session_id>#<ordinal>`。
    ///
    /// hook の spool と違い、rollout の各行は `ordinal` を持つ**永続的な**内容 — daemon が
    /// 再起動して同じ行を読み直しても (byte offset の永続化が追いつかなかった場合など)
    /// 同じ `ordinal` が同じ内容に対して決定的に得られるので、`kikimimi-adapter-claude::
    /// Normalizer` の `epoch_nonce` のような「再起動をまたいだ衝突回避」の仕掛けは不要
    /// (むしろ epoch_nonce を混ぜると、同じ行の再読み込みが毎回別の event_id になり
    /// cloud 側の `ON CONFLICT DO NOTHING` 重複排除が効かなくなってしまう)。
    fn rollout_primary_key(&self, session_id: Option<&str>, ordinal: i64) -> String {
        format!("{}#{ordinal}", session_id.unwrap_or(""))
    }
}

/// `{"secs": N, "nanos": M}` (Rust `Duration` の serde 表現、実測で確認済み) を
/// ミリ秒に変換する。
fn duration_obj_to_ms(d: &Value) -> Option<i64> {
    let secs = d.get("secs").and_then(as_i64)?;
    let nanos = d.get("nanos").and_then(as_i64).unwrap_or(0);
    Some(secs * 1000 + nanos / 1_000_000)
}
