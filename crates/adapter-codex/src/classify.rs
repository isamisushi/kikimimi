//! tool_name → tool_kind / mcp_server / mcp_tool の分類 (Codex 版)。
//!
//! `kikimimi-adapter-claude::classify::classify_tool` と役割は同じだが、`Bash` 決め打ちの
//! Claude 版をそのまま使わない: Codex の rollout (実データ, 2026-08-31 実測) では
//! シェル実行ツールの名前は `"exec"` であり (`custom_tool_call.name == "exec"`)、hook 側
//! (Claude Code 互換とはいえ) の実際の tool_name 文字列は本マシンでは確認できていない
//! (§report 参照)。誤って `mcp` として拾わない・逃さないことを優先し、シェル系のエイリアスは
//! 大文字小文字を無視して複数受け付ける (`bash` / `shell` / `exec`)。
//!
//! MCP の `mcp__<server>__<tool>` 命名規則自体は Codex 側でも確認済み (バイナリ文字列:
//! "Plugin-provided MCP tools keep standard MCP identifiers such as `mcp__server__tool`") —
//! これは Claude と共通なので `kikimimi_schema::split_mcp_tool_name` をそのまま使う。

use kikimimi_schema::split_mcp_tool_name;

pub(crate) struct ToolClass {
    pub kind: &'static str,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
}

/// シェル実行ツールのエイリアス (大文字小文字を無視)。実測で確認済みなのは rollout の
/// `"exec"` のみ; `"bash"`/`"shell"` は Claude Code 互換の hook 経由で来るかもしれない
/// 未確認の候補として保守的に含める (実データが増えたら更新する)。
const SHELL_ALIASES: [&str; 3] = ["bash", "shell", "exec"];

/// tool_kind: "mcp" if `mcp__` prefix, "bash" if a known shell alias (大小無視),
/// else "builtin"。Claude 版の "skill"/"browser" マーカーは Codex での実証が無いため、
/// ここでは決め打ちしない (未確認のものは "builtin" のまま = 何も失わない。分類の粒度が
/// 粗くなるだけで、tool_name 自体は Event にそのまま残る)。
pub(crate) fn classify_tool(tool_name: &str) -> ToolClass {
    if let Some((server, tool)) = split_mcp_tool_name(tool_name) {
        return ToolClass {
            kind: "mcp",
            mcp_server: Some(server),
            mcp_tool: Some(tool),
        };
    }
    let lower = tool_name.to_lowercase();
    if SHELL_ALIASES.contains(&lower.as_str()) {
        return ToolClass {
            kind: "bash",
            mcp_server: None,
            mcp_tool: None,
        };
    }
    ToolClass {
        kind: "builtin",
        mcp_server: None,
        mcp_tool: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_takes_priority_and_splits() {
        let c = classify_tool("mcp__ologs__get_profile");
        assert_eq!(c.kind, "mcp");
        assert_eq!(c.mcp_server.as_deref(), Some("ologs"));
        assert_eq!(c.mcp_tool.as_deref(), Some("get_profile"));
    }

    #[test]
    fn shell_aliases_are_case_insensitive() {
        assert_eq!(classify_tool("exec").kind, "bash");
        assert_eq!(classify_tool("Exec").kind, "bash");
        assert_eq!(classify_tool("bash").kind, "bash");
        assert_eq!(classify_tool("Bash").kind, "bash");
        assert_eq!(classify_tool("shell").kind, "bash");
    }

    #[test]
    fn unknown_is_builtin_not_fabricated() {
        assert_eq!(classify_tool("apply_patch").kind, "builtin");
        assert_eq!(classify_tool("Read").kind, "builtin");
    }
}
