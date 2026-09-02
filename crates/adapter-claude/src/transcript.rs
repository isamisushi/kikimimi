//! Claude Code transcript JSONL (`~/.claude/projects/**/*.jsonl`) の 1 行 →
//! kikimimi_schema::Event (architecture.md §4 「ログ tailer」, §4.1 Claude Code 行)。
//!
//! # 実データの形 (2026-09-02 実測, Claude Code 2.1.258; ドリフトする前提で扱う)
//!
//! 1 行 1 JSON オブジェクト。観測した `type` は `user` / `assistant` / `system` /
//! `attachment` に加え、`mode` / `permission-mode` / `bridge-session` /
//! `file-history-snapshot` / `last-prompt` / `ai-title` / `queue-operation` /
//! `frame-link` など多数のブックキーピング種別 — これらは `mark_skipped(type)` で
//! スキップして件数だけ数える (fail-open)。
//!
//! `sessionId` / `timestamp` (ISO-8601 UTC 文字列) / `cwd` / `version` /
//! `gitBranch` / `uuid` / `parentUuid` / `isSidechain` / `userType` は
//! `user`/`assistant`/`system`/`attachment` 共通のフィールド。`user` 行:
//! `promptId` (ターンごとの id) / `isMeta` / `message.role` /
//! `message.content` (文字列 または `{type: text|tool_result, ...}` ブロック配列)。
//! `assistant` 行: `requestId` (API リクエストごとの id) / `effort` /
//! `isApiErrorMessage`/`apiErrorStatus`/`error` (失敗時) / `message.id` /
//! `message.model` / `message.stop_reason` / `message.usage`
//! `{input_tokens, output_tokens, cache_creation_input_tokens,
//! cache_read_input_tokens, output_tokens_details.thinking_tokens, ...}` /
//! `message.content` (`{type: thinking|text|tool_use, ...}` ブロック配列)。
//! 1 回の API レスポンスが `message.id`/`requestId` を共有する複数の `assistant`
//! 行に分かれて書かれることがあり (ブロックごとに 1 行、`apiBlockIndex`)、
//! 各行が同じ `usage` を繰り返す — トークンは `requestId` (無ければ `message.id`)
//! ごとに 1 回だけ数える。
//!
//! PRIVACY (§5.2): `tool_input`/`tool_result` の中身・prompt 本文・assistant の
//! テキスト・thinking は Event にコピーしない。唯一の例外は `hook.rs` と同じ
//! `tool_use.input.skill` (Skill 名。`tool_name` と同格のメタデータ)。
//!
//! # 設計: [`TranscriptNormalizer`] はセッション (= transcript ファイル) ごとに 1 つ
//!
//! `Normalizer`/`CodexNormalizer` (デーモン起動ごとに 1 つ、`epoch_nonce` で再起動を
//! またいだ primary_key 衝突を避ける) とは事情が異なる: この一次キー
//! (tool_use id・promptId・requestId・session_id) はすべて transcript の中身
//! そのものの値なので、daemon が再起動して同じファイルを読み直しても
//! 決定的に同じ primary_key (= 同じ event_id) を再生成できる
//! (`kikimimi-adapter-codex::rollout::RolloutSessionCtx` の ordinal と同じ理由 —
//! `epoch_nonce`/`seq` の仕掛けは不要)。そのためログ tailer は transcript
//! ファイル 1 つにつき `TranscriptNormalizer::new(host_id)` を 1 つ保持し、
//! ファイルの各行を [`TranscriptNormalizer::line`] に渡し、EOF (または
//! セッション終了とみなすタイミング) で 1 回だけ [`TranscriptNormalizer::finish`]
//! を呼ぶ想定。
//!
//! 壊れた JSON 行 (パース不能) は呼び出し側 (tailer) の責務で数える —
//! `rollout.rs` の doc comment と同じ考え方で、本モジュールの `line` は
//! 常に既にパース済みの `serde_json::Value` を受け取る。

