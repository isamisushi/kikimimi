//! 設定済み MCP サーバー名を集める — 「導入されているのに一度も呼ばれないサーバー」
//! (architecture.md §7.1 「導入されているのに呼ばれないサーバー」, §7.2
//! `unused_mcp_server`) を検知するための設定スナップショット。
//!
//! `~/.claude/settings.json` + `~/.claude.json` (トップレベル `mcpServers` と
//! `projects.<path>.mcpServers`, 元は `query_cmd.rs` の `unused-mcp` 実装) に加え、
//! `<cwd>/.mcp.json` (Claude Code のプロジェクトスコープ MCP 設定、トップレベル
//! `mcpServers` のキー) も読む。すべて **defensive** に読む: 存在しない・読めない・
//! JSON として壊れている場合はそのファイルから単に何も得ない (エラーにしない) —
//! デーモンの drain ループが session.start のたびにこれを呼んでも壊れないため
//! (fail-open, architecture.md §2.2)。
//!
//! [`McpConfigCache`] は `cwd` ごとに短い TTL でキャッシュする
//! (`repo_resolve.rs::RepoResolver` と同じ考え方) — session.start がバーストで
//! 届いても、その間は毎回ファイルを読み直さない。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// キャッシュエントリの有効期間。session.start のバーストをならすだけなので短くてよい。
const CACHE_TTL: Duration = Duration::from_secs(30);
/// `repo_resolve.rs::MAX_CACHE_ENTRIES` と同じ考え方: 上限に達したら単純に `clear()` する。
const MAX_CACHE_ENTRIES: usize = 1024;

/// `cwd` → (取得時刻, 設定済みサーバー名リスト) の per-cwd キャッシュ。
/// `kikimimi agent` の drain ループが session.start ごとに毎回ファイルを読み直さない
/// ようにするためのもの (`RepoResolver` と対になるキャッシュ)。
#[derive(Debug, Default)]
pub struct McpConfigCache {
    cache: HashMap<PathBuf, (Instant, Vec<String>)>,
}

impl McpConfigCache {
    /// `cwd` の設定済み MCP サーバー名を返す。直近 [`CACHE_TTL`] 以内に同じ `cwd` で
    /// 呼ばれていればファイルを読み直さずそのまま返す。
    pub fn get(&mut self, cwd: &str) -> Vec<String> {
        let key = PathBuf::from(cwd);
        if let Some((fetched_at, servers)) = self.cache.get(&key) {
            if fetched_at.elapsed() < CACHE_TTL {
                return servers.clone();
            }
        }
        let servers = configured_for_cwd(Some(cwd));
        if self.cache.len() >= MAX_CACHE_ENTRIES {
            self.cache.clear();
        }
        self.cache.insert(key, (Instant::now(), servers.clone()));
        servers
    }
}

/// `~/.claude/settings.json` + `~/.claude.json` の設定済みサーバーに、`cwd` が
/// 与えられていれば `<cwd>/.mcp.json` (プロジェクトスコープの MCP 設定) の分も
/// 足し込む。ソート済み・重複排除済み。
pub fn configured_for_cwd(cwd: Option<&str>) -> Vec<String> {
    let mut servers = configured_mcp_servers_set();
    if let Some(cwd) = cwd {
        collect_mcp_servers_from_file(&Path::new(cwd).join(".mcp.json"), &mut servers);
    }
    servers.into_iter().collect()
}

/// `~/.claude/settings.json` + `~/.claude.json` から設定済み MCP サーバー名を集める
/// (`cwd` なし版。`kikimimi query unused-mcp` の「configured」側そのもの)。
pub fn configured_mcp_servers() -> Vec<String> {
    configured_mcp_servers_set().into_iter().collect()
}

/// `servers` (空でなければ) をソート済み JSON 配列文字列にする — Event の
/// `configured_mcp_servers` 列そのままの形式。空リストは None (推定で埋めない —
/// 原則 7)。
pub fn configured_mcp_servers_json(servers: &[String]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    serde_json::to_string(servers).ok()
}

fn configured_mcp_servers_set() -> BTreeSet<String> {
    let mut servers = BTreeSet::new();
    collect_mcp_servers_from_file(&crate::claude_settings::settings_path(), &mut servers);
    collect_mcp_servers_from_file(&claude_json_path(), &mut servers);
    servers
}

