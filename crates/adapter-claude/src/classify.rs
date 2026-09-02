//! tool_name → tool_kind / mcp_server / mcp_tool の分類。hook・OTel 双方の正規化で共有する。

use kikimimi_schema::split_mcp_tool_name;

pub(crate) struct ToolClass {
    pub kind: &'static str,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
}

/// tool_kind: name が mcp__ で始まる場合は "mcp"。ただし server 名に
/// playwright/browser/chrome/webfetch/puppeteer のいずれかを含む場合は "browser"
/// とする（Playwright MCP・claude-in-chrome MCP はブラウザ操作の「代替チャネル」
/// であり、bypass / thrash(deny_detour) / reach クエリが tool_kind IN
/// ('bash','browser') で拾う対象に含める必要があるため — architecture.md §1.1）。
/// mcp_server / mcp_tool は "browser" 判定時も引き続き設定する。MCP ヘルス・
/// unused-mcp クエリは mcp_server IS NOT NULL で検出するため、この情報は失わない。
/// 非 MCP 名では "Bash" → "bash"、"Skill" → "skill"、上記マーカーを含む名前は
/// "browser"、それ以外は "builtin"。
///
/// 副作用: BYPASS_SQL の mcp_fail CTE は tool_kind='mcp' でフィルタするため、
/// ブラウザ系 MCP の失敗はもう bypass の「起点」として扱われなくなる。これは
/// 意図した変更（ブラウザ系 MCP はそもそも bypass の代替先チャネルであり、
/// 起点にはならない）。
pub(crate) fn classify_tool(tool_name: &str) -> ToolClass {
    const BROWSER_MARKERS: [&str; 5] = ["playwright", "browser", "chrome", "webfetch", "puppeteer"];

    if let Some((server, tool)) = split_mcp_tool_name(tool_name) {
        let server_lower = server.to_lowercase();
        let kind = if BROWSER_MARKERS
            .iter()
            .any(|marker| server_lower.contains(marker))
        {
            "browser"
        } else {
            "mcp"
        };
        return ToolClass {
            kind,
            mcp_server: Some(server),
            mcp_tool: Some(tool),
        };
    }
    if tool_name == "Bash" {
        return ToolClass {
            kind: "bash",
            mcp_server: None,
            mcp_tool: None,
        };
    }
    if tool_name == "Skill" {
        return ToolClass {
            kind: "skill",
            mcp_server: None,
            mcp_tool: None,
        };
    }
    let lower = tool_name.to_lowercase();
    if BROWSER_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return ToolClass {
            kind: "browser",
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
        let c = classify_tool("mcp__github__get_issue");
        assert_eq!(c.kind, "mcp");
        assert_eq!(c.mcp_server.as_deref(), Some("github"));
        assert_eq!(c.mcp_tool.as_deref(), Some("get_issue"));
    }

    #[test]
    fn bash_and_skill() {
        assert_eq!(classify_tool("Bash").kind, "bash");
        assert_eq!(classify_tool("Skill").kind, "skill");
    }

    #[test]
    fn browser_markers_case_insensitive() {
        assert_eq!(classify_tool("WebFetch").kind, "browser");

        let c = classify_tool("mcp__playwright__browser_click");
        assert_eq!(c.kind, "browser");
        assert_eq!(c.mcp_server.as_deref(), Some("playwright"));
        assert_eq!(c.mcp_tool.as_deref(), Some("browser_click"));
    }

    #[test]
    fn browser_mcp_server_case_insensitive_non_playwright() {
        let c = classify_tool("mcp__claude-in-chrome__navigate");
        assert_eq!(c.kind, "browser");
        assert_eq!(c.mcp_server.as_deref(), Some("claude-in-chrome"));
        assert_eq!(c.mcp_tool.as_deref(), Some("navigate"));
    }

    #[test]
    fn non_browser_mcp_stays_mcp() {
        let c = classify_tool("mcp__github__get_issue");
        assert_eq!(c.kind, "mcp");
    }

    #[test]
    fn unknown_builtin() {
        assert_eq!(classify_tool("Read").kind, "builtin");
    }
}
