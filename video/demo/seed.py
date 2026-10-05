"""Generate isolated kikimimi.v1 Parquet from a reproducible synthetic scenario."""
import datetime as dt
import json
import os
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
STATE = HERE / '.state'
scenario = json.loads((HERE / 'scenario.json').read_text())
# Relative to today so real application date filters keep working on recapture.
anchor = dt.date.fromisoformat(os.environ.get('DEMO_DATE', dt.date.today().isoformat()))
fields = re.findall(r'pub (\w+): (?:Option<)?(String|i64|f64|bool)>?,',
                    (ROOT / 'crates/schema/src/lib.rs').read_text().split('pub mod event_type')[0])
types = {'String': 'VARCHAR', 'i64': 'BIGINT', 'f64': 'DOUBLE', 'bool': 'BOOLEAN'}
events = []
for day in range(scenario['days']):
    date = anchor - dt.timedelta(days=scenario['days'] - 1 - day)
    for member_no, member in enumerate(scenario['members']):
        sid = f"demo-{member['id']}-{date.isoformat()}"
        base_ts = int(dt.datetime.combine(date, dt.time(1, member_no * 15), dt.timezone.utc).timestamp() * 1000)
        sequence = 0

        def emit(kind, **extra):
            global sequence
            sequence += 1
            event = dict.fromkeys(name for name, _ in fields)
            event.update(event_id=f'{sid}-{sequence:03}', ts=base_ts + sequence * 10000,
                         dt=date.isoformat(), org_id='demo-team', team_id='demo-team',
                         user_id=member['id'], user_id_source='account', host_id=f"{member['id']}-laptop",
                         env_kind='laptop', os='macos', agent='claude-code', agent_version='demo',
                         session_id=sid, repo='demo/checkout-app', source='log', event_type=kind,
                         query_source='main', redaction_applied=True)
            event.update(extra)
            events.append(event)

        emit('session.start', configured_mcp_servers=json.dumps(scenario['configuredMcp']),
             configured_skills=json.dumps(scenario['configuredSkills']))
        for _ in range(6):
            emit('api.request', model=scenario['mainModel'], effort='high', provider='anthropic',
                 input_tokens=8000 + day * 200, output_tokens=800, cache_read_tokens=12000,
                 cache_write_tokens=0, cost_usd=0.05, usage_source='log')
        for tool, count, kind, server in [
            ('Read', 6, 'builtin', None), ('Edit', 3, 'builtin', None), ('Bash', 3, 'bash', None),
            ('mcp__github__search_code', 2, 'mcp', 'github'),
            ('mcp__playwright__browser_navigate', 2, 'mcp', 'playwright'),
            ('Skill', 1, 'skill', None),
        ]:
            for call in range(count):
                attrs = dict(tool_name=tool, tool_kind=kind, mcp_server=server,
                             skill_name='code-review' if kind == 'skill' else None,
                             correlation_key=f'{sid}-{tool}-{call}', source='hook')
                emit('tool.call', **attrs)
                failed = server == 'playwright' and member['id'] == 'ren' and day >= 4
                emit('tool.result', **attrs, success=not failed,
                     duration_ms=30000 if failed else (900 if server else 40),
                     error_type='timeout' if failed else None)
        agent_attrs = dict(agent_id=f'{sid}-review', agent_type='code-review', query_source='subagent')
        emit('subagent.start', **agent_attrs)
        opus = member['id'] == 'ren'
        for _ in range(4 if opus else 2):
            emit('api.request', **agent_attrs, model=member['subagentModel'], effort='high' if opus else 'low',
                 provider='anthropic', input_tokens=40000 if opus else 3000, output_tokens=2000 if opus else 300,
                 cache_read_tokens=4000, cache_write_tokens=0,
                 cost_usd=1.1 if opus else 0.008, usage_source='log')
        emit('subagent.stop', **agent_attrs, duration_ms=60000 if opus else 30000)
        emit('session.end')

STATE.mkdir(exist_ok=True)
source = STATE / 'events.jsonl'
source.write_text(''.join(json.dumps(e) + '\n' for e in events))
(HERE / 'fixtures').mkdir(exist_ok=True)
(HERE / 'fixtures/events.jsonl').write_text(source.read_text())
sql_quote = lambda s: "'" + str(s).replace("'", "''") + "'"
columns = '{' + ','.join(f"'{name}':'{types[typ]}'" for name, typ in fields) + '}'
read = f"read_json({sql_quote(source)}, format='newline_delimited', columns={columns})"
for scope in ['personal', 'team']:
    directory = STATE / scope / 'data/events' / f'dt={anchor.isoformat()}'
    directory.mkdir(parents=True, exist_ok=True)
    out = directory / 'demo.parquet'
    # Delete only previous generated demo partitions in this script's dedicated state.
    for old in (STATE / scope / 'data/events').glob('dt=*/demo.parquet'):
        old.unlink()
    condition = "WHERE user_id = 'aki'" if scope == 'personal' else ''
    subprocess.run(['duckdb', '-c', f"COPY (SELECT * FROM {read} {condition}) TO {sql_quote(out)} (FORMAT PARQUET);"], check=True)
(STATE / 'claude-settings.json').write_text(json.dumps({'mcpServers': {s: {} for s in scenario['configuredMcp']}}))
(STATE / 'claude.json').write_text('{}')
(STATE / 'manifest.json').write_text(json.dumps({'anchor': anchor.isoformat(), 'events': len(events), 'sessions': 21,
    'expected': {'playwrightCalls': 42, 'playwrightFailures': 6, 'codeReviewCalls': 21,
                 'unusedMcp': 'notion', 'unusedSkill': 'release-notes', 'opusSubagentShare': 1}}, indent=2))
print(f'Seeded {len(events)} synthetic events / 21 sessions / 3 members under {STATE}')
