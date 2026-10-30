// Exercises real browser interaction with a mocked native bridge. Native child
// webview placement and the history server are tested separately on macOS.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import puppeteer from 'puppeteer-core';

const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  const file = path === '/' ? 'index.html' : path.slice(1);
  if (file.includes('..')) { response.writeHead(404).end(); return; }
  try {
    response.setHeader('Content-Type', file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : file.endsWith('.svg') ? 'image/svg+xml' : 'text/html');
    response.end(await readFile(new URL(`../dist/${file}`, import.meta.url)));
  } catch { response.writeHead(404).end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await puppeteer.launch({ executablePath: process.env.CHROME_BINARY || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : '/usr/bin/google-chrome'), headless: true, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  await page.setViewport({ width: 1200, height: 820 });
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.evaluateOnNewDocument(() => {
    const scenario = new URL(location.href).searchParams.get('scenario');
    const status = {
      collection_target: {cloud:null,s3:null}, applied_collection_target:{cloud:null,s3:null},
      running: scenario !== 'first' && scenario !== 'stopped' && scenario !== 'disconnected',
      service_installed: scenario !== 'first' && scenario !== 'disconnected',
      has_history: scenario !== 'first', duckdb_available: true,
      last_event_ts: Date.now() - 30000,
    };
    const updates = { configured: true, current_version: '0.7.1', available_version: null, auto_update: localStorage.getItem('auto') !== 'false', phase: 'idle', error: null };
    const codexRouter = { codex_found: true, logged_in: true, org_slug: 'test-team', org_kind: 'team', enabled: false, applied: false, base_url: null };
    window.__test = { status, updates, codexRouter, calls: [], fail: null, hold: null, events: {}, signedOut: scenario === 'signed-out-cloud' || scenario === 'email-return', emailReturn: scenario === 'email-return' };
    window.__TAURI__ = {
      event: { listen: (name, handler) => { window.__test.events[name] = handler; } },
      core: { invoke: async (command, args) => {
        window.__test.calls.push(command);
        if (command === 'take_email_return') { const pending = window.__test.emailReturn; window.__test.emailReturn = false; return pending; }
        if (command === 'history_request' || command === 'show_cloud') window.__test.openedPage = {command, page: args?.page};
        if (command === 'history_request') window.__test.reader = args?.source || 'local';
        if (window.__test.hold === command) await new Promise((resolve) => { window.__test.release = resolve; });
        if (window.__test.fail === command) {
          if (command === 'install_update') { updates.phase = 'error'; updates.error = 'Signature verification failed'; }
          throw new Error('Test failure');
        }
        if (command === 'history_request') {
          window.__test.openedPage = {command:'history_request'};
          window.__test.reader = args.source;
          if (args.path.startsWith('/web/q/models')) return {status:200,body:JSON.stringify({models:{columns:[],rows:[]},daily:{columns:[],rows:[]}})};
          return {status:200,body:JSON.stringify(args.path==='/web/me'?{local:true,data_source:args.source,subscription_usage:true}:args.path==='/web/usage'?[{agent:'codex',account:'049533a4-ac22-4a27-8548-e80e7bff4963',observed_at:Date.now(),windows:[{name:'codex/primary',used_percent:12,window_minutes:10080,resets_at:Math.floor(Date.now()/1000)+86400}]}]:{columns:[],rows:[]})};
        }
        if (command === 'cloud_workspaces') return window.__test.signedOut ? null : { orgs: [{slug:'me', name:'me (personal)', kind:'personal'}, {slug:'test-team', name:'Test team', kind:'team'}], active_org: localStorage.getItem('test-org') || 'me' };
        if (command === 'switch_collection') {
          window.__test.collectionSelection = args.selection;
          const next = args.selection.kind === 'cloud' ? {cloud:{org_slug:args.selection.slug,org_kind:args.selection.slug === 'me' ? 'personal' : 'team',hosted:true,repo_patterns:args.selection.repo_patterns},s3:null} : args.selection.kind === 's3' ? {cloud:null,s3:args.selection.s3} : {cloud:null,s3:null};
          status.collection_target = next;
          if (!window.__test.deferCollection) status.applied_collection_target = next;
          return;
        }
        if (command === 'select_cloud_workspace') { localStorage.setItem('test-org', args.slug); return; }
        if (command === 'read_storage') return {local_path:'/test/history', cloud:null, s3:null};
        if (command === 'read_source') return { kind: localStorage.getItem('test-source') || 'local', connection: null };
        if (command === 'set_source') { window.__test.sourceSelection = args.selection; localStorage.setItem('test-source', args.selection.kind); return; }
        if (command === 'status') return { ...status };
        if (command === 'codex_router_status') return { ...window.__test.codexRouter };
        if (command === 'set_codex_router') { window.__test.codexRouter.enabled = args.enabled; window.__test.codexRouter.applied = args.enabled; return; }
        if (command === 'update_status') return { ...updates };
        if (command === 'enable' || command === 'resume') { status.running = true; status.service_installed = true; status.has_history = true; }
        if (command === 'disconnect') { status.running = false; status.service_installed = false; }
        if (command === 'set_auto_update') { updates.auto_update = args.enabled; localStorage.setItem('auto', String(args.enabled)); }
        if (command === 'check_updates') { updates.phase = 'available'; updates.available_version = '0.7.2'; }
        if (command === 'install_update') updates.phase = 'restarting';
      } },
    };
  });
  page.setDefaultTimeout(10000);
  const open = async (scenario='running') => {
    await page.goto(`http://127.0.0.1:${server.address().port}/?scenario=${scenario}`);
    await page.waitForFunction(()=>document.getElementById('loading-view').hidden);
  };
  const visible=id=>page.$eval(`#${id}`,el=>!!el.getClientRects().length);
  const calls=()=>page.evaluate(()=>window.__test.calls);
  const noWrites=async()=>assert.equal((await calls()).some(c=>['set_source','select_cloud_workspace','switch_collection','enable','resume'].includes(c)),false);
  await open();
  await page.waitForSelector('.subscription-account');
  assert.equal(await visible('settings-view'),false);
  assert.equal(await page.$eval('#nav-local',el=>el.disabled),true);
  assert.deepEqual(await page.$eval('#nav-local',el=>({title:el.getAttribute('title'),pointer:getComputedStyle(el).pointerEvents,opacity:getComputedStyle(el).opacity})),{title:null,pointer:'none',opacity:'1'});
  const before=await calls();await page.click('#nav-local');assert.deepEqual(await calls(),before);
  const content=await page.$('.history-host .app-shell');
  await page.click('#nav-settings');await page.waitForSelector('#settings-view:not([hidden])');
  assert.equal(await visible('collection-change'), false, 'collection settings do not repeat their own shortcut');
  await page.click('#nav-local');
  assert.equal(await content.evaluate(el=>el.isConnected),true,'same React history tree survives tab changes');
  await noWrites();
  // Slow Cloud command cannot replace settings or reopen the local document.
  await page.evaluate(()=>{window.__test.hold='show_cloud';});
  await page.click('#nav-dashboard');
  await page.waitForFunction(()=>!!window.__test.release);
  assert.equal(await visible('workspace-control'), false, 'Cloud has one workspace picker owned by its own session');
  await page.click('#nav-settings');
  await page.evaluate(()=>{window.__test.hold=null;window.__test.release();window.__test.events['cloud-load-error']();});
  assert.equal(await visible('settings-view'),true);
  await noWrites();
  // Every animation frame during local tab switches has only the selected view.
  await page.evaluate(()=>{window.__frames=[];let n=0;const sample=()=>{window.__frames.push({settings:!document.querySelector('#settings-view').hidden,history:!document.querySelector('.history-host').hidden,advanced:!document.querySelector('#workspace-view').hidden});if(++n<120)requestAnimationFrame(sample);};requestAnimationFrame(sample);});
  for(let n=0;n<8;n++){await page.click('#nav-local');await page.click('#nav-settings');}
  assert.equal(await page.evaluate(()=>window.__frames.some(f=>(f.settings&&f.history)||f.advanced)),false);
  // Update and provider controls preserve server truth after failure.
  await page.click('#settings-general');await page.click('#check-updates');
  await page.waitForSelector('#available-update:not([hidden])');
  await page.click('#auto-update');await page.waitForFunction(()=>!document.querySelector('#auto-update').checked);
  await page.evaluate(()=>{window.__test.fail='set_auto_update';});await page.click('#auto-update');
  await page.waitForFunction(()=>!document.querySelector('#auto-update').checked);
  await page.evaluate(()=>{window.__test.fail=null;});
  await page.click('#settings-codex');await page.click('#codex-router');
  await page.waitForFunction(()=>window.__test.codexRouter.enabled);
  await page.evaluate(()=>{window.__test.fail='set_codex_router';});await page.click('#codex-router');
  await page.waitForFunction(()=>document.querySelector('#codex-router').checked);
  await page.evaluate(()=>{window.__test.fail=null;});
  // Collection is a separate explicit confirmation, including team scope.
  await page.click('#settings-collection');await page.select('#collection-destination','cloud:test-team');await page.click('#review-collection');
  await page.waitForFunction(()=>document.querySelector('#collection-dialog').open);
  assert.equal((await calls()).includes('switch_collection'),false);
  await page.click('#collection-consent');await page.click('#confirm-collection');
  assert.equal((await calls()).includes('switch_collection'),false,'team needs a repository scope');
  await page.type('#collection-repos','github.com/example/*');await page.click('#confirm-collection');
  await page.waitForFunction(()=>!document.querySelector('#collection-dialog').open);
  assert.deepEqual(await page.evaluate(()=>window.__test.collectionSelection.repo_patterns),['github.com/example/*']);
  assert.equal(await page.evaluate(()=>window.__test.collectionSelection.expected.cloud),null);
  // Explicit S3 connection changes only reading, never collection.
  await page.click('#settings-workspace');await page.click('#workspace-connection-settings');
  await page.select('#data-source','s3');await page.type('#source-url','s3://example/shared');await page.click('#apply-source');
  await page.waitForFunction(()=>window.__test.reader==='s3');
  const switches=(await calls()).filter(c=>c==='switch_collection').length;
  await page.click('#nav-local');await page.waitForFunction(()=>window.__test.reader==='local');
  assert.equal((await calls()).filter(c=>c==='switch_collection').length,switches);
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('workspaces-v1')).preferences.personal.url),'s3://example/shared');
  // Narrow/wide layout and long account IDs; actual built CSS, not a fixture page.
  await mkdir('/tmp/kikimimi-react-e2e',{recursive:true});
  for(const width of [760,1200]){
    await page.setViewport({width,height:820});await page.waitForSelector('.subscription-account');
    for (const route of ['overview','models','tools','mcp','skills','patterns','sessions','subagents']) {
      await page.click(`.history-host a[href="/${route}"]`);
      await page.waitForSelector('.history-host h1');
      assert.equal(await page.$$eval('.brand-mark', els => els.filter(el => el.getClientRects().length).length), 1, `${route}: only the app header owns branding`);
      assert.equal(await page.$eval(`.history-host a[href="/${route}"]`, el => el.getAttribute('aria-current')), 'page');
      assert.notEqual(await page.$eval(`.history-host a[href="/${route}"]`, el => getComputedStyle(el).pointerEvents), 'none', 'selected parent navigation must still return from detail to the list');
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false, `${route}: no window overflow`);
      await page.screenshot({path:`/tmp/kikimimi-react-e2e/${route}-${width}.png`});
    }
    await page.click('.history-host a[href="/overview"]');
    await page.waitForSelector('.subscription-account');
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    const overlap=await page.$eval('.subscription-toolbar',el=>{const a=el.querySelector('label').getBoundingClientRect(),b=el.querySelector('button').getBoundingClientRect();return a.right>b.left&&a.bottom>b.top&&b.bottom>a.top;});
    assert.equal(overlap,false);
    const headingTop=await page.$eval('.history-host h1',el=>el.getBoundingClientRect().top);assert.ok(headingTop>=116&&headingTop<200,'history starts directly below fixed header');
    await page.screenshot({path:`/tmp/kikimimi-react-e2e/history-${width}.png`});
    await page.click('#nav-settings');await page.screenshot({path:`/tmp/kikimimi-react-e2e/settings-${width}.png`});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    const settingsTop=await page.$eval('#settings-title',el=>el.getBoundingClientRect().top);assert.ok(settingsTop>=116&&settingsTop<200,'settings does not add another toolbar offset');
    await page.click('#settings-workspace');await page.waitForSelector('#workspace-view:not([hidden])');
    assert.equal(await visible('workspace-control'), true, 'advanced connections retain their own selector');
    assert.equal(await page.$eval('#source-label', el => el.textContent.trim()), 'S3', 'saved source is not mislabeled by the currently viewed local history');
    await page.screenshot({path:`/tmp/kikimimi-react-e2e/data-sources-${width}.png`});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    await page.click('#workspace-connection-settings');await page.waitForSelector('#connection-view:not([hidden])');
    await page.screenshot({path:`/tmp/kikimimi-react-e2e/connection-${width}.png`});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    await page.click('#nav-local');
  }
  await page.$eval('.shell-main',el=>{el.scrollTop=400;});
  await page.click('.history-host a[href="/mcp"]');
  assert.equal(await page.$eval('.shell-main',el=>el.scrollTop),0,'new history page starts at its heading');
  // First setup does not collect until consent; browsing is always available.
  await page.evaluate(()=>localStorage.clear());await open('first');
  assert.equal(await visible('onboarding-view'),true);
  assert.equal(await page.$eval('#enable',el=>el.disabled),true);
  await page.click('#browse-only');await page.waitForSelector('.history-host:not([hidden])');
  assert.equal((await calls()).includes('enable'),false);
  await page.evaluate(()=>localStorage.clear());await open('first');
  await page.click('#consent');await page.click('#enable');
  await page.waitForSelector('.history-host:not([hidden])');
  assert.equal((await calls()).filter(c=>c==='enable').length,1);
  await page.click('#nav-settings');await page.click('#disconnect');
  await page.waitForFunction(()=>document.querySelector('#disconnect-dialog').open);
  await page.keyboard.press('Escape');assert.equal((await calls()).includes('disconnect'),false);
  await page.click('#disconnect');await page.click('#confirm-disconnect');
  await page.waitForFunction(()=>!window.__test.status.running);
  await page.click('#nav-local');assert.equal(await visible('settings-view'),false);
  assert.deepEqual(errors,[]);
  console.log('React desktop integration passed: lifecycle, tab retention, delayed Cloud, settings, consent, destinations, layout.');
} finally {await browser?.close();server.close();}
