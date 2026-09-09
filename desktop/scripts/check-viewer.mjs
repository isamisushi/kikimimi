// Native CLI integration: no collector, hooks, or login service is started.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import puppeteer from 'puppeteer-core';

const directory = await mkdtemp(join(tmpdir(), 'kikimimi-viewer-test-'));
const cli = process.env.KIKIMIMI_BINARY || resolve(import.meta.dirname, '../../target/debug/kikimimi');
const child = spawn(cli, ['desktop', 'serve-dashboard'], {
  env: { ...process.env, KIKIMIMI_DIR: directory }, stdio: ['pipe', 'pipe', 'pipe'],
});
const exited = once(child, 'exit');
// Never print the startup line or its authentication token.
child.stderr.resume();
const lines = createInterface({ input: child.stdout });
let timeout;
let browser;
try {
  const [line] = await Promise.race([
    once(lines, 'line'),
    new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error('Viewer did not start')), 10000); }),
  ]);
  clearTimeout(timeout);
  const url = new URL(JSON.parse(line).url);
  assert.equal(url.hostname, '127.0.0.1');
  assert.notEqual(url.port, '');
  const unauthenticated = await fetch(new URL('/web/q/overview', url));
  assert.equal(unauthenticated.status, 401);
  const entry = await fetch(url, { redirect: 'manual' });
  assert.equal(entry.status, 302);
  const cookie = entry.headers.get('set-cookie').split(';')[0];
  const response = await fetch(new URL('/web/q/overview', url), { headers: { cookie } });
  assert.equal(response.status, 200);
  assert.deepEqual((await response.json()).rows, []);
  const app = await fetch(new URL('/', url), { headers: { cookie } });
  assert.equal(app.status, 200);
  assert.match(await app.text(), /kikimimi/i);
  const me = await fetch(new URL('/web/me', url), { headers: { cookie } });
  assert.equal((await me.json()).local, true);
  const storage = await fetch(new URL('/web/storage', url), { headers: { cookie } });
  assert.equal(storage.status, 200);
  const destinations = await storage.json();
  assert.equal(destinations.cloud, null);
  assert.equal(destinations.s3, null);
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : '/usr/bin/google-chrome'), headless: true, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  await page.goto(url.href);
  await page.waitForSelector('.app-main h1');
  assert.equal(await page.$eval('.app-main h1', (el) => el.textContent), 'Overview');
  const navigation = await page.$eval('.topbar', (el) => el.textContent);
  assert.equal(navigation.includes('Log out'), false);
  assert.equal(await page.$('.topbar a[href="/members"]'), null);
  assert.equal(await page.$('.topbar a[href="/team"]'), null);
  assert.equal(await page.$('.topbar a[href="/devices"]'), null);
  await page.click('.topbar a[href="/storage"]');
  await page.waitForFunction(() => document.body.textContent.includes('No Cloud or S3 destination is configured.'));
  const deviceRequests = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/web/devices')) deviceRequests.push(request.url());
  });
  await page.goto(new URL('/devices', url).href);
  await page.waitForSelector('.app-main h1');
  assert.match(await page.$eval('.app-main', (el) => el.textContent), /Device registration is available in the cloud dashboard/);
  assert.deepEqual(deviceRequests, []);
  await page.click('.app-main a[href="/"]');
  await page.waitForFunction(() => document.querySelector('.app-main h1')?.textContent === 'Overview');
  await browser.close();
  browser = null;
  // History browsing alone must not create collection configuration or state.
  assert.deepEqual(await readdir(directory), []);
  child.stdin.end();
  const [code] = await Promise.race([
    exited,
    new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error('Viewer outlived its owning pipe')), 10000); }),
  ]);
  clearTimeout(timeout);
  assert.equal(code, 0);
  console.log('PASS: standalone history viewer authenticates requests, serves the dashboard with collection off, leaves collection files untouched, and exits when its owner closes.');
} finally {
  await browser?.close();
  clearTimeout(timeout);
  if (child.exitCode === null) child.kill('SIGKILL');
  lines.close();
  await rm(directory, { recursive: true, force: true });
}
