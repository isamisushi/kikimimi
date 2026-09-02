//! kikimimi-adapter-claude: Claude Code hooks / OTLP export → kikimimi_schema::Event 正規化
//! (docs/design/architecture.md §4 正規化, §4.1, §5.1)。
//!
//! - [`Normalizer::hook`]: `kikimimi hook <event>` の stdin JSON (hooks) を正規化する。
//! - [`Normalizer::otlp_logs`] / [`Normalizer::otlp_metrics`]: OTLP エクスポートを正規化する。
//! - [`TranscriptNormalizer`]: `~/.claude/projects/**/*.jsonl` (transcript) の
//!   ログ tailer 向け正規化。`Normalizer` とは別の、transcript ファイル 1 つに
//!   つき 1 インスタンスの状態機械 (詳細は `transcript` モジュールの doc comment)。
//!
//! PRIVACY: 本文 (`tool_input` / `tool_response` / prompt) は Event にコピーしない。
//! 本文オプトインは後続ステージ (§5.2) の責務。

mod classify;
mod hook;
mod otlp;
mod transcript;
mod util;

pub use transcript::TranscriptNormalizer;

use std::collections::{HashMap, VecDeque};

/// `seq` に同時に保持するセッション数の上限 (メモリ上限措置、下記 `Normalizer::seq` 参照)。
const MAX_TRACKED_SESSIONS: usize = 20_000;

/// Claude Code の hook JSON / OTLP export を `kikimimi_schema::Event` に正規化する状態。
///
/// tool_use_id を持たないイベント (session.start / turn など) の一次キーに使う
/// セッションごとの連番カウンタを保持する。
pub struct Normalizer {
    pub host_id: String,
    /// このプロセス起動 (= この `Normalizer` インスタンス) ごとに 1 回だけ採番される
    /// ランダムな値。tool_use_id を持たないイベントの一次キーに混ぜ込むことで、
    /// デーモンがクラッシュ・再起動した際に `seq` が 0 から数え直されても
    /// (再起動前と) 同じ primary_key ・ひいては同じ event_id を再生成しないようにする。
    /// これをしないと cloud 側の `event_id` UNIQUE + `ON CONFLICT DO NOTHING` 重複排除
    /// (architecture.md §5.1) が、再起動後の最初の tool_use_id 無しイベントを
    /// 「前回と同じ event_id」の誤検知で静かに落としてしまう (実データではなく)。
    epoch_nonce: String,
    /// session_id → 次に払い出す連番 (tool_use_id が無いイベント用)。
    ///
    /// `kikimimi agent` は長寿命の常駐デーモンなので、このマップを無条件に増やし続けると
    /// 何週間・何ヶ月も稼働した場合にじわじわメモリを消費する。ただし Claude Code の
    /// セッションは同じ session_id で後から resume されることがあるため、SessionEnd を
    /// 見た時点で即座にエントリを消すと、同一プロセス内で resume された時に seq が 1 から
    /// 数え直され、以前と同じ primary_key (= 同じ event_id) を再生成してしまう
    /// (event_id 衝突の再発)。そのため「即座に忘れる」のではなく `MAX_TRACKED_SESSIONS`
    /// を超えたときにだけ最も古く挿入されたセッションを追い出す FIFO キャッシュにして、
    /// 通常運用でセッションが resume される程度の頻度では衝突リスクを実質ゼロに保ちつつ、
    /// 上限を設ける。
    seq: HashMap<String, u64>,
    /// `seq` への挿入順 (FIFO 追い出し用)。
    seq_order: VecDeque<String>,
    /// hook_event_name / OTLP レコード名が未対応でスキップした件数の内訳。
    /// キーは未知の hook_event_name (例: "PreCompact")、hook_event_name が
    /// 無かった場合は "no_hook_event_name"、OTLP の未知レコードは
    /// "otlp:<record_name>" ("otlp:" プレフィックス)。
    skipped_by_reason: HashMap<String, u64>,
}

impl Normalizer {
    pub fn new(host_id: String) -> Self {
        Self {
            host_id,
            epoch_nonce: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            seq: HashMap::new(),
            seq_order: VecDeque::new(),
            skipped_by_reason: HashMap::new(),
        }
    }

    /// 未対応の hook_event_name / OTLP レコード名でスキップした累計件数
    /// (`skipped_by_reason()` の値の合計)。
    pub fn skipped(&self) -> u64 {
        self.skipped_by_reason.values().sum()
    }

    /// 未対応でスキップした件数の理由別内訳。キーの意味は `skipped_by_reason` フィールドの
    /// ドキュメントを参照。
    pub fn skipped_by_reason(&self) -> &HashMap<String, u64> {
        &self.skipped_by_reason
    }

