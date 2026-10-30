// Real collector -> Parquet -> DuckDB -> viewer -> browser. Only producer logs
// are fixtures; no account credentials, agent settings or login services change.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, appendFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { createServer } from 'node:net';
import puppeteer from 'puppeteer-core';

// macOS AF_UNIX paths are limited to 104 bytes, including the socket basename.
const root = await mkdtemp(join(process.platform === 'darwin' ? '/tmp' : tmpdir(), 'kk-e2e-'));
const cli = process.env.KIKIMIMI_BINARY || resolve(import.meta.dirname, '../../target/debug/kikimimi');
const state = join(root, 'state'), codex = join(root, 'codex'), claude = join(root, 'claude');
const cowork = join(root, 'cowork/account/org/local_test/.claude/projects/project');
const server = createServer();
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;
await new Promise(resolve => server.close(resolve));
for (const dir of [state, join(codex, 'sessions'), join(claude, 'project'), cowork]) await mkdir(dir, { recursive: true });
const env = {
  ...process.env, PATH: `${dirname(cli)}:${process.env.PATH}`,
  KIKIMIMI_DIR: state, XDG_RUNTIME_DIR: join(root, 'runtime'),
  CODEX_HOME: codex, KIKIMIMI_CLAUDE_PROJECTS_DIR: claude,
  KIKIMIMI_COWORK_SESSIONS_DIR: join(root, 'cowork'),
  KIKIMIMI_NO_CLAUDE_BACKFILL: '1', KIKIMIMI_OTLP_PORT: String(port),
};
await mkdir(env.XDG_RUNTIME_DIR);
const children = [];
let browser;
const run = (args, input) => execFileSync(cli, args, { env, input, encoding: 'utf8', timeout: 15000, stdio: ['pipe', 'pipe', 'pipe'] });
const until = async (fn, label) => {
  const end = Date.now() + 20000;
  while (Date.now() < end) {
    if (await fn()) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out: ${label}`);
};
const launch = (args, stdin = false) => {
  const child = spawn(cli, args, { env, stdio: [stdin ? 'pipe' : 'ignore', 'pipe', 'pipe'] });
  children.push(child); child.stderr.resume(); return child;
};
const start = async () => {
  const agent = launch(['agent', '--foreground']); agent.stdout.resume();
  await until(() => { try { return run(['flush']).startsWith('flush ok'); } catch { return false; } }, 'collector readiness');
  return agent;
};
const stop = async child => {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exit = once(child, 'exit');
  child.kill('SIGTERM');
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000).unref();
  try { await exit; } finally { clearTimeout(timer); }
};
const ts = offset => new Date(Date.now() + offset).toISOString();
const coworkOffset = -180 * 24 * 60 * 60 * 1000;
const record = (type, id, offset, extra = {}) => JSON.stringify({ type, sessionId: id, timestamp: ts(offset), ...extra }) + '\n';
const sql = query => JSON.parse(execFileSync('duckdb', ['-no-agent', '-json', '-c', query], { env, encoding: 'utf8' }));
const glob = join(state, 'data/events/dt=*/*.parquet').replaceAll("'", "''");

try {
  // Application labels must be observed, including Claude Desktop's entrypoint.
  for (const [id, entrypoint] of [['claude-cli', 'cli'], ['claude-desktop', 'claude-desktop']]) {
    await writeFile(join(claude, `project/${id}.jsonl`), record('user', id, 0, { entrypoint }));
  }
  const path = join(cowork, 'cowork.jsonl');
  await writeFile(path,
    record('user', 'cowork', coworkOffset, { uuid: 'user', message: { content: 'PRIVATE-PROMPT' } }) +
    record('assistant', 'cowork', coworkOffset + 100, { uuid: 'assistant', requestId: 'request', message: { id: 'message', model: 'claude-test', usage: { input_tokens: 9, output_tokens: 4 }, content: [{ type: 'tool_use', id: 'call', name: 'Bash', input: { command: 'PRIVATE-COMMAND' } }] } }),
  );
  let agent = await start();
  for (const [id, originator] of [['codex-cli', 'codex-tui'], ['codex-desktop', 'Codex Desktop']]) {
    await writeFile(join(codex, `sessions/rollout-${id}.jsonl`), JSON.stringify({ timestamp: ts(0), type: 'session_meta', payload: { id, session_id: id, originator } }) + '\n');
  }
  for (const id of ['claude-cli', 'claude-desktop']) {
    run(['hook', 'SessionStart'], JSON.stringify({ session_id: id, hook_event_name: 'SessionStart' }));
  }
  // A stop-only subagent has no observed duration. It must not display zero.
  run(['hook', 'SubagentStop'], JSON.stringify({ session_id: 'claude-cli', agent_id: 'stop-only', hook_event_name: 'SubagentStop' }));
  await until(() => {
    run(['flush']);
    const rows = sql(`SELECT count(DISTINCT session_id) n FROM read_parquet('${glob}', union_by_name=true)`);
    return rows[0].n === 5;
  }, 'all five producer types');
  const storedClients = sql(`SELECT session_id, max(client) AS client FROM read_parquet('${glob}', union_by_name=true) WHERE session_id IS NOT NULL GROUP BY session_id`);
  for (const [id, client] of [['claude-cli', 'claude_code_cli'], ['claude-desktop', 'claude_desktop_code'], ['cowork', 'claude_desktop_cowork'], ['codex-cli', 'codex_cli'], ['codex-desktop', 'codex_desktop']]) {
    assert.equal(storedClients.find(row => row.session_id === id)?.client, client, 'application provenance is saved, not just inferred by the viewer');
  }
  const viewer = launch(['desktop', 'serve-dashboard'], true);
  const lines = createInterface({ input: viewer.stdout });
  const [line] = await Promise.race([once(lines, 'line'), new Promise((_, reject) => setTimeout(() => reject(new Error('Viewer startup timeout')), 15000).unref())]);
  const url = new URL(JSON.parse(line).url); // Never print the private authentication URL.
  assert.equal((await fetch(new URL('/web/q/sessions', url))).status, 401);
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true, args: ['--no-sandbox'] });
  const page = await browser.newPage(), errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.goto(url.href);
  await page.waitForSelector('a[href="/sessions"]');
  await page.click('a[href="/sessions"]');
  await page.waitForSelector('select[aria-label="Period"]');
  await page.select('select[aria-label="Period"]', '365');
  for (const label of ['Claude Code CLI', 'Claude Desktop · Code', 'Claude Desktop · Cowork', 'Codex CLI', 'Codex Desktop']) {
    await page.waitForFunction(label => document.querySelector('table')?.textContent.includes(label), {}, label);
  }
  const request = path => page.evaluate(async path => { const r = await fetch(path); if (!r.ok) throw new Error('Query failed'); return r.json(); }, path);
  await page.select('select[aria-label="Application"]', 'claude_desktop_cowork');
  await page.waitForFunction(() => document.querySelector('table')?.textContent.includes('Claude Desktop · Cowork'));
  assert.equal(await page.$$eval('tbody tr', rows => rows.length), 1);
  await page.click('a[href="/sessions/cowork"]');
  await page.waitForSelector('h1');
  assert.equal(await page.$eval('a[href="/sessions"]', el => el.getAttribute('aria-current')), 'page', 'detail keeps Sessions selected');
  await page.click('a[href="/sessions"]');
  await page.waitForSelector('select[aria-label="Period"]');
  assert.deepEqual(await page.$$eval('.sessions-filters select', els => els.map(el => el.value)), ['365', 'claude_desktop_cowork'], 'returning from detail preserves filters');
  await page.waitForSelector('tbody tr');
  assert.equal(await page.$$eval('tbody tr', rows => rows.length), 1);
  let sessions = await request('/web/q/sessions?days=365&limit=500');
  const clientIndex = sessions.columns.indexOf('client');
  assert.ok(clientIndex > 0);
  assert.equal(sessions.rows.find(r => r[0] === 'cowork')[clientIndex], 'claude_desktop_cowork');
  let detail = await request('/web/q/session?session_id=cowork');
  assert.equal(detail.summary.rows[0][20], 9);
  assert.equal(detail.summary.rows[0][21], 4);
  assert.equal(detail.summary.rows[0][8], false, 'an open transcript is not a completed session');
  await page.click('a[href="/subagents"]');
  await page.waitForFunction(() => document.querySelector('main')?.textContent.includes('unknown'));
  const subagents = await request('/web/q/subagents');
  assert.equal(subagents.rows[0][subagents.columns.indexOf('subagent_duration_ms')], null);
  const stoppedDetail = await request('/web/q/session?session_id=claude-cli');
  assert.equal(stoppedDetail.subagents.rows[0][4], null);

  // A line split across writes must appear once, after the writer finishes it.
  const result = record('user', 'cowork', coworkOffset + 200, { message: { content: [{ type: 'tool_result', tool_use_id: 'call', content: 'PRIVATE-OUTPUT', is_error: false }] } });
  const before = sql(`SELECT count(*) n FROM read_parquet('${glob}', union_by_name=true) WHERE session_id='cowork'`)[0].n;
  await appendFile(path, result.slice(0, result.length / 2)); run(['flush']);
  assert.equal(sql(`SELECT count(*) n FROM read_parquet('${glob}', union_by_name=true) WHERE session_id='cowork'`)[0].n, before);
  await appendFile(path, result.slice(result.length / 2)); run(['flush']);
  await until(() => {
    run(['flush']);
    return sql(`SELECT count(*) n FROM read_parquet('${glob}', union_by_name=true) WHERE session_id='cowork' AND event_type='tool.result'`)[0].n === 1;
  }, 'live Cowork tool result');
  await stop(agent); agent = await start(); run(['flush']);
  assert.equal(sql(`SELECT count(*) n FROM read_parquet('${glob}', union_by_name=true) WHERE session_id='cowork'`)[0].n, before + 1);
  assert.equal(sql(`SELECT count(*) n FROM read_parquet('${glob}', union_by_name=true) WHERE prompt_text IS NOT NULL OR tool_input_json IS NOT NULL OR tool_output_excerpt IS NOT NULL`)[0].n, 0);
  assert.deepEqual(errors, []);
  console.log('PASS: real collector, Parquet, DuckDB and browser: five application labels, Cowork history/live tools/usage, partial writes, restart deduplication, unknown subagent duration, authentication and metadata-only storage.');
} finally {
  if (browser) await browser.close();
  for (const child of children.reverse()) await stop(child);
  await rm(root, { recursive: true, force: true });
}
