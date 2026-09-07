-- kikimimi.v1 additive column (KKM-18): セッションで読み込まれていたスキル名の
-- ソート済み JSON 配列文字列。transcript backfill の session.end に付く
-- (`skill_listing` attachment 由来)。同じ理由で transcript 由来の
-- configured_mcp_servers も session.end に付くようになった (`deferred_tools_delta`
-- の MCP ツール名から)。0013 より前の行は NULL のまま。
ALTER TABLE events ADD COLUMN IF NOT EXISTS configured_skills TEXT;