use std::collections::{HashMap, HashSet};

use kikimimi_schema::{cwd_hash, dt_of, event_id, event_type, Event};
use serde_json::Value;

use crate::classify::classify_tool;
use crate::util::{as_i64, parse_iso8601_ms};

/// 1 つの transcript ファイル (= 1 Claude Code セッション) を処理する間、
/// 行をまたいで持ち回る状態。モジュール doc comment の「設計」節を参照。
pub struct TranscriptNormalizer {
    pub host_id: String,
    session_id: Option<String>,
    /// このセッションで最初に見つかったタイムスタンプ (session.start の ts)。
    first_ts: Option<i64>,
    /// 直近に見つかったタイムスタンプ (finish() 時点で session.end の ts になる)。
    last_ts: Option<i64>,
    first_cwd_hash: Option<String>,
    first_agent_version: Option<String>,
    last_cwd_hash: Option<String>,
    last_agent_version: Option<String>,
    /// 直近に見えた promptId。assistant 行は promptId を持たないため
    /// (実測、2026-09-02)、tool_use の turn_id をこれで補う。
    current_turn_id: Option<String>,
    /// tool_use_id → 呼び出し時点で分かっていた情報。対応する tool_result が
    /// 後続行で来たときに tool_name/kind/mcp_server/mcp_tool・duration_ms を
    /// 埋めるために引く (`kikimimi-adapter-codex::rollout` の
    /// "同じシェル実行を跨いで対応づける" パターンと同じ発想)。
    tool_calls: HashMap<String, ToolCallSeen>,
    /// 使用量を計上済みの requestId (無ければ message.id)。同じ API レスポンスが
    /// 複数の assistant 行に分かれていても api.request は 1 回だけ出す。
    usage_counted: HashSet<String>,
    /// 未対応の type / 中身が無く判定できなかった行の内訳 (件数)。
    skipped_by_reason: HashMap<String, u64>,
}

#[derive(Clone)]
struct ToolCallSeen {
    tool_name: Option<String>,
    tool_kind: Option<String>,
    mcp_server: Option<String>,
    mcp_tool: Option<String>,
    ts: i64,
}

/// `line()` 内で一度だけ計算する、その行に共通のコンテキスト。
struct LineCtx {
    ts: i64,
    session_id: Option<String>,
    cwd_hash: Option<String>,
    agent_version: Option<String>,
}

impl TranscriptNormalizer {
    pub fn new(host_id: String) -> Self {
        Self {
            host_id,
            session_id: None,
            first_ts: None,
            last_ts: None,
            first_cwd_hash: None,
            first_agent_version: None,
            last_cwd_hash: None,
            last_agent_version: None,
            current_turn_id: None,
            tool_calls: HashMap::new(),
            usage_counted: HashSet::new(),
            skipped_by_reason: HashMap::new(),
        }
    }

    /// 未対応の type 等でスキップした累計件数。
    pub fn skipped(&self) -> u64 {
        self.skipped_by_reason.values().sum()
    }

    /// 未対応でスキップした件数の理由別内訳。キーは主に record の `type` 文字列
    /// そのもの (例 "mode")、`type` が無ければ "no_type"。`user`/`assistant`
    /// 行の中でも実イベントを生成できなかったケースは "user:*"/"assistant:*"
    /// プレフィックス付きの理由になる (詳細は各ヘルパーの doc comment)。
    pub fn skipped_by_reason(&self) -> &HashMap<String, u64> {
        &self.skipped_by_reason
    }

    fn mark_skipped(&mut self, reason: &str) {
        *self
            .skipped_by_reason
            .entry(reason.to_string())
            .or_insert(0) += 1;
    }

