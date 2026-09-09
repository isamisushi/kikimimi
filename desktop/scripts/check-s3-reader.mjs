// Real viewer + DuckDB + browser, using a deterministic, read-only AWS CLI fixture.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, writeFile, readFile, mkdir, stat, rm } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import puppeteer from 'puppeteer-core';

const root = await mkdtemp(join(tmpdir(), 'kikimimi-s3-read-'));
const data = join(root, 'state'), bin = join(root, 'bin');
await mkdir(data); await mkdir(bin);
const original = { otlp_port: 12345, s3: { url: 's3://separate-upload-bucket/events', profile: null, endpoint_url: null } };
await writeFile(join(data, 'config.json'), JSON.stringify(original));
const date = new Date().toISOString().slice(0, 10);
const prefix = 'shared/kikimimi.v1/events/';
const objects = [];
for (const [name, id, host] of [['a', 'event-a', 'machine-a'], ['duplicate-a', 'event-a', 'machine-a'], ['b', 'event-b', 'machine-b'], ['c', 'event-c', 'machine-c']]) {
  const path = join(root, `${name}.parquet`);
  const quote = value => value.replaceAll("'", "''");
  execFileSync('duckdb', ['-batch', '-no-stdin', '-c', `COPY (SELECT '${id}' AS event_id, ${Date.now()}::BIGINT AS ts, '${date}' AS dt, '${host}' AS host_id, '${host}-session' AS session_id, 'claude-code' AS agent, 'hook' AS source, 'tool.call' AS event_type, 'Bash' AS tool_name, 'mcp' AS tool_kind, 'shared-mcp' AS mcp_server, '["shared-mcp","unused-shared-mcp"]' AS configured_mcp_servers, 10::BIGINT AS input_tokens, 2::BIGINT AS output_tokens) TO '${quote(path)}' (FORMAT PARQUET)`], { stdio: 'pipe' });
  objects.push({ Key: `${prefix}dt=${date}/${name}.parquet`, ETag: `"${name}"`, Size: (await stat(path)).size, path });
}
await writeFile(join(root, 'objects.json'), JSON.stringify(objects));
await writeFile(join(root, 'mode'), 'normal');
const fixture = `#!${process.execPath}
const fs = require('node:fs'), path = require('node:path');
const root = path.dirname(__dirname), args = process.argv.slice(2);
const mode = fs.readFileSync(path.join(root, 'mode'), 'utf8');
const objects = JSON.parse(fs.readFileSync(path.join(root, 'objects.json'), 'utf8'));
const value = name => args[args.indexOf(name) + 1];
if (args[0] !== 's3api' || !['list-objects-v2','get-object'].includes(args[1])) process.exit(90);
fs.appendFileSync(path.join(root, 'calls'), args[1] + '\\n');
if (mode === 'fail') { process.stderr.write('fixture-private-diagnostic'); process.exit(1); }
if (args[1] === 'list-objects-v2') {
  if (value('--bucket') !== 'team-bucket' || value('--prefix') !== '${prefix}' || !args.includes('--no-paginate')) process.exit(91);
  if (mode === 'unsafe') { console.log(JSON.stringify({Contents:[{Key:'${prefix}dt=${date}/../../escape.parquet',Size:20,ETag:'"bad"'}],IsTruncated:false})); process.exit(0); }
  if (mode === 'oversize') { console.log(JSON.stringify({Contents:[{Key:'${prefix}dt=${date}/large.parquet',Size:536870913,ETag:'"large"'}],IsTruncated:false})); process.exit(0); }
  if (mode === 'empty') { console.log(JSON.stringify({IsTruncated:false})); process.exit(0); }
  const rows = mode === 'updated' ? objects.slice(2) : objects.slice(0,3);
  const second = args.includes('--continuation-token');
  console.log(JSON.stringify({ Contents:(second ? rows.slice(1) : rows.slice(0,1)).map(({path,...o})=>o), IsTruncated:!second, ...(second ? {} : {NextContinuationToken:'page-two'}) }));
} else {
  const object = objects.find(o => o.Key === value('--key'));
  if (!object || value('--if-match') !== object.ETag) process.exit(92);
  fs.copyFileSync(object.path, args[args.indexOf('--if-match') + 2]);
  console.log('{}');
}
`;
await writeFile(join(bin, 'aws'), fixture, { mode: 0o700 });
const cli = process.env.KIKIMIMI_BINARY || resolve(import.meta.dirname, '../../target/debug/kikimimi');
const standalone = process.env.S3_READER_STANDALONE === '1';
const child = spawn(cli, standalone ? ['web', '--read-only'] : ['desktop', 'serve-dashboard'], { env: { ...process.env, KIKIMIMI_DIR: data, PATH: `${bin}:${process.env.PATH}` }, stdio: ['pipe', 'pipe', 'pipe'] });
child.stderr.resume();
const lines = createInterface({ input: child.stdout });
let timer, browser;
try {
  const [line] = await Promise.race([once(lines, 'line'), new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Viewer startup timeout')), 10000); })]);
  clearTimeout(timer);
  const url = new URL(standalone ? line.replace(/^Open /, '') : JSON.parse(line).url); // Never print this authenticated URL.
  assert.equal((await fetch(new URL('/web/source', url))).status, 401);
  assert.equal((await fetch(new URL('/web/source/refresh', url), { method: 'POST' })).status, 401);
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : '/usr/bin/google-chrome'), headless: true, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewport({ width: 1200, height: 900 });
  await page.goto(url.href);
  await page.waitForSelector('.topbar a[href="/storage"]');
  await page.click('.topbar a[href="/storage"]');
  await page.waitForSelector('#read-s3-url');
  await page.type('#read-s3-url', 's3://team-bucket/shared');
  await page.click('.source-form button[type="submit"], .source-form button:not([type])');
  await page.waitForSelector('.s3-snapshot-bar', { timeout: 30000 });
  await page.waitForFunction(() => document.querySelector('.app-main')?.textContent.includes('machine-a'));
  const request = (path, method = 'GET') => page.evaluate(async ({path,method}) => {
    const response = await fetch(path, { method });
    return { status: response.status, body: await response.json() };
  }, { path, method });
  const overview = await request('/web/q/overview');
  assert.equal(overview.status, 200);
  assert.equal(overview.body.rows.reduce((sum, row) => sum + row[1], 0), 2, 'duplicate exports must not double-count');
  const machines = await request('/web/q/machines');
  assert.deepEqual(machines.body.rows.map(row => row[0]).sort(), ['machine-a','machine-b']);
  for (const endpoint of ['tools','mcp','skills','unused-mcp','unused-skills','sessions','patterns','subagents','coverage','models','session?session_id=machine-a-session','pattern-hits?pattern_id=tool-thrash&subject=Bash','pattern-timeline?pattern_id=tool-thrash&subject=Bash']) {
    const response = await request(`/web/q/${endpoint}`);
    assert.equal(response.status, 200, `S3 query ${endpoint}: ${JSON.stringify(response.body)}`);
  }
  const mcp = await request('/web/q/unused-mcp');
  assert.deepEqual(mcp.body.rows.map(row=>row[0]).sort(), ['shared-mcp','unused-shared-mcp']);
  assert.deepEqual((await request('/web/usage')).body, []);
  assert.deepEqual((await request('/web/marks?pattern_id=x&subject=y')).body, { marks: [] });
  assert.equal((await request('/web/marks', 'POST')).status, 403);
  const before = (await request('/web/source')).body;
  await writeFile(join(root, 'mode'), 'fail');
  const failed = await request('/web/source/refresh', 'POST');
  assert.equal(failed.status, 502);
  assert.equal(JSON.stringify(failed.body).includes('fixture-private-diagnostic'), false);
  assert.equal((await request('/web/source')).body.refreshed_at, before.refreshed_at);
  assert.deepEqual((await request('/web/q/machines')).body, machines.body);
  await writeFile(join(root, 'mode'), 'unsafe');
  assert.equal((await request('/web/source/refresh', 'POST')).status, 502);
  await writeFile(join(root, 'mode'), 'oversize');
  assert.equal((await request('/web/source/refresh', 'POST')).status, 502);
  await writeFile(join(root, 'mode'), 'updated');
  assert.equal((await request('/web/source/refresh', 'POST')).status, 200);
  assert.deepEqual((await request('/web/q/machines')).body.rows.map(row=>row[0]).sort(), ['machine-b','machine-c'], 'removed S3 objects must disappear from the new snapshot');
  const gets = (await readFile(join(root,'calls'),'utf8')).trim().split('\n').filter(call => call === 'get-object');
  assert.equal(gets.length, 4, 'unchanged object B must be reused on refresh');
  const saved = JSON.parse(await readFile(join(data, 'config.json'), 'utf8'));
  assert.equal(saved.otlp_port, original.otlp_port);
  assert.deepEqual(saved.s3, original.s3, 'reading must not change the upload destination');
  assert.equal(saved.s3_reader.url, 's3://team-bucket/shared');
  if (process.env.S3_SCREENSHOT) { await page.reload(); await page.waitForFunction(() => document.querySelector('.app-main')?.textContent.includes('machine-c')); await page.screenshot({path:process.env.S3_SCREENSHOT,fullPage:true}); }
  await writeFile(join(root, 'mode'), 'empty');
  assert.equal((await request('/web/source/refresh', 'POST')).status, 200);
  assert.deepEqual((await request('/web/q/overview')).body.rows, []);
  await page.goto(new URL('/storage', url).href);
  await page.waitForSelector('.source-form');
  await page.evaluate(() => [...document.querySelectorAll('button')].find(b=>b.textContent==='View this machine instead').click());
  await page.waitForFunction(() => document.querySelector('.app-main h1')?.textContent === 'Overview' && !document.querySelector('.s3-snapshot-bar'));
  assert.deepEqual((await request('/web/q/overview')).body.rows, [], 'local view must not include S3 data');
  assert.deepEqual(errors, []);
  console.log('PASS: real viewer and DuckDB read two S3 machines, deduplicate, query every analysis, isolate local settings, preserve failed snapshots, reject unsafe keys, reuse files and switch back to local. AWS CLI fixture is read-only.');
} finally {
  await browser?.close(); clearTimeout(timer); lines.close();
  const exited = once(child, 'exit');
  if (standalone) child.kill('SIGINT'); else child.stdin.end();
  const stopTimer = setTimeout(() => child.kill('SIGKILL'), 5000);
  if (child.exitCode === null) await exited;
  clearTimeout(stopTimer);
  await rm(root, { recursive: true, force: true });
}
