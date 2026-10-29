// Reproduce the phases in the reported recording, including late panel data
// and sidebar resizing after the HTML document has already finished loading.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import puppeteer from 'puppeteer-core';
const script = await readFile(new URL('../src-tauri/src/presentation-readiness.js', import.meta.url), 'utf8');
const browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true, args: ['--no-sandbox'] });
try {
  for (const channel of ['cloud']) {
  const page = await browser.newPage();
  const signals = [];
  await page.setRequestInterception(true);
  page.on('request', request => {
    if (request.url().includes('/__desktop_')) {
      signals.push({url:request.url(), at:Date.now()});
      void request.abort();
    } else void request.respond({status:200, contentType:'text/html', body:'<div id="root"><div class="boot-loading">Loading kikimimi…</div></div>'});
  });
  await page.goto('https://kikimimi.dev/overview');
  await page.evaluate(`(${script})(7, ${JSON.stringify(channel)})`);
  const settle = () => new Promise(resolve => setTimeout(resolve, 250));
  await settle();
  assert.equal(signals.length, 0, 'HTML load must not reveal session loading');
  await page.evaluate(() => {
    document.getElementById('root').innerHTML = '<aside aria-label="Workspace sidebar" style="width:200px">Your teams & sharing</aside><main><h1>Overview</h1><div id="usage" class="state-panel--loading">Loading usage…</div><div id="machines" class="state-panel--loading">Loading machines…</div></main>';
  });
  await settle();
  assert.equal(signals.length, 0, 'partial Overview must stay hidden');
  await page.evaluate(() => document.getElementById('usage').remove());
  await settle();
  assert.equal(signals.length, 0, 'wait for the last panel too');
  await page.evaluate(() => {
    document.getElementById('machines').remove();
    const sidebar = document.querySelector('aside');
    let width = 200;
    window.resizeTest = setInterval(() => { sidebar.style.width = `${width += 10}px`; }, 40);
  });
  await settle();
  assert.equal(signals.length, 0, 'moving sidebar must not be revealed');
  await page.evaluate(() => clearInterval(window.resizeTest));
  for (let i=0; i<20 && !signals.length; i++) await new Promise(resolve => setTimeout(resolve,50));
  assert.equal(signals.length, 1, 'reveal once after the whole layout settles');
  assert.equal(new URL(signals[0].url).pathname, `/__desktop_${channel}_ready`);
  assert.equal(new URL(signals[0].url).searchParams.get('generation'), '7');
  console.log(`PASS ${channel}: session loading, partial data and moving layout stay hidden; reveal once.`);
  await page.close();
  }
} finally { await browser.close(); }