/// `~/.claude.json` の場所。テスト用に `KIKIMIMI_CLAUDE_JSON_PATH` で上書きできる
/// (本番の既定は常に `$HOME/.claude.json`)。`claude_settings::settings_path()` が
/// `~/.claude/settings.json` 側の同じパターンを持つ。
fn claude_json_path() -> PathBuf {
    if let Ok(p) = std::env::var("KIKIMIMI_CLAUDE_JSON_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claude.json")
}

fn collect_mcp_servers_from_file(path: &Path, out: &mut BTreeSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        return;
    };
    collect_mcp_server_keys(&json, out);
}

/// トップレベルの `mcpServers` オブジェクトのキーに加えて、`~/.claude.json` の
/// 実際のレイアウトである `projects.<path>.mcpServers` のキーも集める
/// (プロジェクトごとの MCP 設定はここにしか出てこない)。`<cwd>/.mcp.json` も
/// トップレベル `mcpServers` を見るだけなので同じ関数で読める。
pub fn collect_mcp_server_keys(json: &Value, out: &mut BTreeSet<String>) {
    if let Some(obj) = json.get("mcpServers").and_then(Value::as_object) {
        out.extend(obj.keys().cloned());
    }
    if let Some(projects) = json.get("projects").and_then(Value::as_object) {
        for project in projects.values() {
            if let Some(obj) = project.get("mcpServers").and_then(Value::as_object) {
                out.extend(obj.keys().cloned());
            }
        }
    }
}

