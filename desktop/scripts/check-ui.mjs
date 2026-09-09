// Exercises real browser interaction with a mocked native bridge. Native child
// webview placement and the history server are tested separately on macOS.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import puppeteer from 'puppeteer-core';

const server = createServer(async (request, response) => {
  const files = { '/': ['index.html', 'text/html'], '/app.js': ['app.js', 'text/javascript'], '/style.css': ['style.css', 'text/css'], '/workspace.css': ['workspace.css', 'text/css'] };
  const file = files[new URL(request.url, 'http://localhost').pathname];
  if (!file) { response.writeHead(404).end(); return; }
  response.setHeader('Content-Type', file[1]);
  response.end(await readFile(new URL(`../ui/${file[0]}`, import.meta.url)));
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
    window.__test = { status, updates, calls: [], fail: null, hold: null, events: {} };
    window.__TAURI__ = {
      event: { listen: (name, handler) => { window.__test.events[name] = handler; } },
      core: { invoke: async (command, args) => {
        window.__test.calls.push(command);
        if (command === 'show_dashboard' || command === 'show_cloud') window.__test.openedPage = {command, page: args?.page};
        if (window.__test.hold === command) await new Promise((resolve) => { window.__test.release = resolve; });
        if (window.__test.fail === command) {
          if (command === 'install_update') { updates.phase = 'error'; updates.error = 'Signature verification failed'; }
          throw new Error('Test failure');
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
        if (command === 'update_status') return { ...updates };
        if (command === 'enable' || command === 'resume') { status.running = true; status.service_installed = true; status.has_history = true; }
        if (command === 'disconnect') { status.running = false; status.service_installed = false; }
        if (command === 'set_auto_update') { updates.auto_update = args.enabled; localStorage.setItem('auto', String(args.enabled)); }
        if (command === 'check_updates') { updates.phase = 'available'; updates.available_version = '0.7.2'; }
        if (command === 'install_update') updates.phase = 'restarting';
      } },
    };
  });
  const open = async (scenario = 'running') => {
    await page.goto(`http://127.0.0.1:${server.address().port}/?scenario=${scenario}`);
    await page.waitForFunction(() => document.getElementById('loading-view').hidden);
  };
  const visible = (id) => page.$eval(`#${id}`, (el) => Boolean(el.getClientRects().length));
  const disabled = (id) => page.$eval(`#${id}`, (el) => el.disabled);
  const text = (id) => page.$eval(`#${id}`, (el) => el.textContent);
  const calls = () => page.evaluate(() => window.__test.calls);
  const screenshot = async (name) => {
    if (!process.env.UI_SCREENSHOTS) return;
    await mkdir(process.env.UI_SCREENSHOTS, { recursive: true });
    await page.screenshot({ path: `${process.env.UI_SCREENSHOTS}/${name}.png`, fullPage: true });
  };

  await open('first');
  await page.click('#browse-only');
  await page.waitForFunction(() => window.__test.calls.includes('show_dashboard'));
  assert.equal((await calls()).includes('enable'), false, 'S3 readers must not need to enable collection');
  assert.equal(await visible('navigation'), true);

  await open('first');
  assert.equal(await visible('onboarding-view'), true);
  assert.equal(await visible('navigation'), false);
  assert.equal(await disabled('enable'), true);
  assert.equal((await calls()).includes('enable'), false);
  await screenshot('onboarding');
  await page.click('#consent');
  await page.evaluate(() => { window.__test.hold = 'enable'; });
  await page.click('#enable');
  await page.waitForFunction(() => Boolean(window.__test.release));
  assert.match(await text('status'), /Setting up/);
  assert.equal(await disabled('enable'), true);
  await page.evaluate(() => { window.__test.hold = null; window.__test.release(); });
  await page.waitForFunction(() => window.__test.calls.includes('show_dashboard'));
  assert.equal(await visible('onboarding-view'), false);
  assert.equal(await visible('status-action'), false);
  // Successful setup stays visible after background status updates.
  await page.waitForFunction(() => window.__test.calls.filter((c) => c === 'status').length >= 3);
  assert.match(await text('status-detail'), /Restart existing agent sessions/);
  await page.click('#status-dismiss');
  assert.match(await text('status-detail'), /Last activity/);

  await open('first');
  await page.click('#consent');
  await page.evaluate(() => { window.__test.fail = 'enable'; });
  await page.click('#enable');
  await page.waitForFunction(() => document.getElementById('message-text').textContent.includes('Could not finish setup'));
  assert.equal(await disabled('enable'), false);
  assert.equal(await visible('onboarding-view'), true);

  await open();
  await page.waitForFunction(() => window.__test.calls.includes('show_dashboard'));
  assert.equal((await calls()).includes('enable'), false);
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-connection-settings');
  await page.waitForFunction(() => window.__test.calls.includes('hide_dashboard'));
  assert.equal(await visible('enable'), false);
  await screenshot('settings');
  await page.select('#data-source', 'cloud');
  await page.click('#apply-source');
  await page.waitForFunction(() => window.__test.calls.includes('show_cloud'));
  assert.equal(await page.$eval('#nav-dashboard', el => el.getAttribute('aria-current')), 'page');
  assert.equal((await calls()).includes('enable'), false);
  await page.reload();
  await page.waitForFunction(() => window.__test.calls.includes('show_cloud'));
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-connection-settings');
  await page.select('#data-source', 'local');
  await page.click('#apply-source');
  await page.waitForFunction(() => window.__test.calls.includes('show_dashboard'));
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-connection-settings');
  await page.select('#data-source', 's3');
  await page.type('#source-url', 's3://test-bucket/shared');
  await page.type('#source-profile', 'test-reader');
  await page.evaluate(() => { window.__test.fail = 'set_source'; });
  await page.click('#apply-source');
  await page.waitForFunction(() => !document.getElementById('source-error').hidden);
  assert.equal(await text('source-label'), 'This Mac');
  await page.evaluate(() => { window.__test.fail = null; });
  await page.click('#apply-source');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'S3');
  assert.deepEqual(await page.evaluate(() => window.__test.sourceSelection), {kind:'s3', url:'s3://test-bucket/shared', profile:'test-reader', endpoint_url:null});
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-connection-settings');
  await page.select('#data-source', 'local');
  await page.click('#apply-source');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'This Mac');
  // Workspace selector survives local mode, failures, and restart.
  assert.equal(await visible('workspace'), true);
  assert.equal(await page.$eval('#workspace', el => el.value), 'personal');
  await page.select('#workspace', 'cloud:test-team');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'Kikimimi Cloud');
  assert.equal(await page.evaluate(() => localStorage.getItem('test-org')), 'test-team');
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-connection-settings');
  await page.waitForFunction(() => !document.getElementById('connection-view').hidden);
  assert.equal(await page.$eval('#data-source option[value=local]', el => el.disabled), true);
  await page.select('#workspace', 'personal');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'This Mac');
  await page.evaluate(() => { window.__test.fail = 'select_cloud_workspace'; });
  await page.select('#workspace', 'cloud:test-team');
  await page.waitForFunction(() => !document.getElementById('workspace').disabled);
  assert.equal(await page.$eval('#workspace', el => el.value), 'personal');
  assert.equal(await text('source-label'), 'This Mac');
  await page.evaluate(() => { window.__test.fail = null; });
  await page.select('#workspace', 's3-team');
  await page.waitForFunction(() => !document.getElementById('connection-view').hidden);
  assert.equal(await page.$eval('#source-url', el => el.value), '');
  assert.equal(await page.$eval('#data-source option[value=local]', el => el.disabled), true);
  await page.click('#nav-dashboard');
  assert.equal(await visible('connection-view'), true, 'unconfigured S3 must never show local history');
  await page.type('#source-url', 's3://team-bucket/shared');
  await page.click('#apply-source');
  await page.waitForFunction(() => document.getElementById('connection-view').hidden);
  await page.reload();
  await page.waitForFunction(() => window.__test.sourceSelection?.url === 's3://team-bucket/shared');
  assert.equal(await page.$eval('#workspace', el => el.value), 's3-team');
  await page.select('#workspace', 'personal');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'This Mac');
  // Viewer failures retain the committed preference; Dashboard retries it.
  await page.evaluate(() => { window.__test.fail = 'show_cloud'; });
  await page.select('#workspace', 'cloud:test-team');
  await page.waitForFunction(() => !document.getElementById('workspace').disabled);
  assert.equal(await text('source-label'), 'This Mac');
  assert.equal(await visible('connection-view'), true);
  await page.evaluate(() => { window.__test.fail = null; });
  await page.click('#nav-dashboard');
  await page.waitForFunction(() => document.getElementById('connection-view').hidden);
  assert.equal(await page.evaluate(() => window.__test.sourceSelection.kind), 'local');
  await page.evaluate(() => { window.__test.signedOut = true; });
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-signin');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'Cloud sign-in');
  assert.equal(await page.$eval('#workspace', el => el.value), 'personal');
  await page.select('#workspace', 'personal');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'This Mac');
  await page.evaluate(() => { window.__test.signedOut = false; });
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-signin');
  await page.waitForFunction(() => [...document.getElementById('workspace').options].some(o => o.value === 'cloud:test-team'));
  await page.click('#nav-dashboard');
  // Collection belongs to this Mac, independently of the workspace being viewed.
  assert.equal(await visible('workspace-collecting'), true);
  await page.select('#workspace', 'cloud:test-team');
  await page.waitForFunction(() => !document.getElementById('workspace').disabled);
  assert.equal(await visible('workspace-collecting'), false);
  assert.equal(await visible('collection-change'), true);
  assert.equal((await calls()).includes('switch_collection'), false);
  await page.click('#collection-change');
  await page.waitForFunction(() => !document.getElementById('settings-view').hidden);
  assert.equal(await page.$eval('#collection-destination', el => el.value), 'cloud:test-team');
  await page.click('#review-collection');
  await page.click('#collection-consent');
  await page.click('#confirm-collection');
  assert.equal(await visible('collection-dialog-error'), true, 'team sharing requires explicit scope');
  await page.type('#collection-repos', 'github.com/test-team/*');
  await page.evaluate(() => { window.__test.fail = 'switch_collection'; });
  await page.click('#confirm-collection');
  await page.waitForFunction(() => !document.getElementById('confirm-collection').disabled);
  assert.equal(await visible('workspace-collecting'), false);
  await page.evaluate(() => { window.__test.fail = null; window.__test.deferCollection = true; });
  await page.click('#confirm-collection');
  await page.waitForFunction(() => !document.getElementById('collection-dialog').open);
  assert.equal(await visible('workspace-collecting'), false, 'saved config must not masquerade as applied');
  await page.evaluate(() => { window.__test.status.applied_collection_target = window.__test.status.collection_target; });
  await page.waitForFunction(() => !document.getElementById('workspace-collecting').hidden);
  assert.deepEqual(await page.evaluate(() => window.__test.collectionSelection.repo_patterns), ['github.com/test-team/*']);
  await page.select('#collection-destination', 'local');
  await page.click('#review-collection');
  await page.click('#cancel-collection');
  assert.equal(await page.evaluate(() => window.__test.status.collection_target.cloud.org_slug), 'test-team');
  await page.evaluate(() => { window.__test.status.collection_target = {cloud:null,s3:null}; window.__test.status.applied_collection_target = {cloud:null,s3:null}; });
  await page.select('#workspace', 'personal');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'This Mac');
  await page.click('#nav-settings');
  await page.waitForFunction(() => document.getElementById('upload-status').textContent === 'Configured destinations for this Mac');
  assert.equal(await visible('settings-view'), true);
  assert.equal(await visible('data-source'), false, 'App Settings must not contain workspace viewing controls');
  assert.match(await text('upload-destinations'), /Not configured/);
  await page.evaluate(() => { window.__test.fail = 'read_storage'; });
  await page.click('#refresh-storage');
  await page.waitForFunction(() => document.getElementById('upload-status').textContent.includes('unknown'));
  assert.equal(await visible('upload-destinations'), false, 'failed read must not show stale destinations');
  await page.evaluate(() => { window.__test.fail = null; });
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  assert.match(await text('workspace-name'), /Personal/);
  assert.equal(await visible('workspace-context'), true);
  assert.equal(await page.$eval('#nav-dashboard', el => el.getAttribute('aria-current')), 'page', 'workspace settings belong to Dashboard');
  assert.equal(await page.$eval('#workspace-pages [aria-current=page]', el => el.textContent), 'Workspace Settings');
  assert.equal(await page.$('#workspace-pages [data-page=members]'), null);
  await page.click('#workspace-pages [data-page=models]');
  await page.waitForFunction(() => window.__test.openedPage?.page === 'models');
  assert.deepEqual(await page.evaluate(() => window.__test.openedPage), {command:'show_dashboard',page:'models'});
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);
  await page.click('#workspace-connection-settings');
  assert.equal(await visible('workspace-context'), true, 'data source settings retain workspace tabs');
  assert.equal(await page.$eval('#nav-dashboard', el => el.getAttribute('aria-current')), 'page', 'data source settings belong to Dashboard');
  await page.click('#workspace-pages [data-page=workspace]');
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);

  assert.equal(await visible('disconnect'), false);
  assert.equal(await visible('data-source'), false);
  await page.select('#workspace', 'cloud:test-team');
  await page.waitForFunction(() => document.getElementById('workspace-name').textContent === 'Test team');
  assert.equal(await visible('workspace-view'), true, 'workspace switch stays in Workspace Settings');
  assert.notEqual(await page.$('#workspace-pages [data-page=members]'), null);
  await page.click('#workspace-pages [data-page=members]');
  await page.waitForFunction(() => window.__test.openedPage?.page === 'members');
  assert.deepEqual(await page.evaluate(() => window.__test.openedPage), {command:'show_cloud',page:'members'});
  await page.evaluate(() => window.__test.events.navigate({payload:'workspace'}));
  await page.waitForFunction(() => !document.getElementById('workspace-view').hidden);

  await page.click('#manage-team');
  await page.waitForFunction(() => document.getElementById('workspace-view').hidden);
  await page.select('#workspace', 'personal');
  await page.waitForFunction(() => document.getElementById('source-label').textContent === 'This Mac');
  await page.keyboard.down('Control'); await page.keyboard.press(','); await page.keyboard.up('Control');
  await page.waitForFunction(() => !document.getElementById('settings-view').hidden);
  assert.equal(await visible('connection-view'), false);
  await page.click('#nav-settings');
  await page.click('#auto-update');
  await page.waitForFunction(() => localStorage.getItem('auto') === 'false');
  await open();
  await page.click('#nav-settings');
  assert.equal(await page.$eval('#auto-update', (el) => el.checked), false);
  await page.evaluate(() => { window.__test.fail = 'set_auto_update'; });
  await page.click('#auto-update');
  await page.waitForFunction(() => document.getElementById('message-text').textContent.includes('Could not save'));
  assert.equal(await page.$eval('#auto-update', (el) => el.checked), false);

  // An update check never prevents history navigation or resuming collection.
  await open('stopped');
  assert.equal(await text('status-action'), 'Resume');
  await page.waitForFunction(() => window.__test.calls.includes('show_dashboard'));
  assert.equal((await calls()).includes('enable'), false);
  await page.click('#nav-settings');
  await page.evaluate(() => { window.__test.updates.phase = 'checking'; });
  await page.waitForFunction(() => document.getElementById('check-updates').disabled);
  assert.equal(await disabled('status-action'), false);
  await page.click('#nav-dashboard');
  assert.equal(await visible('dashboard-view'), true);
  await page.click('#status-action');
  await page.waitForFunction(() => window.__test.calls.includes('resume') && document.getElementById('status').textContent.startsWith('Collecting ·'));
  assert.equal((await calls()).includes('enable'), false);

  await open('stopped');
  await page.evaluate(() => { window.__test.fail = 'resume'; });
  await page.click('#status-action');
  await page.waitForFunction(() => document.getElementById('status-detail').textContent.includes('did not start'));
  assert.equal(await disabled('status-action'), false);
  await page.click('#nav-settings');
  assert.equal(await visible('message'), true);
  await screenshot('stopped');

  await open();
  await page.click('#nav-settings');
  await page.click('#disconnect');
  assert.equal(await page.$eval('#disconnect-dialog', (el) => el.open), true);
  await page.keyboard.press('Escape');
  assert.equal((await calls()).includes('disconnect'), false);
  await page.click('#disconnect');
  await page.click('#confirm-disconnect');
  await page.waitForFunction(() => document.getElementById('status').textContent.startsWith('Collection stopped ·'));
  await page.click('#history');
  assert.equal(await visible('dashboard-view'), true);
  assert.equal(await text('status-action'), 'Set up collection');
  await open('disconnected');
  await page.waitForFunction(() => window.__test.calls.includes('show_dashboard'));
  assert.equal((await calls()).includes('enable'), false);
  assert.equal((await calls()).includes('resume'), false);

  await page.click('#nav-settings');
  await page.click('#check-updates');
  await page.waitForFunction(() => !document.getElementById('install-update').disabled);
  await page.evaluate(() => { window.__test.fail = 'install_update'; });
  await page.click('#install-update');
  await page.waitForFunction(() => document.getElementById('update-status').textContent.includes('could not be installed'));
  assert.match(await text('update-error'), /Signature/);
  await page.evaluate(() => { window.__test.updates.phase = 'downloading'; window.__test.updates.error = null; });
  await page.waitForFunction(() => document.getElementById('update-status').textContent.includes('Downloading'));
  assert.equal(await disabled('setup'), true);
  assert.equal(await disabled('nav-dashboard'), false);
  await page.evaluate(() => { window.__test.updates.configured = false; });
  await page.waitForFunction(() => document.getElementById('update-status').textContent.includes('unavailable'));
  assert.equal(await disabled('check-updates'), true);

  // Viewer failure has a real retry, and does not rerun collection setup.
  await open();
  await page.click('#nav-settings');
  await page.evaluate(() => { window.__test.fail = 'show_dashboard'; });
  await page.click('#nav-dashboard');
  await page.waitForFunction(() => !document.getElementById('retry-dashboard').hidden);
  await page.evaluate(() => { window.__test.fail = null; });
  await page.click('#retry-dashboard');
  await page.waitForFunction(() => document.getElementById('retry-dashboard').hidden);
  await page.evaluate(() => window.__test.events.navigate({ payload: 'settings' }));
  assert.equal(await visible('settings-view'), true);
  await page.evaluate(() => { window.__test.hold = 'show_dashboard'; });
  await page.click('#nav-dashboard');
  await page.waitForFunction(() => Boolean(window.__test.release));
  await page.click('#nav-settings');
  await page.evaluate(() => { window.__test.hold = null; window.__test.release(); });
  await page.waitForFunction(() => window.__test.calls.filter((c) => c === 'show_dashboard' || c === 'hide_dashboard').at(-1) === 'hide_dashboard');
  assert.equal(await visible('settings-view'), true);
  await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: 'dark' }]);
  await screenshot('settings-dark');
  await page.setViewport({ width: 760, height: 600 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  assert.deepEqual(errors, []);
  console.log('PASS: first setup, dashboard-first launch, resume, retained history, recovery, settings navigation, update preferences, nonblocking checks, installation locking, and narrow layout (native bridge mocked).');
} finally {
  await browser?.close();
  await new Promise((resolve) => server.close(resolve));
}