    /// transcript の 1 行を正規化する。未対応の `type` は `mark_skipped` して
    /// 空配列を返す (fail-open)。
    ///
    /// `type` に関わらず、この行が持つ `sessionId`/`cwd`/`version` は毎回
    /// 最新値として (session.start/session.end に使う) 状態を更新する —
    /// これらはブックキーピング行にも付いている共通フィールドのため。
    /// `timestamp` を持つ行にこのセッションで初めて出会った時点で
    /// `session.start` を 1 件差し込む (`type` がブックキーピングでスキップ
    /// 対象であっても — 「セッションでタイムスタンプ付きの最初の行」が基準)。
    pub fn line(&mut self, raw: &Value) -> Vec<Event> {
        let mut out = Vec::new();

        if let Some(sid) = raw.get("sessionId").and_then(Value::as_str) {
            self.session_id = Some(sid.to_string());
        }

        let cwd_hash_now = raw.get("cwd").and_then(Value::as_str).map(cwd_hash);
        let agent_version_now = raw
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string);
        if cwd_hash_now.is_some() {
            self.last_cwd_hash = cwd_hash_now.clone();
            if self.first_cwd_hash.is_none() {
                self.first_cwd_hash = cwd_hash_now.clone();
            }
        }
        if agent_version_now.is_some() {
            self.last_agent_version = agent_version_now.clone();
            if self.first_agent_version.is_none() {
                self.first_agent_version = agent_version_now.clone();
            }
        }

