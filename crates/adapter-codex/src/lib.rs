//! kikimimi-adapter-codex: Codex CLI hooks / rollout JSONL → kikimimi_schema::Event 正規化
//! (docs/design/architecture.md §4 正規化, §4.1 Codex 行)。
//!
//! - [`CodexNormalizer::hook`]: `kikimimi hook <event>` の stdin JSON (Codex の
//!   Claude Code 互換 hooks) を正規化する。
//! - [`CodexNormalizer::rollout_line`]: `~/.codex/sessions/**/rollout-*.jsonl` の 1 行を
//!   正規化する (ログ tailer 側)。
//!
//! # 実機調査で分かったこと (2026-08-31, codex-cli 0.151.0, Linux) — 詳細はタスク報告を参照
//!
//! `internal/research/hook-telemetry-daemon.md` の Codex 節は
//! `SessionMeta` / `ExecCommandBegin` / `ExecCommandEnd` / `McpToolCallBegin` /
//! `McpToolCallEnd` / `PatchApplyBegin` / `PatchApplyEnd` / `TokenCount` という
//! フラットなトップレベル rollout エントリ種別を報告しているが、これは **stale**:
//! インストール済みの codex-cli 0.151.0 が実際にこのマシンで書いた rollout JSONL
//! (`~/.codex/sessions/2026/08/31/rollout-*.jsonl`) は、行ごとに
//! `{"timestamp","ordinal","type","payload"}` という封筒に包まれており、`type` は
//! `session_meta` / `event_msg` / `response_item` / `world_state` / `turn_context` の
//! いずれか。ツール実行は `event_msg.item_completed` の中の `item.type ==
//! "CommandExecution"` として (Begin/End のペアではなく) **完了済みの 1 レコード**で
//! 届く (`duration`/`exit_code`/`command`/`cwd` を含む)。`exec_command_begin` /
//! `exec_command_end` / `mcp_tool_call_begin` / `mcp_tool_call_end` /
//! `patch_apply_begin` といった文字列自体はバイナリ内に実在する
//! (`strings` で確認済み) が、これは `[otel]` エクスポート/アプリサーバー向けの
//! イベント名の語彙であって、ディスク上の rollout JSONL のトップレベル種別ではない —
//! 本アダプタは実際にディスクへ書かれる形を正とする。
//!
//! [`rollout_line`](CodexNormalizer::rollout_line) は `session_id`/`turn_id`/`model` を
//! 行をまたいで持ち回るための小さな可変状態 [`RolloutSessionCtx`] を呼び出し側から
//! 受け取る (1 rollout ファイル = 1 Codex セッションなので、tailer 側がファイルごとに
//! 1 つ保持する想定)。
//!
//! PRIVACY: 本文 (`tool_input` / `tool_response` / プロンプト・応答テキスト / シェル出力)
//! は Event にコピーしない (`kikimimi-adapter-claude` と同じ方針)。

mod classify;
mod hook;
mod rollout;
mod util;

pub use rollout::RolloutSessionCtx;

use std::collections::{HashMap, VecDeque};

/// `seq` に同時に保持するセッション数の上限 (`kikimimi-adapter-claude::Normalizer` と同じ
/// メモリ上限措置)。
const MAX_TRACKED_SESSIONS: usize = 20_000;

/// Codex CLI の hook JSON / rollout JSONL を `kikimimi_schema::Event` に正規化する状態。
///
/// `hook()` 用に、`tool_use_id` を持たないイベント (SessionStart/SessionEnd/turn 等) の
/// 一次キーに使うセッションごとの連番カウンタを持つ — これは
/// `kikimimi-adapter-claude::Normalizer` と完全に同じ理由・同じ形 (daemon 再起動をまたいだ
/// primary_key 衝突を避けるための `epoch_nonce`。詳細は同クレートの `Normalizer` の
/// ドキュメント参照)。あえて共有クレートに抽出せず並行して持つ (`hooks/src/util.rs` 的な
/// 重複だが、adapter-claude が長期間安定稼働しているものに手を入れるリスクを避ける —
/// Stage 1 で両アダプタの形が固まったら `kikimimi-schema` 側へ抽出するのが妥当)。
///
/// `rollout_line()` 側は事情が異なり、rollout JSONL の各行が (spool の 1 ファイル 1 通知
/// と違って) **`ordinal` を持つ永続的な内容**なので、`session_id + ordinal` だけで
/// daemon 再起動をまたいでも決定的に同じ一次キーを再生成できる — こちらは
/// `epoch_nonce` を使わない (`rollout.rs` のドキュメント参照)。
pub struct CodexNormalizer {
    pub host_id: String,
    /// このプロセス起動ごとに 1 回だけ採番されるランダムな値 (`hook()` の
    /// tool_use_id 無しイベント用。理由は `kikimimi-adapter-claude::Normalizer` と同じ)。
    epoch_nonce: String,
    /// session_id → 次に払い出す連番 (`hook()` 用、tool_use_id が無いイベント用)。
    seq: HashMap<String, u64>,
    seq_order: VecDeque<String>,
    /// hook_event_name / rollout の種別が未対応でスキップした件数の内訳。
    /// キーの意味は `hook.rs` / `rollout.rs` 各所の `mark_skipped` 呼び出しを参照。
    skipped_by_reason: HashMap<String, u64>,
}

