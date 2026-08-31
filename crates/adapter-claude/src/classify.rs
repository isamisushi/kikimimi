//! tool_name → tool_kind / mcp_server / mcp_tool の分類。hook・OTel 双方の正規化で共有する。

use kikimimi_schema::split_mcp_tool_name;

pub(crate) struct ToolClass {
    pub kind: &'static str,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
}

/// tool_kind: "mcp" if name starts with mcp__, "bash" if Bash, "skill" if Skill,
/// "browser" if lowercased name contains playwright/browser/chrome/webfetch, else "builtin"。
pub(crate) fn classify_tool(tool_name: &str) -> ToolClass {
    if let Some((server, tool)) = split_mcp_tool_name(tool_name) {
        return ToolClass {
            kind: "mcp",
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
    const BROWSER_MARKERS: [&str; 4] = ["playwright", "browser", "chrome", "webfetch"];
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
        assert_eq!(classify_tool("mcp__playwright__browser_click").kind, "mcp");
    }

    #[test]
    fn unknown_builtin() {
        assert_eq!(classify_tool("Read").kind, "builtin");
    }
}
