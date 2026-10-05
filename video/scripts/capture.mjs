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
  // Same UI and mock API data, with the local navigation variant.
  await page.route('**/web/me', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    await route.fulfill({response, json: {...body, local: true, subscription_usage: false}});
  });
  for (const route of ['overview', 'tools', 'mcp']) {
    await page.goto(`${origin}/${route}`, {waitUntil: 'networkidle'});
    await page.locator(route === 'overview' ? '.stat-tile' : 'tbody tr').first().waitFor();
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({path: `${output}/${route}.png`, animations: 'disabled'});
    console.log(`Captured ${route}`);
  }
} finally {
  await browser.close();
}
