// Local recording harness: real built SPA + real Rust/DuckDB query handlers.
// Workspace/auth is simulated; it never connects to production/cloud accounts.
import http from 'node:http';
import {spawn, execFile} from 'node:child_process';
import {readFile, writeFile, mkdir} from 'node:fs/promises';
import {resolve, dirname, extname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {promisify} from 'node:util';

const exec = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const state = resolve(here, '.state');
const binary = process.env.DEMO_CLI || resolve(root, 'target/debug/kikimimi');
const scenario = JSON.parse(await readFile(resolve(here, 'scenario.json'), 'utf8'));
const manifest = JSON.parse(await readFile(resolve(state, 'manifest.json'), 'utf8'));
const children = [];
export const cliEnv = scope => ({...process.env,
  KIKIMIMI_DIR: resolve(state, scope),
  KIKIMIMI_CLAUDE_SETTINGS_PATH: resolve(state, 'claude-settings.json'),
  KIKIMIMI_CLAUDE_JSON_PATH: resolve(state, 'claude.json'),
  NO_COLOR: '1',
});

async function viewer(scope) {
  return new Promise((resolveViewer, reject) => {
    const child = spawn(binary, ['web', '--read-only'], {env: cliEnv(scope), stdio: ['ignore', 'pipe', 'pipe']});
    children.push(child);
    const timeout = setTimeout(() => reject(new Error('Viewer startup timed out')), 15000);
    let output = '';
    child.stdout.on('data', chunk => {
      output += chunk;
      const match = output.match(/http:\/\/127\.0\.0\.1:(\d+)\/\?t=([a-f0-9]+)/);
      if (match) {clearTimeout(timeout); resolveViewer({origin: `http://127.0.0.1:${match[1]}`, cookie: `kikimimi_local=${match[2]}`});}
    });
    child.stderr.on('data', chunk => process.stderr.write(chunk));
    child.once('error', error => {clearTimeout(timeout); reject(error);});
    child.once('exit', code => {clearTimeout(timeout); if (!output.includes('?t=')) reject(new Error(`Viewer exited ${code}`));});
  });
}

// The cloud member query itself, run on the exact same team fixture in DuckDB.
const cloudSource = await readFile(resolve(root, 'crates/cloud/src/web_query_sql.rs'), 'utf8');
const memberSql = cloudSource.match(/pub const MEMBERS_SQL: &str = r#"([\s\S]*?)"#;/)[1]
  .replaceAll('$1', `'${manifest.anchor.slice(0, 4)}-01-01'`)
  .replaceAll('FROM events', `FROM read_parquet('${resolve(state, 'team/data/events/dt=*/*.parquet').replaceAll("'", "''")}', union_by_name=true, hive_partitioning=false)`);
const memberColumns = ['user_id', 'sessions', 'api_requests', 'tool_calls', 'tool_failures', 'input_tokens', 'output_tokens', 'cache_read_tokens', 'cost_usd', 'loop_suspect_sessions'];
const members = async () => {
  const {stdout} = await exec('duckdb', ['-json', '-c', memberSql]);
  return {columns: memberColumns, rows: JSON.parse(stdout).map(row => memberColumns.map(k => row[k]))};
};
const viewers = {personal: await viewer('personal'), team: await viewer('team')};
const transcripts = resolve(state, 'transcripts');
await mkdir(transcripts, {recursive: true});

function json(res, body, status = 200) {res.writeHead(status, {'content-type': 'application/json'}); res.end(JSON.stringify(body));}
async function body(req) {let text = ''; for await (const chunk of req) {text += chunk; if (text.length > 4096) throw new Error('Request too large');} return JSON.parse(text || '{}');}
const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, 'http://localhost');
    const scope = /(?:^|;\s*)demo_scope=team(?:;|$)/.test(req.headers.cookie || '') ? 'team' : 'personal';
    if (url.pathname === '/web/me') return json(res, {
      local: false, subscription_usage: false, email: 'aki@example.com', github_login: null, operator: false,
      orgs: [{slug: 'personal', name: 'Personal', kind: 'personal', role: 'owner'},
        {slug: 'demo-team', name: scenario.team.name, kind: 'team', role: 'admin'}],
      active_org: scope === 'team' ? 'demo-team' : 'personal',
    });
    if (url.pathname === '/web/active-org' && req.method === 'POST') {
      const {slug} = await body(req);
      if (!['personal', 'demo-team'].includes(slug)) return json(res, {error: 'unknown demo workspace'}, 400);
      res.setHeader('set-cookie', `demo_scope=${slug === 'demo-team' ? 'team' : 'personal'}; HttpOnly; Path=/; SameSite=Strict`);
      return json(res, {active_org: slug});
    }
    if (url.pathname === '/web/orgs/demo-team/members') return json(res, {members: scenario.members.map(m => ({
      account_id: m.id, email: m.email, github_login: null, role: m.id === 'aki' ? 'admin' : 'member', created_at: `${manifest.anchor}T00:00:00Z`,
    }))});
    if (url.pathname === '/web/q/members') return json(res, await members());
    if (url.pathname.startsWith('/web/')) {
      if (req.method !== 'GET') return json(res, {error: 'Demo viewer is read-only'}, 405);
      const backend = viewers[scope];
      const response = await fetch(`${backend.origin}${url.pathname}${url.search}`, {headers: {cookie: backend.cookie}});
      res.writeHead(response.status, {'content-type': response.headers.get('content-type') || 'application/json'});
      return res.end(Buffer.from(await response.arrayBuffer()));
    }
    if (url.pathname === '/demo/run' && req.method === 'POST') {
      const {command} = await body(req);
      const match = /^kikimimi query (tools|unused-mcp|skills|subagents)$/.exec(command || '');
      if (!match) return json(res, {error: 'Use one of the documented demo query commands.'}, 400);
      const start = performance.now();
      const {stdout, stderr} = await exec(binary, ['query', match[1]], {env: cliEnv('team'), timeout: 20000, maxBuffer: 1024 * 1024});
      const result = {command, stdout, stderr, elapsedMs: Math.round(performance.now() - start), exitCode: 0};
      await writeFile(resolve(transcripts, `${match[1]}.json`), JSON.stringify(result, null, 2));
      return json(res, result);
    }
    if (url.pathname === '/terminal') {
      res.writeHead(200, {'content-type': 'text/html; charset=utf-8'});
      return res.end(await readFile(resolve(here, 'terminal.html')));
    }
    const dist = resolve(root, 'web/dist');
    const requested = resolve(dist, '.' + decodeURIComponent(url.pathname));
    if (!requested.startsWith(dist + '/') && requested !== dist) return json(res, {error: 'bad path'}, 400);
    const path = extname(requested) ? requested : resolve(dist, 'index.html');
    const type = {'.html':'text/html', '.js':'text/javascript', '.css':'text/css', '.svg':'image/svg+xml', '.png':'image/png'}[extname(path)] || 'application/octet-stream';
    res.writeHead(200, {'content-type': type}); res.end(await readFile(path));
  } catch (error) {console.error(error.message); if (!res.headersSent) json(res, {error: error.message}, 500); else res.end();}
});
server.listen(5186, '127.0.0.1', () => console.log('Demo: http://127.0.0.1:5186 (synthetic data only)'));
const stop = () => {server.close(); children.forEach(child => child.kill('SIGTERM')); setTimeout(() => process.exit(0), 250).unref();};
process.on('SIGINT', stop); process.on('SIGTERM', stop);
