import {chromium} from 'playwright';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdir, writeFile, readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
const exec = promisify(execFile);
const origin = 'http://127.0.0.1:5186';
const output = fileURLToPath(new URL('../public/demo/', import.meta.url));
const raw = fileURLToPath(new URL('./raw/', import.meta.url));
await mkdir(output, {recursive: true}); await mkdir(raw, {recursive: true});
const browser = await chromium.launch({executablePath: process.env.CHROME_PATH || '/usr/bin/google-chrome', args: ['--no-sandbox']});
const log = [];

async function record(name, seconds, startPage, scope, perform) {
  if (process.env.DEMO_CLIPS && !process.env.DEMO_CLIPS.split(',').includes(name)) return;
  const context = await browser.newContext({viewport: {width: 1440, height: 720},
    colorScheme: 'light', locale: 'en-US', timezoneId: 'UTC', recordVideo: {dir: raw, size: {width: 1440, height: 720}}});
  await context.addCookies([{name: 'demo_scope', value: scope, url: origin}]);
  // Presentation-only pointer overlay. All clicks, sorting and scrolling hit the real UI.
  await context.addInitScript(() => {
    document.addEventListener('DOMContentLoaded', () => {
      const cursor = document.createElement('div'); cursor.id = 'demo-cursor';
      cursor.innerHTML = '<svg width="30" height="38" viewBox="0 0 30 38"><path d="M3 2v28l7-7 6 12 5-3-6-11h11Z" fill="#3457d5" stroke="white" stroke-width="2"/></svg>';
      Object.assign(cursor.style, {position: 'fixed', left: '850px', top: '600px', zIndex: '2147483647', pointerEvents: 'none', filter: 'drop-shadow(0 2px 2px #0004)'});
      document.body.append(cursor);
      document.addEventListener('mousemove', e => {cursor.style.left = e.clientX + 'px'; cursor.style.top = e.clientY + 'px';});
      document.addEventListener('mousedown', e => {
        const ring = document.createElement('div');
        Object.assign(ring.style, {position: 'fixed', left: e.clientX - 20 + 'px', top: e.clientY - 20 + 'px', width: '40px', height: '40px', border: '3px solid #3457d5', borderRadius: '50%', zIndex: '2147483646', pointerEvents: 'none'});
        document.body.append(ring); ring.animate([{transform:'scale(.5)',opacity:1},{transform:'scale(1.5)',opacity:0}], {duration:500}).onfinish=()=>ring.remove();
      });
    });
  });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  page.on('response', r => {if(r.url().includes('/web/') && r.status() >= 400) errors.push(`${r.status()} ${r.url()}`);});
  await page.goto(`${origin}${startPage}`, {waitUntil: 'networkidle'});
  const started = performance.now();
  const steps = [];
  const at = async (time, label, action) => {
    const wait = time * 1000 - (performance.now() - started);
    if (wait > 0) await page.waitForTimeout(wait);
    steps.push({at: Number(((performance.now() - started) / 1000).toFixed(3)), label});
    await action();
  };
  const move = async locator => {
    await locator.scrollIntoViewIfNeeded();
    const box = await locator.boundingBox(); if (!box) throw new Error('Missing target');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, {steps: 20});
  };
  const click = async locator => {await move(locator); await page.waitForTimeout(180); await locator.click();};
  const nav = label => page.getByRole('link', {name: label, exact: true});
  try {
    await perform({page, at, move, click, nav});
    await at(seconds, 'end', async () => {});
    const elapsed = (performance.now() - started) / 1000;
    if (elapsed > seconds + 1) throw new Error(`${name} overran by ${elapsed - seconds}s`);
    if (errors.length) throw new Error(errors.join('\n'));
    await page.screenshot({path: `${output}/${name}-end.png`});
    const video = page.video();
    await context.close();
    const path = await video.path();
    const {stdout} = await exec('ffprobe', ['-v','error','-show_entries','format=duration','-of','default=nw=1:nk=1',path]);
    const lead = Math.max(0, Number(stdout.trim()) - elapsed);
    await exec('ffmpeg', ['-v','error','-y','-ss',String(lead),'-i',path,'-t',String(seconds),'-vf',`fps=30,tpad=stop_mode=clone:stop_duration=1`,'-c:v','libx264','-crf','17','-pix_fmt','yuv420p','-an','-movflags','+faststart',`${output}/${name}.mp4`]);
    log.push({name, seconds, steps, rawDuration: Number(stdout.trim()), trimmedLead: lead});
    console.log(`Recorded ${name}: ${seconds}s, ${steps.length - 1} real UI actions`);
  } catch (error) {
    await page.screenshot({path: `${raw}/${name}-error.png`}).catch(()=>{});
    await context.close().catch(()=>{}); throw error;
  }
}

