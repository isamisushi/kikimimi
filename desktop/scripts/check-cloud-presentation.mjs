// The deployed Cloud uses these stable sidebar slots. Exercise delayed SPA
// mounting, navigation, mobile dismissal, sign-in and OAuth origin isolation.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import puppeteer from 'puppeteer-core';
const script = await readFile(new URL('../src-tauri/src/cloud-presentation.js', import.meta.url), 'utf8');
const browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true, args: ['--no-sandbox'] });
const layout = title => `<aside data-slot="sidebar-header"><div><a href="/overview"><img class="cloud-brand-mark" alt="">kikimimi Cloud</a><button aria-label="Close navigation">Close</button></div><div class="cloud-workspace"><label>Workspace <select class="org-switcher"><option>isami</option></select></label></div></aside><main data-slot="sidebar-inset" style="position:relative"><header><button data-sidebar="trigger" onclick="document.body.dataset.toggled='yes'">Toggle navigation</button><span aria-hidden="true">|</span><span>${title}</span></header><main id="main-content"><div class="page__header"><h1>${title}</h1></div></main></main>`;
try {
  const page = await browser.newPage();
  await page.setRequestInterception(true);
  page.on('request', request => void request.respond({ status: 200, contentType: 'text/html', body: '<html><head></head><body><div id="root"></div></body></html>' }));
  await page.evaluateOnNewDocument(script);
  for (const width of [760, 1200]) {
    await page.setViewport({width, height:820});
    await page.goto('https://kikimimi.dev/overview');
    assert.equal(await page.evaluate(() => window.__KIKIMIMI_DESKTOP__), true);
    for (const title of ['Overview', 'Models', 'Tools', 'MCP', 'Skills', 'Struggles', 'Member usage', 'Sessions', 'Subagents', 'Team', 'AI allocation', 'Devices', 'Billing', 'Router', 'Storage & sharing', 'Your teams & sharing']) {
      await page.$eval('#root', (root, html) => {root.innerHTML=html;}, layout(title));
      assert.equal(await page.$eval('.cloud-brand-mark', el => !!el.getClientRects().length && getComputedStyle(el.closest('a')).display !== 'none'), false);
      assert.equal(await page.$eval('.org-switcher', el => !!el.getClientRects().length), true, 'Cloud owns the actual workspace selector');
      assert.equal(await page.$eval('header > span:last-child', el => getComputedStyle(el).display), 'none');
      const header = await page.$eval('header', el => ({position:getComputedStyle(el).position, height:el.getBoundingClientRect().height}));
      assert.deepEqual(header, {position:'absolute',height:28}, 'no second horizontal title bar');
      await page.click('[data-sidebar="trigger"]');
      assert.equal(await page.evaluate(() => document.body.dataset.toggled), 'yes');
      assert.equal(await page.$eval('[aria-label="Close navigation"]', el => !!el.getClientRects().length), true, 'mobile close stays accessible');
    }
    await page.$eval('#root', root => {root.innerHTML='<div class="login-card__brand"><img class="brand-mark">kikimimi Cloud</div><form><button>Sign in</button></form>';});
    assert.equal(await page.$eval('.login-card__brand', el => getComputedStyle(el).display), 'none');
    assert.equal(await page.$eval('form button', el => !!el.getClientRects().length), true);
  }
  await page.goto('https://github.com/login');
  assert.equal(await page.evaluate(() => window.__KIKIMIMI_DESKTOP__), undefined);
  assert.equal(await page.$('#kikimimi-desktop-presentation'), null, 'OAuth is untouched');
  console.log('PASS Cloud presentation: 16 routes, narrow/wide, delayed mounting, one brand, one workspace selector, one page title, navigation controls and OAuth isolation.');
} finally { await browser.close(); }
