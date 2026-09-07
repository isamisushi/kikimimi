//! Codex CLI の設定済み MCP サーバー名 (`$CODEX_HOME/config.toml` の
//! `[mcp_servers.<name>]` テーブル) — Claude Code 版 `mcp_config.rs` の Codex 対応
//! (KKM-16)。`session.start` の `configured_mcp_servers` スナップショットに使う。
//!
//! TOML 全体はパースしない: 必要なのはテーブルヘッダの名前だけで、依存を増やさずに
//! `[mcp_servers.foo]` / `[mcp_servers."my server"]` / `[mcp_servers.foo.env]` (サブ
//! テーブル — 先頭セグメントだけ取る) を読めれば足りる。値 (コマンド・URL・環境変数)
//! は §5.2 どおり一切読まない。
//!
//! 形式は OpenAI の Codex 設定ドキュメント (`mcp_servers` テーブル) と、この端末の
//! codex-cli 0.151.0 バイナリ内の設定キー文字列 `mcp_servers` で確認 (2026-09-07)。
//! この端末には MCP サーバーが設定されていないため、実ファイルでの検証はテストの
//! 合成 TOML のみ。

use std::collections::BTreeSet;
use std::path::Path;

/// `<codex_home>/config.toml` に書かれた MCP サーバー名 (ソート済み・重複排除)。
/// ファイルが無い/読めない場合は空 (エラーにしない — Codex 未使用マシンが普通)。
pub fn configured_mcp_servers(codex_home: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(codex_home.join("config.toml")) else {
        return Vec::new();
    };
    parse_mcp_server_names(&text).into_iter().collect()
}

/// テーブルヘッダ行から `mcp_servers.` 直下のキーを集める。
pub fn parse_mcp_server_names(toml_text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for raw in toml_text.lines() {
        let line = raw.trim();
        // `[a.b]` or `[[a.b]]`; comments after the header are fine.
        let Some(inner) = line.strip_prefix('[') else {
            continue;
        };
        let inner = inner.strip_prefix('[').unwrap_or(inner);
        let Some(end) = inner.find(']') else {
            continue;
        };
        let header = inner[..end].trim();
        let Some(rest) = header.strip_prefix("mcp_servers") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('.') else {
            continue; // `[mcp_servers]` itself, or `[mcp_servers_other]`
        };
        if let Some(name) = first_key_segment(rest.trim_start()) {
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

/// `foo.env` → `foo`, `"my server".env` → `my server`, `'x'` → `x`.
fn first_key_segment(s: &str) -> Option<String> {
    let mut chars = s.chars();
    match chars.next()? {
        q @ ('"' | '\'') => {
            let rest: &str = &s[1..];
            let end = rest.find(q)?;
            Some(rest[..end].to_string())
        }
        _ => Some(s.split(['.', ' ', '\t']).next().unwrap_or("").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_quoted_and_nested_headers_and_ignores_values() {
        let toml = r#"
model = "gpt-5"
[mcp_servers]            # bare table: no server
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
[mcp_servers.github.env]
GITHUB_TOKEN = "secret"  # never read
  [ mcp_servers."my server" ]   # quoted, padded
url = "http://localhost:1234"
[[mcp_servers.arr]]
[mcp_servers_other.nope]
[other.mcp_servers.x]
"#;
        let names: Vec<String> = parse_mcp_server_names(toml).into_iter().collect();
        assert_eq!(names, vec!["arr", "github", "my server"]);
    }

    #[test]
    fn missing_config_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(configured_mcp_servers(dir.path()).is_empty());
        std::fs::write(dir.path().join("config.toml"), "[mcp_servers.jira]\n").unwrap();
        assert_eq!(configured_mcp_servers(dir.path()), vec!["jira".to_string()]);
    }
}
