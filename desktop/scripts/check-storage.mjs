import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import puppeteer from 'puppeteer-core';

let mode = 'local', storageError = false, storageRequests = 0;
const server = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://localhost').pathname;
  const json = (value, status = 200) => { res.writeHead(status, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(value)); };
  if (path === '/web/me') return json({
    local: mode === 'local', email: 'test@example.com', github_login: null, operator: false,
    active_org: mode, orgs: [{ slug: mode, name: mode === 'team' ? 'Example team' : 'Test', kind: mode === 'team' ? 'team' : 'personal', role: 'owner' }],
  });
  if (path === '/web/storage') {
    storageRequests++;
    return storageError ? json({ error: 'read failed' }, 500) : json({
      local_path: '/example/events', cloud: { org_slug: 'example', org_kind: 'team', repo_patterns: [], hosted: true }, s3: { url: 's3://example/events' },
    });
  }
  if (path === '/web/source') return json({kind:'local',connection:null,refreshed_at:null,objects:null,error:null,max_bytes:536870912,max_objects:10000});
  if (path === '/web/active-org') return json({ error: 'switch failed' }, 500);
  if (path.startsWith('/web/')) return json({ columns: [], rows: [] });
  try {
    const file = path.startsWith('/assets/') && !path.includes('..') ? path.slice(1) : 'index.html';
    const body = await readFile(new URL(`../../web/dist/${file}`, import.meta.url));
    res.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html');
    res.end(body);
  } catch { res.writeHead(404).end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewport({ width: 1200, height: 820 });
  const open = async () => {
    await page.goto(`http://127.0.0.1:${server.address().port}/storage`);
    await page.waitForSelector('h1');
  };
  await open();
  await page.waitForFunction(() => document.body.textContent.includes('Sending to:'));
  assert.equal(await page.$('.topbar .org-switcher'), null);
  assert.match(await page.$eval('main', el => el.textContent), /Viewing: This machine/);
  assert.match(await page.$eval('main', el => el.textContent), /All repositories are shared/);
  assert.equal(await page.$eval('a[href="https://kikimimi.dev/"]', el => el.referrerPolicy), 'no-referrer');
  if (process.env.STORAGE_SCREENSHOT) await page.screenshot({ path: process.env.STORAGE_SCREENSHOT, fullPage: true });
  await page.setViewport({ width: 760, height: 700 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  storageError = true;
  await open();
  await page.waitForSelector('[role="alert"]');
  assert.match(await page.$eval('main', el => el.textContent), /Sharing status is unknown/);
  assert.doesNotMatch(await page.$eval('main', el => el.textContent), /Not connected/);
  storageError = false;
  await page.click('main button');
  await page.waitForFunction(() => document.body.textContent.includes('Sending to:'));
  for (const cloudMode of ['personal', 'team']) {
    mode = cloudMode;
    const previousRequests = storageRequests;
    await open();
    await page.waitForSelector('.org-switcher');
    assert.equal(storageRequests, previousRequests, 'Cloud must not call a local settings API');
    assert.match(await page.$eval('main', el => el.textContent), /This Cloud session cannot read or change them/);
    assert.equal(await page.$eval('.org-switcher option', el => el.textContent), mode === 'personal' ? 'Personal' : 'Example team');
  }
  assert.deepEqual(errors, []);
  console.log('Storage UI: local, read failure/retry, Personal, team, and narrow layout passed.');
} finally {
  if (browser) await browser.close();
  await new Promise(resolve => server.close(resolve));
}