try {
  await record('workspace', 10, '/overview', 'personal', async ({page, at, click, move, nav}) => {
    await at(1, 'Switch Personal → Studio Demo', async () => {
      const select = page.getByRole('combobox', {name:'Viewing workspace'});
      await click(select);
      await page.keyboard.press('ArrowDown');
      await Promise.all([page.waitForNavigation({waitUntil:'networkidle'}), page.keyboard.press('Enter')]);
      if (await select.inputValue() !== 'demo-team') throw new Error('Workspace did not switch');
    });
    await at(4, 'Open member usage', async () => {await click(nav('Members')); await page.getByText('ren@example.com', {exact:true}).waitFor();});
    await at(6, 'Inspect member totals', async () => {await move(page.locator('tr').filter({hasText:'ren@example.com'}).locator('td').nth(8));});
  });
  await record('models', 14, '/members', 'team', async ({page, at, click, move, nav}) => {
    await at(.8, 'Open Models', async () => {await click(nav('Models')); await page.getByRole('heading',{name:'Daily tokens by model'}).waitFor();});
    await at(3, 'Inspect daily model usage', async () => {await move(page.getByRole('img',{name:'Daily tokens per model'}));});
    await at(5, 'Scroll to model and effort breakdown', async () => {await page.mouse.move(1160,540); await page.mouse.wheel(0,470);});
    await at(7, 'Sort by use inside subagents', async () => {await click(page.getByRole('button',{name:'In subagents'}));});
    await at(10, 'Inspect Opus 100% in subagents', async () => {
      const row = page.locator('tbody tr').filter({hasText:'claude-opus-4-1'});
      if (!(await row.innerText()).includes('100%')) throw new Error('Missing Opus attribution');
      await move(row.locator('td').nth(5));
    });
  });
  await record('extensions', 14, '/models', 'team', async ({page, at, click, move, nav}) => {
    await at(.8, 'Open MCP', async () => {await click(nav('MCP')); await page.getByText('notion',{exact:true}).waitFor();});
    await at(3, 'Inspect unused notion server', async () => {await move(page.locator('tbody tr').filter({hasText:'notion'}).locator('td').first());});
    await at(6, 'Open Skills', async () => {await click(nav('Skills')); await page.getByRole('heading',{name:'Skills',exact:true}).waitFor();});
    await at(8, 'Inspect 21 code-review invocations', async () => {await move(page.locator('tbody tr').filter({hasText:'code-review'}).first());});
    await at(10, 'Inspect configured but unused release-notes', async () => {await page.mouse.wheel(0,280); await move(page.locator('tbody tr').filter({hasText:'release-notes'}));});
  });
  await record('failures', 9, '/skills', 'team', async ({page, at, click, move, nav}) => {
    await at(.8, 'Open Tools', async () => {await click(nav('Tools')); await page.getByRole('heading',{name:'Tools',exact:true}).waitFor();});
    await at(3, 'Sort failures descending', async () => {await click(page.getByRole('button',{name:'Failures'}));});
    await at(5, 'Inspect Playwright: 42 calls / 6 failures / p95 30s', async () => {
      const first = page.locator('tbody tr').first();
      if (!(await first.innerText()).includes('playwright')) throw new Error('Failure sort did not work');
      await move(first.locator('td').nth(3));
    });
  });
  await record('cli', 17, '/terminal', 'team', async ({page, at, click}) => {
    const command = async text => {
      const input = page.getByRole('textbox',{name:'Command'}); await click(input);
      await input.pressSequentially(text, {delay:42}); await input.press('Enter');
      await page.waitForFunction(command => window.lastResult?.command === command, text);
    };
    await at(.6, 'Run real CLI: unused-mcp', () => command('kikimimi query unused-mcp'));
    await at(7.7, 'Clear terminal', async () => {await page.keyboard.press('Control+l');});
    await at(8.3, 'Run real CLI: tools', () => command('kikimimi query tools'));
  });
  const previous = process.env.DEMO_CLIPS ? JSON.parse(await readFile(`${output}/recording.json`, 'utf8')) : [];
  const merged = previous.map(clip => log.find(updated => updated.name === clip.name) || clip);
  await writeFile(`${output}/recording.json`, JSON.stringify(previous.length ? merged : log, null, 2));
} finally {await browser.close();}
