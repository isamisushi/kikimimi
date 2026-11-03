// Real OTLP collector -> durable DuckDB outbox -> Cloud/Postgres -> both built SPAs.
// All services, identities and provider data are disposable. No API response mocks.
// Prerequisites: built debug kikimimi and sibling kikimimi-cloud, both web/dist,
// Docker with postgres:18-alpine available, Chrome, desktop's puppeteer-core dependency.
// Run: node web/scripts/check-cost-sync-e2e.mjs [path/to/kikimimi-cloud]
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, open } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { createServer } from 'node:net';
const repo = fileURLToPath(new URL('../../', import.meta.url));
const cloudRepo = path.resolve(process.argv[2] || path.join(repo, '../kikimimi-cloud'));
const puppeteer = createRequire(new URL('../../desktop/package.json', import.meta.url))('puppeteer-core');
const dir = await mkdtemp(path.join(tmpdir(), 'kikimimi-cost-sync-e2e-'));
const children = [], handles = [], checks = [];
const cli = path.join(repo, 'target/debug/kikimimi');
let browser, pgContainer;
function check(value, label) { assert.ok(value, label); checks.push(label); console.log(`PASS ${label}`); }
function run(bin, args, env = process.env) {
  const r = spawnSync(bin, args, { env, encoding: 'utf8', timeout: 60000 });
  assert.equal(r.status, 0, `${bin}: ${r.stderr || r.error}`); return r.stdout;
}
async function port() { const s = createServer(); await new Promise((r,reject) => { s.once('error',reject); s.listen(0,'127.0.0.1',r); }); const p=s.address().port; await new Promise(r=>s.close(r)); return p; }
async function launch(bin,args,env,label) {
  const log=await open(path.join(dir,`${label}.log`),'a'); handles.push(log);
  const child=spawn(bin,args,{env,stdio:['ignore',log.fd,log.fd]}); children.push(child); return child;
}
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill('SIGTERM');
  await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{child.kill('SIGKILL');reject(Error('stop timeout'));},15000);child.once('exit',()=>{clearTimeout(timer);resolve();});});
}
async function until(fn,label,ms=45000) {
  const end=Date.now()+ms; let last;
  while(Date.now()<end) { try { const v=await fn(); if(v)return v; } catch(e){last=e;} await new Promise(r=>setTimeout(r,250)); }
  throw Error(`Timeout: ${label}; ${last?.message || ''}`);
}
async function request(base,p,body,cookie) {
  const r=await fetch(base+p,{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json',...(cookie?{Cookie:cookie}:{})},body:body===undefined?undefined:JSON.stringify(body)});
  assert.equal(r.status,200,`${p}: ${r.status} ${await r.clone().text()}`);return r;
}
try {
  // A whole new Postgres cluster avoids changing roles/passwords on any existing DB.
  const pgPort=await port(), cloudPort=await port(), otlpPort=await port(), webPort=await port();
  pgContainer=run('docker',['run','--rm','-d','-e','POSTGRES_PASSWORD=disposable-e2e-pg','-p',`127.0.0.1:${pgPort}:5432`,'postgres:18-alpine']).trim();
  await until(()=>spawnSync('docker',['exec',pgContainer,'pg_isready','-h','127.0.0.1','-U','postgres'],{encoding:'utf8'}).status===0,'Postgres ready');
  const cloud=`http://127.0.0.1:${cloudPort}`;
  const cloudEnv={PATH:process.env.PATH,HOME:process.env.HOME,DATABASE_URL:`postgres://postgres:disposable-e2e-pg@127.0.0.1:${pgPort}/postgres?sslmode=disable`,BIND_ADDR:`127.0.0.1:${cloudPort}`,KIKIMIMI_PUBLIC_BASE_URL:cloud,KIKIMIMI_APP_DB_PASSWORD:'disposable-e2e-password',KIKIMIMI_INVITE_CODE:'disposable-e2e-invite',KIKIMIMI_INVITE_ONLY:'0',KIKIMIMI_LEGACY_INVITE:'1',PATTERN_SCAN_INTERVAL_SECS:'0'};
  await launch(path.join(cloudRepo,'target/debug/kikimimi-cloud'),[],cloudEnv,'cloud');
  await until(async()=> (await fetch(cloud+'/healthz',{signal:AbortSignal.timeout(2000)})).ok,'Cloud ready');
  const login=await request(cloud,'/web/login',{email:'cost-e2e@example.com',invite_code:'disposable-e2e-invite'});
  const cookie=login.headers.get('set-cookie').split(';')[0];
  await request(cloud,'/web/orgs',{slug:'cost-e2e',name:'Cost E2E'},cookie);
  await request(cloud,'/web/active-org',{slug:'cost-e2e'},cookie);
  const host='cost-e2e-device';
  const codes=await (await request(cloud,'/v1/device/code',{host_id:host,hostname:'E2E'})).json();
  const approved=await fetch(cloud+'/activate',{method:'POST',headers:{Cookie:cookie,'Content-Type':'application/x-www-form-urlencoded'},body:new URLSearchParams({code:codes.user_code,org_slug:'cost-e2e'})});
  assert.equal(approved.status,200);
  const device=await (await request(cloud,'/v1/device/token',{device_code:codes.device_code})).json();
  assert.equal(device.status,'ok');
  const local=path.join(dir,'collector'), empty=path.join(dir,'empty'); await mkdir(local);await mkdir(empty);
  await writeFile(path.join(local,'host_id'),host);
  await writeFile(path.join(local,'config.json'),JSON.stringify({otlp_port:otlpPort,otlp_token:'e2e-otlp-token',web_port:webPort,cloud:{endpoint:cloud,token:device.token,email:device.email,org_id:device.org_id,org_slug:'cost-e2e',org_kind:'team',repo_patterns:[]}}),{mode:0o600});
  // Use each provider's profile override as intended: never tail the user's real files.
  const env={...process.env,KIKIMIMI_DIR:local,KIKIMIMI_SECRET_STORE:'file',CODEX_HOME:empty,CLAUDE_CONFIG_DIR:empty,KIKIMIMI_CLAUDE_SUPPORT_DIR:empty,KIKIMIMI_CLAUDE_PROJECTS_DIR:empty,KIKIMIMI_COWORK_SESSIONS_DIR:empty}; delete env.XDG_RUNTIME_DIR;
  let collector=await launch(cli,['agent','--foreground'],env,'collector');
  const state=()=>readFile(path.join(local,'state.json'),'utf8').then(JSON.parse);
  await until(async()=>{const s=await state();return s.pid===collector.pid&&s.web?.token;},'collector ready');
  let localToken=(await state()).web.token; const localBase=`http://127.0.0.1:${webPort}`;
  const models=async(base,auth)=> (await request(base,'/web/q/models?days=30',undefined,auth)).json();
  const localModels=async()=> {const r=await fetch(localBase+'/web/q/models?days=30',{headers:{Cookie:`kikimimi_local=${localToken}`}});assert.equal(r.status,200,await r.clone().text());return r.json();};
  check((await localModels()).models.rows.length===0,'isolated collector starts with no requests');
  const ts=BigInt(Date.now())*1000000n;
  function payload(offset,missing=false) {
    const attrs={'session.id':'cost-e2e-session',model:'claude-haiku-4-5',input_tokens:1000000,cache_read_tokens:0,cache_creation_tokens:0,...(!missing?{output_tokens:1000000,cost_usd:6}:{})};
    return {resourceLogs:[{scopeLogs:[{logRecords:[{timeUnixNano:String(ts+BigInt(offset)),eventName:'api_request',attributes:Object.entries(attrs).map(([key,v])=>({key,value:typeof v==='string'?{stringValue:v}:{intValue:String(v)}}))}]}]}]};
  }
  async function collect(data) {const r=await fetch(`http://127.0.0.1:${otlpPort}/v1/logs`,{method:'POST',headers:{'Content-Type':'application/json',Authorization:'Bearer e2e-otlp-token'},body:JSON.stringify(data)});assert.equal(r.status,200,await r.text());run(cli,['flush'],env);}
  async function agree(count,complete,cost) {
    const response=await until(async()=>{const l=await localModels(),c=await models(cloud,cookie);return l.models.rows[0]?.[2]===count&&c.models.rows[0]?.[2]===count?{l,c}:false;},'local/Cloud aggregate convergence');
    const l=response.l.models.rows[0],c=response.c.models.rows[0];
    assert.deepEqual(c,l,'local and Cloud full model rows'); assert.equal(l[14],complete);assert.equal(l[12],cost);assert.equal(l[13],complete);
    assert.equal(l[15],complete*1000000);assert.equal(l[16],complete*1000000);
    check(true,`${count} collected request(s): local/Cloud tokens, reported cost and completeness match`);
    return response;
  }
  await collect(payload(0)); await agree(1,1,6);
  browser=await puppeteer.launch({executablePath:process.env.CHROME_BIN||'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true,args:['--no-sandbox']});
  const pages=[]; const errors=[];
  for(const [name,base,auth] of [['desktop',localBase,null],['cloud',cloud,cookie]]) {
    const page=await browser.newPage();page.on('pageerror',e=>errors.push(e.message));await page.setViewport({width:1440,height:1100});
    if(auth){const [key,...val]=auth.split('=');await page.setCookie({name:key,value:val.join('='),url:base});}
    if(!auth)await page.goto(base+`/?t=${localToken}`,{waitUntil:'networkidle0'});
    await page.goto(base+'/cost',{waitUntil:'networkidle0'});
    await page.waitForSelector('#compare-current-price');
    console.log(`Ready ${name} comparison`);
    pages.push({name,page,base});
  }
  async function fill(page,id,value){
    // Number inputs do not expose text selection consistently in headless Chrome.
    // Dispatch the native input event; React still handles and validates the edit.
    await page.$eval('#'+id,(el,v)=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(el,v);el.dispatchEvent(new Event('input',{bubbles:true}));},value);
    assert.equal(await page.$eval('#'+id,e=>e.value),value);
  }
  async function verdict(page,text){await page.waitForFunction(t=>document.querySelector('.billing-verdict')?.textContent.includes(t),{timeout:10000},text);}
  async function scenario(page,current,proposed,winner) {
    console.log(`Compare ${current}/${proposed}: ${winner}`);
    await fill(page,'compare-current-price',current);await fill(page,'compare-proposed-price',proposed);
    if(!await page.$eval('#compare-scope',e=>e.checked))await page.click('#compare-scope');
    await verdict(page,winner);
  }
  for(const {name,page} of pages){
    await page.bringToFront();
    await page.waitForFunction(()=>document.querySelector('[data-comparison-monthly="api"]')?.textContent.includes('$6.00'));
    await scenario(page,'100','80','Lowest cost under these assumptions: Move to API');
    assert.match(await page.$eval('.billing-verdict',e=>e.textContent),/1,128\.00/);
    await scenario(page,'3','2','Lowest cost under these assumptions: Change plan');
    await scenario(page,'1','2','Lowest cost under these assumptions: Keep subscription');
    check(true,`${name}: collected usage drives all three cheapest choices and $1,128 API savings`);
  }
  // Replay the exact provider batch, then restart with the persisted database/outbox.
  await collect(payload(0));await agree(1,1,6);
  await stop(collector);collector=await launch(cli,['agent','--foreground'],env,'collector');
  await until(async()=>{const s=await state();return s.pid===collector.pid&&s.web?.token;},'collector restart');localToken=(await state()).web.token;
  run(cli,['flush'],env);await agree(1,1,6);
  check(true,'provider replay and collector restart do not double-count');
  await collect(payload(1,true));await agree(2,1,6);
  for(const {name,page,base} of pages){
    await page.bringToFront();
    if(name==='desktop')await page.goto(base+`/?t=${localToken}`,{waitUntil:'networkidle0'});
    await page.goto(base+'/cost',{waitUntil:'networkidle0'});
    await page.waitForSelector('#compare-current-price');
    await page.waitForFunction(()=>document.querySelector('[data-comparison-monthly="api"]')?.textContent.includes('≥ $6.00'));
    await scenario(page,'100','80','The three-way ranking is pending.');
    assert.match(await page.$eval('.billing-verdict',e=>e.textContent),/Only 1 of 2 requests are priced/);
    await scenario(page,'3','2','Lowest cost under these assumptions: Change plan');
    await page.screenshot({path:path.join(dir,`${name}-missing.png`),fullPage:true});
    check(true,`${name}: missing tokens survive sync, API shows lower bound, unsafe API recommendation withheld`);
  }
  check(errors.length===0,'no browser exceptions');
  await writeFile(path.join(dir,'results.json'),JSON.stringify({checks,providerBoundary:'synthetic OTLP; no paid API calls',cloud:'real isolated Cloud binary and Postgres',desktop:'real collector and built local SPA',screenshots:['desktop-missing.png','cloud-missing.png']},null,2));
  console.log(`PASS ${checks.length} checks; artifacts: ${dir}`);
} catch(error) {
  if(browser)for(const [i,p] of (await browser.pages()).entries()){await p.screenshot({path:path.join(dir,`failure-${i}.png`),fullPage:true}).catch(()=>{});await writeFile(path.join(dir,`failure-${i}.txt`),await p.evaluate(()=>document.body.innerText).catch(()=>''));}
  throw error;
} finally {
  await browser?.close();
  for(const child of children.reverse())await stop(child).catch(()=>{});
  if(pgContainer)run('docker',['stop',pgContainer]);
  for(const h of handles)await h.close();
  console.log(`E2E logs: ${dir}`);
}