        let ts_now = raw
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso8601_ms);
        if let Some(ts) = ts_now {
            self.last_ts = Some(ts);
            if self.first_ts.is_none() {
                self.first_ts = Some(ts);
                out.push(self.session_boundary_event(
                    event_type::SESSION_START,
                    ts,
                    self.first_cwd_hash.clone(),
                    self.first_agent_version.clone(),
                ));
            }
        }

        match raw.get("type").and_then(Value::as_str) {
            Some("user") => {
                if let Some(ts) = ts_now {
                    let ctx = LineCtx {
                        ts,
                        session_id: self.session_id.clone(),
                        cwd_hash: cwd_hash_now,
                        agent_version: agent_version_now,
                    };
                    out.extend(self.handle_user(raw, &ctx));
                } else {
                    self.mark_skipped("user:no_timestamp");
                }
            }
            Some("assistant") => {
                if let Some(ts) = ts_now {
                    let ctx = LineCtx {
                        ts,
                        session_id: self.session_id.clone(),
                        cwd_hash: cwd_hash_now,
                        agent_version: agent_version_now,
                    };
                    out.extend(self.handle_assistant(raw, &ctx));
                } else {
                    self.mark_skipped("assistant:no_timestamp");
                }
            }
            Some(other) => self.mark_skipped(other),
            None => self.mark_skipped("no_type"),
        }

        out
    }

    /// ファイル末尾で 1 回呼ぶ。このセッションでタイムスタンプ付きの行を
    /// 一度も見ていなければ (空ファイル・全滅ファイル) 何も返さない。
    pub fn finish(&mut self) -> Vec<Event> {
        let Some(last_ts) = self.last_ts else {
            return Vec::new();
        };
        vec![self.session_boundary_event(
            event_type::SESSION_END,
            last_ts,
            self.last_cwd_hash.clone(),
            self.last_agent_version.clone(),
        )]
    }

    fn session_boundary_event(
        &self,
        event_type_str: &'static str,
        ts: i64,
        cwd_hash: Option<String>,
        agent_version: Option<String>,
    ) -> Event {
        let sid = self.session_id.clone().unwrap_or_default();
        let primary_key = format!("{sid}#{event_type_str}");
        Event {
            event_id: event_id(&self.host_id, "log", event_type_str, &primary_key),
            ts,
            dt: dt_of(ts),
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "log".to_string(),
            session_id: self.session_id.clone(),
            cwd_hash,
            agent_version,
            correlation_confidence: Some("none".to_string()),
            event_type: event_type_str.to_string(),
            ..Default::default()
        }
    }

    /// `user` 行 → tool_result ブロックがあれば `tool.result` イベント群
    /// (それ以外の内容は無視)、無ければ「本物のプロンプトか」を判定して
    /// `turn` イベントを 0 または 1 件返す。
    fn handle_user(&mut self, raw: &Value, ctx: &LineCtx) -> Vec<Event> {
        let mut out = Vec::new();

        let Some(message) = raw.get("message") else {
            self.mark_skipped("user:no_message");
            return out;
        };
        let content = message.get("content");
        let blocks = content_blocks(content);

        let prompt_id = raw
            .get("promptId")
            .and_then(Value::as_str)
            .map(str::to_string);
        // tool_result を運ぶ行も promptId を持つ (実測) — assistant 行が
        // promptId を持たない分、こちらを "直近のターン" の情報源として使う。
        if let Some(pid) = &prompt_id {
            self.current_turn_id = Some(pid.clone());
        }

        if has_block_type(blocks, "tool_result") {
            for block in blocks.into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                match self.tool_result_event(block, ctx, prompt_id.clone()) {
                    Some(ev) => out.push(ev),
                    None => self.mark_skipped("tool_result:no_tool_use_id"),
                }
            }
            return out;
        }

        let is_meta = raw.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
        if is_meta {
            self.mark_skipped("user:meta");
            return out;
        }

        let is_turn_content = match content {
            Some(Value::String(_)) => true,
            Some(Value::Array(_)) => has_block_type(blocks, "text"),
            _ => false,
        };
        if !is_turn_content {
            self.mark_skipped("user:no_turn_content");
            return out;
        }

        match self.turn_event(raw, ctx, prompt_id) {
            Some(ev) => out.push(ev),
            None => self.mark_skipped("turn:no_id"),
        }
        out
    }

    fn turn_event(&self, raw: &Value, ctx: &LineCtx, prompt_id: Option<String>) -> Option<Event> {
        let uuid = raw.get("uuid").and_then(Value::as_str).map(str::to_string);
        let primary_key = prompt_id.clone().or(uuid)?;
        Some(Event {
            event_id: event_id(&self.host_id, "log", event_type::TURN, &primary_key),
            ts: ctx.ts,
            dt: dt_of(ctx.ts),
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            turn_id: prompt_id,
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            correlation_confidence: Some("none".to_string()),
            event_type: event_type::TURN.to_string(),
            ..Default::default()
        })
    }

    fn tool_result_event(
        &mut self,
        block: &Value,
        ctx: &LineCtx,
        prompt_id: Option<String>,
    ) -> Option<Event> {
        let tool_use_id = block
            .get("tool_use_id")
            .and_then(Value::as_str)?
            .to_string();

        // is_error 欠落 = 成功 (Anthropic API のセマンティクス。hook.rs の
        // tool_response.success 欠落 = "unknown" とは扱いが異なる — こちらは
        // 「無ければ true」が仕様として確定しているフィールドのため、原則7の
        // 「推定で埋めない」には抵触しない)。
        let is_error = block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let success = !is_error;

        let seen = self.tool_calls.get(&tool_use_id).cloned();
        let (tool_name, tool_kind, mcp_server, mcp_tool, duration_ms) = match &seen {
            Some(s) => (
                s.tool_name.clone(),
                s.tool_kind.clone(),
                s.mcp_server.clone(),
                s.mcp_tool.clone(),
                (ctx.ts >= s.ts).then_some(ctx.ts - s.ts),
            ),
            None => (None, None, None, None, None),
        };

        Some(Event {
            event_id: event_id(&self.host_id, "log", event_type::TOOL_RESULT, &tool_use_id),
            ts: ctx.ts,
            dt: dt_of(ctx.ts),
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            turn_id: prompt_id,
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            correlation_key: Some(tool_use_id),
            correlation_confidence: Some("exact".to_string()),
            event_type: event_type::TOOL_RESULT.to_string(),
            tool_name,
            tool_kind,
            mcp_server,
            mcp_tool,
            success: Some(success),
            duration_ms,
            ..Default::default()
        })
    }

    /// `assistant` 行 → `isApiErrorMessage` なら `api.error` のみ、そうでなければ
    /// `message.usage` から (requestId ごとに 1 回だけ) `api.request`、
    /// `message.content` の `tool_use` ブロックごとに `tool.call`。
    fn handle_assistant(&mut self, raw: &Value, ctx: &LineCtx) -> Vec<Event> {
        let mut out = Vec::new();

        if raw.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
            match self.api_error_event(raw, ctx) {
                Some(ev) => out.push(ev),
                None => self.mark_skipped("api_error:no_id"),
            }
            return out;
        }

        let Some(message) = raw.get("message") else {
            self.mark_skipped("assistant:no_message");
            return out;
        };

        let model = message
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string);
        let effort = raw
            .get("effort")
            .and_then(Value::as_str)
            .map(str::to_string);
        let request_id = raw
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let usage_key = request_id.or(message_id);

        if let Some(key) = &usage_key {
            if !self.usage_counted.contains(key) {
                if let Some(usage) = message.get("usage") {
                    out.push(self.api_request_event(
                        usage,
                        ctx,
                        model.clone(),
                        effort.clone(),
                        key,
                    ));
                    self.usage_counted.insert(key.clone());
                }
            }
        }

        let blocks = content_blocks(message.get("content"));
        for block in blocks.into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            match self.tool_call_event(block, ctx, effort.clone()) {
                Some(ev) => out.push(ev),
                None => self.mark_skipped("tool_use:no_id"),
            }
        }

        out
    }

    fn api_request_event(
        &self,
        usage: &Value,
        ctx: &LineCtx,
        model: Option<String>,
        effort: Option<String>,
        primary_key: &str,
    ) -> Event {
        let reasoning_tokens = usage
            .get("output_tokens_details")
            .and_then(|d| d.get("thinking_tokens"))
            .and_then(as_i64);

        Event {
            event_id: event_id(&self.host_id, "log", event_type::API_REQUEST, primary_key),
            ts: ctx.ts,
            dt: dt_of(ctx.ts),
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            correlation_confidence: Some("none".to_string()),
            event_type: event_type::API_REQUEST.to_string(),
            model,
            effort,
            input_tokens: usage.get("input_tokens").and_then(as_i64),
            output_tokens: usage.get("output_tokens").and_then(as_i64),
            cache_read_tokens: usage.get("cache_read_input_tokens").and_then(as_i64),
            cache_write_tokens: usage.get("cache_creation_input_tokens").and_then(as_i64),
            reasoning_tokens,
            cost_usd: None,
            usage_source: Some("log".to_string()),
            ..Default::default()
        }
    }

    fn api_error_event(&self, raw: &Value, ctx: &LineCtx) -> Option<Event> {
        let message = raw.get("message");
        let request_id = raw
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let message_id = message
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let primary_key = request_id.or(message_id)?;

        let model = message
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let effort = raw
            .get("effort")
            .and_then(Value::as_str)
            .map(str::to_string);
        // error_type = apiErrorStatus を文字列化したもの (never the error text —
        // "error" フィールドは短い分類語だが本文と紙一重のため使わない)。
        let error_type = raw.get("apiErrorStatus").and_then(scalar_to_string);

        Some(Event {
            event_id: event_id(&self.host_id, "log", event_type::API_ERROR, &primary_key),
            ts: ctx.ts,
            dt: dt_of(ctx.ts),
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            correlation_confidence: Some("none".to_string()),
            event_type: event_type::API_ERROR.to_string(),
            model,
            effort,
            error_type,
            ..Default::default()
        })
    }

    fn tool_call_event(
        &mut self,
        block: &Value,
        ctx: &LineCtx,
        effort: Option<String>,
    ) -> Option<Event> {
        let id = block.get("id").and_then(Value::as_str)?.to_string();
        let tool_name = block
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);

        let (tool_kind, mcp_server, mcp_tool) = match &tool_name {
            Some(tn) => {
                let class = classify_tool(tn);
                (
                    Some(class.kind.to_string()),
                    class.mcp_server,
                    class.mcp_tool,
                )
            }
            None => (None, None, None),
        };

        // Skill 名は tool_name と同格のメタデータとして tool_input.skill から抽出する
        // (hook.rs と同じ、PRIVACY 方針の唯一の例外)。
        let skill_name = if tool_kind.as_deref() == Some("skill") {
            block
                .get("input")
                .and_then(|i| i.get("skill"))
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };

        self.tool_calls.insert(
            id.clone(),
            ToolCallSeen {
                tool_name: tool_name.clone(),
                tool_kind: tool_kind.clone(),
                mcp_server: mcp_server.clone(),
                mcp_tool: mcp_tool.clone(),
                ts: ctx.ts,
            },
        );

        Some(Event {
            event_id: event_id(&self.host_id, "log", event_type::TOOL_CALL, &id),
            ts: ctx.ts,
            dt: dt_of(ctx.ts),
            host_id: self.host_id.clone(),
            agent: "claude-code".to_string(),
            source: "log".to_string(),
            session_id: ctx.session_id.clone(),
            turn_id: self.current_turn_id.clone(),
            cwd_hash: ctx.cwd_hash.clone(),
            agent_version: ctx.agent_version.clone(),
            correlation_key: Some(id),
            correlation_confidence: Some("exact".to_string()),
            event_type: event_type::TOOL_CALL.to_string(),
            tool_name,
            tool_kind,
            mcp_server,
            mcp_tool,
            skill_name,
            effort,
            ..Default::default()
        })
    }
}

