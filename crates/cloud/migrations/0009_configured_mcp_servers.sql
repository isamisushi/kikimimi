-- kikimimi.v1 additive column: 設定済み MCP サーバー名のスナップショット
-- (event_type='session.start' のときのみ埋まる、ソート済み重複排除 JSON 配列文字列。
-- URL/コマンド/引数は含まない、§5.2)。「導入されているのに一度も呼ばれないサーバー」
-- (§7.1, §7.2 unused_mcp_server) を、cloud 側の観測プロキシではなく実際の設定
-- スナップショットから検知できるようにする。
-- 0009 より前に ingested された行は単に NULL のまま。
ALTER TABLE events ADD COLUMN IF NOT EXISTS configured_mcp_servers TEXT;