    fn mark_skipped(&mut self, reason: &str) {
        *self
            .skipped_by_reason
            .entry(reason.to_string())
            .or_insert(0) += 1;
    }

    /// tool_use_id が無いイベント用の一次キー "<session_id>#<epoch_nonce>#<seq>" を払い出す。
    /// セッションごとに呼び出すたびに単調増加する `seq` は同一プロセス内での衝突を防ぎ、
    /// プロセスごとにランダムな `epoch_nonce` はデーモン再起動をまたいだ衝突を防ぐ
    /// (ただし冪等ではない: 同じ入力を再送すると別の event_id になる。tool_use_id を
    /// 持つイベントのみ再送に対して安定する — architecture.md §4 正規化)。
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

    /// event_id の一次キー: tool_use_id があればそれ、無ければ "<session_id>#<seq>"。
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
        let mut n = Normalizer::new("host-1".into());
        assert_eq!(n.primary_key(Some("toolu_1"), Some("sess")), "toolu_1");
        // 呼ぶたびに tool_use_id があれば安定 (seq を消費しない)
        assert_eq!(n.primary_key(Some("toolu_1"), Some("sess")), "toolu_1");
    }

    #[test]
    fn primary_key_falls_back_to_session_seq_and_increments() {
        let mut n = Normalizer::new("host-1".into());
        let a = n.primary_key(None, Some("sess"));
        let b = n.primary_key(None, Some("sess"));
        assert!(a.starts_with("sess#"), "got {a:?}");
        assert!(a.ends_with("#1"), "got {a:?}");
        assert!(b.ends_with("#2"), "got {b:?}");
        assert_ne!(a, b);
    }

    /// A fresh `Normalizer` (as created every `kikimimi agent` restart, agent.rs) must not
    /// reproduce the same primary key for a tool_use_id-less event as a previous process
    /// run, even for the exact same session_id + seq — otherwise cloud's `event_id` UNIQUE
    /// + `ON CONFLICT DO NOTHING` dedup (architecture.md §5.1) silently drops the new,
    /// genuinely different post-restart event as if it were a duplicate.
    #[test]
    fn primary_key_differs_across_simulated_daemon_restarts() {
        let mut before_restart = Normalizer::new("host-1".into());
        let mut after_restart = Normalizer::new("host-1".into());

        let key_before = before_restart.primary_key(None, Some("sess"));
        let key_after = after_restart.primary_key(None, Some("sess"));

        assert_ne!(
            key_before, key_after,
            "same session_id + same first seq must not collide across a fresh Normalizer (daemon restart)"
        );

        let id_before = kikimimi_schema::event_id("host-1", "hook", "session.start", &key_before);
        let id_after = kikimimi_schema::event_id("host-1", "hook", "session.start", &key_after);
        assert_ne!(id_before, id_after);
    }

    #[test]
    fn resumed_session_in_same_process_still_gets_a_distinct_key() {
        // A session_id can legitimately be resumed (another SessionStart/SessionEnd pair)
        // within the same long-lived daemon process. seq must keep advancing for it rather
        // than being reset, or the resumed SessionStart would collide with the original one.
        let mut n = Normalizer::new("host-1".into());
        let a = n.primary_key(None, Some("sess"));
        let b = n.primary_key(None, Some("sess")); // pretend some events happened in between
        let c = n.primary_key(None, Some("sess")); // "resumed" SessionStart
        assert!(a.ends_with("#1"));
        assert!(b.ends_with("#2"));
        assert!(c.ends_with("#3"));
    }

    #[test]
    fn seq_map_is_capped_by_evicting_oldest_session_first() {
        let mut n = Normalizer::new("host-1".into());
        for i in 0..MAX_TRACKED_SESSIONS + 10 {
            n.primary_key(None, Some(&format!("sess-{i}")));
        }
        assert_eq!(
            n.seq.len(),
            MAX_TRACKED_SESSIONS,
            "map must not grow past the cap"
        );
        assert!(
            !n.seq.contains_key("sess-0"),
            "the oldest session must have been evicted first (FIFO)"
        );
        assert!(
            n.seq
                .contains_key(&format!("sess-{}", MAX_TRACKED_SESSIONS + 9)),
            "the most recently seen session must still be tracked"
        );
    }

    #[test]
    fn skipped_starts_at_zero() {
        let n = Normalizer::new("host-1".into());
        assert_eq!(n.skipped(), 0);
        assert!(n.skipped_by_reason().is_empty());
    }
}
