// Browser integration test with a mocked native bridge. This does not claim to
// exercise macOS installation, signature verification, or the real update server.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import puppeteer from 'puppeteer-core';

const server = createServer(async (request, response) => {
  const files = { '/': ['index.html', 'text/html'], '/app.js': ['app.js', 'text/javascript'], '/style.css': ['style.css', 'text/css'] };
  const file = files[request.url];
  if (!file) { response.writeHead(404).end(); return; }
  response.setHeader('Content-Type', file[1]);
  response.end(await readFile(new URL(`../ui/${file[0]}`, import.meta.url)));
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : '/usr/bin/google-chrome'), headless: true, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.evaluateOnNewDocument(() => {
    const state = {
      configured: true, current_version: '0.7.1', available_version: null,
      auto_update: localStorage.getItem('auto') !== 'false', phase: 'idle', error: null,
    };
    window.__test = { state, calls: [], fail: false };
    window.__TAURI__ = { core: { invoke: async (command, args) => {
      window.__test.calls.push(command);
      if (command === 'status') return { running: true, dashboard_url: 'http://127.0.0.1:1234', duckdb_available: true, service_installed: true };
      if (command === 'update_status') return { ...state };
      if (command === 'set_auto_update') { state.auto_update = args.enabled; localStorage.setItem('auto', String(args.enabled)); return; }
      if (command === 'check_updates') { state.phase = 'available'; state.available_version = '0.7.2'; return; }
      if (command === 'install_update') {
        if (window.__test.fail) { state.phase = 'error'; state.error = 'Signature verification failed'; throw new Error(state.error); }
        state.phase = 'restarting'; return;
      }
    } } };
  });
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  await page.waitForFunction(() => !document.getElementById('auto-update').disabled);
  assert.equal(await page.$eval('#auto-update', (element) => element.checked), true);
  await page.click('#auto-update');
  await page.waitForFunction(() => localStorage.getItem('auto') === 'false');
  await page.reload();
  await page.waitForFunction(() => !document.getElementById('auto-update').disabled);
  assert.equal(await page.$eval('#auto-update', (element) => element.checked), false);
  await page.click('#check-updates');
  await page.waitForFunction(() => !document.getElementById('install-update').disabled);
  assert.match(await page.$eval('#update-status', (element) => element.textContent), /0\.7\.2/);
  await page.evaluate(() => { window.__test.fail = true; });
  await page.click('#install-update');
  await page.waitForFunction(() => document.getElementById('update-status').textContent.includes('Signature verification failed'));
  await page.evaluate(() => { window.__test.state.configured = false; });
  await page.waitForFunction(() => document.getElementById('check-updates').disabled);
  assert.equal(await page.$eval('#install-update', (element) => element.disabled), true);
  await page.evaluate(() => { window.__test.state.configured = true; window.__test.state.error = null; window.__test.state.phase = 'downloading'; });
  await page.waitForFunction(() => document.getElementById('update-status').textContent.includes('Downloading'));
  assert.equal(await page.$eval('#enable', (element) => element.disabled), true);
  assert.equal(await page.$eval('#disconnect', (element) => element.disabled), true);
  assert.deepEqual(errors, []);
  console.log('PASS: browser update controls, preference persistence, update availability, failure display, disabled unsigned builds, and operation locking (native bridge mocked).');
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
