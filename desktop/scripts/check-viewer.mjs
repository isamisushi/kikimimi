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
  assert.equal(await page.$('.topbar a[href="/router"]'), null, 'hosted Router stays out of local navigation');
  assert.equal(await page.$('.topbar a[href="/devices"]'), null);
  await page.waitForSelector('#overview-empty-title');
  assert.match(await page.$eval('.overview-empty', el => el.textContent), /Review collection/);
  assert.equal(await page.$('.overview-empty a[href="/__desktop_setup"]'), null, 'plain browsers get working web setup options');
  assert.ok(await page.$('.overview-empty a[href="/storage"]'));
  await page.waitForFunction(() => !document.querySelector('#coverage'));
  await page.waitForSelector('.subscription-empty');
  await page.evaluateOnNewDocument(() => { window.__kikimimiDesktop = true; });
  await page.reload();
  await page.waitForSelector('.overview-empty a[href="/__desktop_setup"]');
  await page.setViewport({ width: 760, height: 600 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'empty-state actions fit the minimum native window width');
  await page.screenshot({ path: '/tmp/kikimimi-empty-dashboard.png', fullPage: true });
  const refresh = await page.$('.overview-empty button');
  await refresh.click();
  await page.waitForSelector('#overview-empty-title');
  let overviewMode = 'error';
  let failSecondary = false;
  await page.setRequestInterception(true);
  page.on('request', request => {
    if (new URL(request.url()).pathname === '/web/usage') {
      return void request.respond({status:200,contentType:'application/json',body:JSON.stringify([{agent:'codex',account:'test-account-with-a-long-identifier-049533a4-ac22-4a27-8548-e80e7bff4963',observed_at:Date.now(),windows:[{name:'primary',used_percent:12,window_minutes:10080,resets_at:Math.floor(Date.now()/1000)+86400}]}])});
    }

    if (failSecondary && ['/web/q/coverage', '/web/q/machines'].includes(new URL(request.url()).pathname)) {
      return void request.respond({ status: 500, contentType: 'application/json', body: '{"error":"Test unavailable"}' });
    }
    if (new URL(request.url()).pathname === '/web/q/overview') {
      if (overviewMode === 'error') return void request.respond({ status: 500, contentType: 'application/json', body: '{"error":"Test unavailable"}' });
      if (overviewMode === 'populated') return void request.respond({ status: 200, contentType: 'application/json', body: JSON.stringify({ rows: [['2026-10-27', 5, 3, 0, 100, 50, 0.01]] }) });
    }
    void request.continue();
  });
  await page.reload();
  await page.waitForSelector('.state-panel--error');
  assert.equal(await page.$('#overview-empty-title'), null, 'errors must not masquerade as empty data');
  assert.equal(await page.$eval('.query-error-details', el => el.open), false, 'raw errors are collapsed');
  await page.waitForSelector('.subscription-toolbar select');
  for (const width of [760, 1200]) {
    await page.setViewport({width,height:900});
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'long account IDs do not overflow');
    const separated = await page.$eval('.subscription-toolbar', el => {
      const label = el.querySelector('label').getBoundingClientRect();
      const button = el.querySelector('button').getBoundingClientRect();
      const card = document.querySelector('.subscription-account').getBoundingClientRect();
      return (button.left >= label.right || button.top >= label.bottom) && card.top > button.bottom;
    });
    assert.equal(separated,true,'controls and account cards have separate space');
    await page.screenshot({path:`/tmp/kikimimi-usage-layout-${width}.png`,fullPage:true});
  }

  overviewMode = 'populated';
  await page.click('.state-panel--error button');
  await page.waitForSelector('.stat-grid');
  assert.equal(await page.$('#overview-empty-title'), null, 'existing activity keeps its normal dashboard');
  failSecondary = true;
  await page.reload();
  await page.waitForFunction(() => document.querySelectorAll('.state-panel--error button').length === 2);
  failSecondary = false;
  for (let remaining = 2; remaining > 0; remaining--) {
    await page.click('.state-panel--error button');
    await page.waitForFunction(count => document.querySelectorAll('.state-panel--error').length < count, {}, remaining);
  }
  assert.equal(await page.$('.state-panel--error'), null, 'secondary panels can recover independently');
  overviewMode = 'empty';
  await page.click('.topbar a[href="/storage"]');
  await page.waitForFunction(() => document.body.textContent.includes('No Cloud or S3 destination is configured.'));
  const deviceRequests = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.startsWith('/web/devices')) deviceRequests.push(request.url());
  });
  for (const route of ['/devices', '/team', '/members', '/billing', '/allocation', '/router', '/login', '/join/test']) {
    await page.goto(new URL(route, url).href);
    await page.waitForSelector('.state-panel--empty');
    assert.match(await page.$eval('.app-main', el => el.textContent), /Page not found/);
    assert.equal(await page.$('input[name=email]'), null);
  }
  assert.deepEqual(deviceRequests, []);
  await page.click('.topbar a[href="/overview"]');
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
