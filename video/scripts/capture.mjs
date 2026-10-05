import {chromium} from 'playwright';
import {mkdir} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';

// Capture only the local, synthetic demo server. Never use a production account.
const origin = 'http://localhost:5173';
const output = fileURLToPath(new URL('../public/screenshots/', import.meta.url));
await mkdir(output, {recursive: true});
const browser = await chromium.launch({
  ...(process.env.CHROME_PATH ? {executablePath: process.env.CHROME_PATH} : {}),
  args: ['--no-sandbox'],
});
try {
  const context = await browser.newContext({
    viewport: {width: 1440, height: 940}, deviceScaleFactor: 1.5,
    colorScheme: 'light', locale: 'en-US', timezoneId: 'UTC',
  });
  const response = await context.request.post(`${origin}/web/login`, {
    data: {email: 'demo@example.com', invite_code: 'KIKIMIMI-DEMO'},
  });
  if (!response.ok()) throw new Error(`Demo login failed: ${response.status()}`);
  const page = await context.newPage();
  // Reuse the existing configured-skill demo rows for the missing skills API.
  await page.route('**/web/q/skills?*', async route => {
    const response = await context.request.get(`${origin}/web/q/unused-skills?days=14`);
    if (!response.ok()) throw new Error('Could not load demo skills');
    const {rows} = await response.json();
    await route.fulfill({json: {
      columns: ['skill_name', 'calls', 'failures', 'distinct_sessions', 'last_used_dt'],
      rows: rows.filter(r => r[3] > 0).map(r => [r[0], r[3], null, r[4], r[5]]),
    }});
  });
  // Same UI and mock API data, with the local navigation variant.
  await page.route('**/web/me', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({response, json: {...body, local: true, subscription_usage: false}});
  });
  for (const route of ['overview', 'tools', 'mcp', 'skills', 'models', 'subagents']) {
    await page.goto(`${origin}/${route}`, {waitUntil: 'networkidle'});
    await page.locator(route === 'overview' ? '.stat-tile' : 'tbody tr').first().waitFor();
    await page.evaluate(() => document.fonts.ready);
    if (await page.getByText('Failed to load:', {exact: false}).count()) throw new Error(`API error on ${route}`);
    await page.screenshot({path: `${output}/${route}.png`, animations: 'disabled'});
    console.log(`Captured ${route}`);
    if (route === 'models') {
      await page.locator('.panel').filter({hasText: 'By model and effort'}).screenshot({path: `${output}/model-detail.png`});
      await page.locator('.panel').filter({hasText: 'Daily tokens by model'}).screenshot({path: `${output}/model-chart.png`});
    }
    if (['skills', 'mcp', 'tools', 'subagents'].includes(route)) {
      await page.locator('.panel').first().screenshot({path: `${output}/${route}-panel.png`});
    }
  }
  // Cloud workspace UI backed by the same synthetic, in-memory mock server.
  await page.unroute('**/web/me');
  await page.route('**/web/me', async (route) => {
    const response = await route.fetch();
    await route.fulfill({response, json: {...await response.json(), local: false, subscription_usage: false}});
  });
  await page.goto(`${origin}/overview`, {waitUntil: 'networkidle'});
  await page.locator('.stat-tile').first().waitFor();
  await page.screenshot({path: `${output}/personal.png`});
  await Promise.all([
    page.waitForNavigation({waitUntil: 'networkidle'}),
    page.getByRole('combobox', {name: 'Viewing workspace'}).selectOption('acme'),
  ]);
  // The app implements member usage, but the development mock lacks /q/members.
  // Supply synthetic values for the mock roster only, without changing app code.
  const rosterResponse = await context.request.get(`${origin}/web/orgs/acme/members`);
  if (!rosterResponse.ok()) throw new Error('Could not load demo team roster');
  const roster = await rosterResponse.json();
  await page.route('**/web/q/members?*', route => route.fulfill({json: {
    columns: ['user_id', 'sessions', 'api_requests', 'tool_calls', 'tool_failures', 'input_tokens', 'output_tokens', 'cache_read_tokens', 'cost_usd', 'loop_suspect_sessions'],
    rows: roster.members.map((m, i) => [m.account_id, 24 + i * 8, 320 + i * 96, 740 + i * 170, 6 + i * 3, 980000 + i * 240000, 64000 + i * 18000, 3400000 + i * 700000, 4.2 + i * 1.8, 0]),
  }}));
  await page.goto(`${origin}/members`, {waitUntil: 'networkidle'});
  await page.locator('tbody tr').first().waitFor();
  await page.screenshot({path: `${output}/team.png`});
  await page.locator('.panel').first().screenshot({path: `${output}/team-panel.png`});
  console.log('Captured personal and team workspaces');
} finally {
  await browser.close();
}
