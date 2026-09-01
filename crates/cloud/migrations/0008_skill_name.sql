-- kikimimi.v1 additive column: Skill 名 (tool_kind='skill' のとき)。
-- Claude Code hook の tool_input.skill からメタデータとして抽出される (本文は含まない)。
-- 0008 より前に ingest された行は単に NULL のまま。
ALTER TABLE events ADD COLUMN IF NOT EXISTS skill_name TEXT;