/// `servers` を DuckDB の `VARCHAR[]` リテラルにレンダリングする
/// (例: `['a','b']::VARCHAR[]`、空なら `[]::VARCHAR[]`)。単一引用符は 2 個に
/// 倍化してエスケープする (`{glob}` と同じ方式)。`query_cmd.rs`/`web_query.rs`
/// の `unused-mcp` クエリで使う。
pub fn mcp_configured_sql_list(servers: &[String]) -> String {
    let items: Vec<String> = servers
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect();
    format!("[{}]::VARCHAR[]", items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_mcp_server_keys_finds_top_level_and_per_project_entries() {
        let json = serde_json::json!({
            "mcpServers": { "github": {}, "linear": {} },
            "projects": {
                "/home/me/proj-a": { "mcpServers": { "notion": {} } },
                "/home/me/proj-b": { "otherField": true },
                "/home/me/proj-c": { "mcpServers": { "github": {} } }
            }
        });
        let mut out = BTreeSet::new();
        collect_mcp_server_keys(&json, &mut out);
        assert_eq!(
            out,
            ["github", "linear", "notion"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn collect_mcp_server_keys_tolerates_missing_or_malformed_shapes() {
        let mut out = BTreeSet::new();
        collect_mcp_server_keys(&serde_json::json!({}), &mut out);
        assert!(out.is_empty());

        collect_mcp_server_keys(
            &serde_json::json!({"mcpServers": "not-an-object"}),
            &mut out,
        );
        assert!(out.is_empty());

        collect_mcp_server_keys(
            &serde_json::json!({"projects": {"p": {"mcpServers": null}}}),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn configured_mcp_servers_reads_both_files_defensively() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let claude_json_path = dir.path().join("claude.json");

        // Neither file exists yet: must yield an empty list, not an error.
        std::env::set_var("KIKIMIMI_CLAUDE_SETTINGS_PATH", &settings_path);
        std::env::set_var("KIKIMIMI_CLAUDE_JSON_PATH", &claude_json_path);
        assert!(configured_mcp_servers().is_empty());

        // settings.json has a top-level mcpServers; claude.json is malformed JSON
        // (must be skipped, not fail the whole call).
        std::fs::write(
            &settings_path,
            serde_json::json!({"mcpServers": {"github": {}}}).to_string(),
        )
        .unwrap();
        std::fs::write(&claude_json_path, "{ not valid json").unwrap();
        assert_eq!(configured_mcp_servers(), vec!["github".to_string()]);

        // Now give claude.json a real, per-project mcpServers block too.
        std::fs::write(
            &claude_json_path,
            serde_json::json!({
                "projects": { "/x": { "mcpServers": {"notion": {}} } }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            configured_mcp_servers(),
            vec!["github".to_string(), "notion".to_string()]
        );

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");
    }

    /// `<cwd>/.mcp.json` (プロジェクトスコープ) が `~/.claude*` の分と合算されること。
    #[test]
    #[serial_test::serial]
    fn configured_for_cwd_adds_project_scoped_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let claude_json_path = dir.path().join("claude.json");
        let project_dir = dir.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        std::env::set_var("KIKIMIMI_CLAUDE_SETTINGS_PATH", &settings_path);
        std::env::set_var("KIKIMIMI_CLAUDE_JSON_PATH", &claude_json_path);
        std::fs::write(
            &settings_path,
            serde_json::json!({"mcpServers": {"github": {}}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            project_dir.join(".mcp.json"),
            serde_json::json!({"mcpServers": {"playwright": {}}}).to_string(),
        )
        .unwrap();

        assert_eq!(
            configured_for_cwd(Some(project_dir.to_str().unwrap())),
            vec!["github".to_string(), "playwright".to_string()]
        );
        // Without a cwd, only the ~/.claude* servers show up.
        assert_eq!(configured_for_cwd(None), vec!["github".to_string()]);

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");
    }

    /// `.mcp.json` が存在しない/壊れている cwd でも defensive に空扱いされる。
    #[test]
    #[serial_test::serial]
    fn configured_for_cwd_tolerates_missing_or_malformed_mcp_json() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(
            "KIKIMIMI_CLAUDE_SETTINGS_PATH",
            dir.path().join("no-settings.json"),
        );
        std::env::set_var(
            "KIKIMIMI_CLAUDE_JSON_PATH",
            dir.path().join("no-claude.json"),
        );

        let no_mcp_json = dir.path().join("no-mcp-json-here");
        std::fs::create_dir_all(&no_mcp_json).unwrap();
        assert!(configured_for_cwd(Some(no_mcp_json.to_str().unwrap())).is_empty());

        let malformed = dir.path().join("malformed");
        std::fs::create_dir_all(&malformed).unwrap();
        std::fs::write(malformed.join(".mcp.json"), "{ not valid json").unwrap();
        assert!(configured_for_cwd(Some(malformed.to_str().unwrap())).is_empty());

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");
    }

    #[test]
    fn mcp_configured_sql_list_renders_empty_and_escapes_quotes() {
        assert_eq!(mcp_configured_sql_list(&[]), "[]::VARCHAR[]");
        assert_eq!(
            mcp_configured_sql_list(&["github".to_string()]),
            "['github']::VARCHAR[]"
        );
        assert_eq!(
            mcp_configured_sql_list(&["it's".to_string(), "fine".to_string()]),
            "['it''s', 'fine']::VARCHAR[]"
        );
    }

    #[test]
    fn configured_mcp_servers_json_is_none_when_empty_else_a_sorted_json_array() {
        assert_eq!(configured_mcp_servers_json(&[]), None);
        assert_eq!(
            configured_mcp_servers_json(&["github".to_string(), "playwright".to_string()]),
            Some(r#"["github","playwright"]"#.to_string())
        );
    }

    /// `McpConfigCache` は TTL 内なら同じ `cwd` に対してファイルを読み直さない。
    #[test]
    #[serial_test::serial]
    fn mcp_config_cache_returns_same_value_on_second_call_without_rereading_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(
            "KIKIMIMI_CLAUDE_SETTINGS_PATH",
            dir.path().join("no-settings.json"),
        );
        std::env::set_var(
            "KIKIMIMI_CLAUDE_JSON_PATH",
            dir.path().join("no-claude.json"),
        );
        let project_dir = dir.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(".mcp.json"),
            serde_json::json!({"mcpServers": {"github": {}}}).to_string(),
        )
        .unwrap();

        let mut cache = McpConfigCache::default();
        let cwd = project_dir.to_str().unwrap();
        let first = cache.get(cwd);
        assert_eq!(first, vec!["github".to_string()]);

        // Prove the second call comes from the cache, not a fresh read.
        std::fs::remove_file(project_dir.join(".mcp.json")).unwrap();
        let second = cache.get(cwd);
        assert_eq!(second, first);

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");
    }
}