/// `message.content` (文字列 or ブロック配列) からブロック配列だけを取り出す。
fn content_blocks(content: Option<&Value>) -> Option<&Vec<Value>> {
    match content {
        Some(Value::Array(arr)) => Some(arr),
        _ => None,
    }
}

/// ブロック配列に指定した `type` のブロックが 1 つでもあるか。
fn has_block_type(blocks: Option<&Vec<Value>>, ty: &str) -> bool {
    blocks
        .map(|b| {
            b.iter()
                .any(|blk| blk.get("type").and_then(Value::as_str) == Some(ty))
        })
        .unwrap_or(false)
}

/// JSON の数値/文字列/真偽値を、そのまま文字列表現にする
/// (`apiErrorStatus` が数値 429 で来ても "429" として error_type に入れる用)。
fn scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_blocks_only_matches_arrays() {
        assert!(content_blocks(Some(&serde_json::json!("x"))).is_none());
        assert!(content_blocks(None).is_none());
        let arr = serde_json::json!([{"type": "text"}]);
        assert!(content_blocks(Some(&arr)).is_some());
    }

    #[test]
    fn has_block_type_finds_and_misses() {
        let arr = serde_json::json!([{"type": "thinking"}, {"type": "tool_use"}]);
        let blocks = content_blocks(Some(&arr));
        assert!(has_block_type(blocks, "tool_use"));
        assert!(!has_block_type(blocks, "text"));
        assert!(!has_block_type(None, "text"));
    }

    #[test]
    fn scalar_to_string_handles_number_string_bool() {
        assert_eq!(
            scalar_to_string(&serde_json::json!(429)),
            Some("429".to_string())
        );
        assert_eq!(
            scalar_to_string(&serde_json::json!("rate_limit")),
            Some("rate_limit".to_string())
        );
        assert_eq!(
            scalar_to_string(&serde_json::json!(true)),
            Some("true".to_string())
        );
        assert_eq!(scalar_to_string(&serde_json::json!(null)), None);
    }

    #[test]
    fn finish_without_any_line_returns_empty() {
        let mut n = TranscriptNormalizer::new("host-1".into());
        assert!(n.finish().is_empty());
    }

    #[test]
    fn skipped_starts_at_zero() {
        let n = TranscriptNormalizer::new("host-1".into());
        assert_eq!(n.skipped(), 0);
        assert!(n.skipped_by_reason().is_empty());
    }
}
