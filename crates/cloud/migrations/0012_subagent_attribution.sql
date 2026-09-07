-- kikimimi.v1 additive columns (KKM-15): サブエージェントのセッション帰属。
--   agent_id     Claude Code のサブエージェント id (hook `agent_id` / transcript `agentId`)。
--                NULL = メイン会話、または id を持たない source (OTel の api.request)。
--   agent_type   サブエージェントの種類名 (hook `agent_type` / OTel `agent.name`)。
--   query_source main | subagent | auxiliary (OTel の `query_source`、hook/log は
--                agent_id の有無から派生)。NULL = 判定材料なし。
-- session_id は親セッションのまま (既存のセッション集計は無変更)。0012 より前に
-- ingested された行は NULL のまま — 「不明」であって「メイン」ではない。
ALTER TABLE events ADD COLUMN IF NOT EXISTS agent_id TEXT;
ALTER TABLE events ADD COLUMN IF NOT EXISTS agent_type TEXT;
ALTER TABLE events ADD COLUMN IF NOT EXISTS query_source TEXT;