impl CodexNormalizer {
    pub fn new(host_id: String) -> Self {
        Self {
            host_id,
            epoch_nonce: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            seq: HashMap::new(),
            seq_order: VecDeque::new(),
            skipped_by_reason: HashMap::new(),
        }
    }

    /// 未対応でスキップした累計件数。
    pub fn skipped(&self) -> u64 {
        self.skipped_by_reason.values().sum()
    }

    /// 未対応でスキップした件数の理由別内訳。
    pub fn skipped_by_reason(&self) -> &HashMap<String, u64> {
        &self.skipped_by_reason
    }

    fn mark_skipped(&mut self, reason: &str) {
        *self
            .skipped_by_reason
            .entry(reason.to_string())
            .or_insert(0) += 1;
    }

    /// `hook()` 用: tool_use_id が無いイベント用の一次キー
    /// "<session_id>#<epoch_nonce>#<seq>" を払い出す。
    /// (`kikimimi-adapter-claude::Normalizer::next_seq_key` と同じロジック)
    fn next_seq_key(&mut self, session_id: Option<&str>) -> String {
        let sid = session_id.unwrap_or("");
        if !self.seq.contains_key(sid) {
            self.seq_order.push_back(sid.to_string());
            if self.seq.len() >= MAX_TRACKED_SESSIONS {
                if let Some(oldest) = self.seq_order.pop_front() {
                    self.seq.remove(&oldest);
                }
            }
        }
        let counter = self.seq.entry(sid.to_string()).or_insert(0);
        *counter += 1;
        format!("{sid}#{}#{counter}", self.epoch_nonce)
    }

    /// `hook()` 用の event_id 一次キー: tool_use_id があればそれ、無ければ
    /// "<session_id>#<epoch_nonce>#<seq>"。
    fn primary_key(&mut self, tool_use_id: Option<&str>, session_id: Option<&str>) -> String {
        match tool_use_id {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => self.next_seq_key(session_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_key_uses_tool_use_id_when_present() {
        let mut n = CodexNormalizer::new("host-1".into());
        assert_eq!(n.primary_key(Some("call_1"), Some("sess")), "call_1");
        assert_eq!(n.primary_key(Some("call_1"), Some("sess")), "call_1");
    }

    #[test]
    fn primary_key_falls_back_to_session_seq_and_increments() {
        let mut n = CodexNormalizer::new("host-1".into());
        let a = n.primary_key(None, Some("sess"));
        let b = n.primary_key(None, Some("sess"));
        assert!(a.ends_with("#1"), "got {a:?}");
        assert!(b.ends_with("#2"), "got {b:?}");
        assert_ne!(a, b);
    }

    #[test]
    fn primary_key_differs_across_simulated_daemon_restarts() {
        let mut before_restart = CodexNormalizer::new("host-1".into());
        let mut after_restart = CodexNormalizer::new("host-1".into());
        let key_before = before_restart.primary_key(None, Some("sess"));
        let key_after = after_restart.primary_key(None, Some("sess"));
        assert_ne!(key_before, key_after);
    }

    #[test]
    fn seq_map_is_capped_by_evicting_oldest_session_first() {
        let mut n = CodexNormalizer::new("host-1".into());
        for i in 0..MAX_TRACKED_SESSIONS + 10 {
            n.primary_key(None, Some(&format!("sess-{i}")));
        }
        assert_eq!(n.seq.len(), MAX_TRACKED_SESSIONS);
        assert!(!n.seq.contains_key("sess-0"));
        assert!(n
            .seq
            .contains_key(&format!("sess-{}", MAX_TRACKED_SESSIONS + 9)));
    }

    #[test]
    fn skipped_starts_at_zero() {
        let n = CodexNormalizer::new("host-1".into());
        assert_eq!(n.skipped(), 0);
        assert!(n.skipped_by_reason().is_empty());
    }
}
