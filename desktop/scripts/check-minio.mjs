// Real S3Sink -> AWS CLI v2 -> MinIO -> separate viewer -> browser.
// Requires Docker, DuckDB, built kikimimi and s3_smoke binaries, and Chrome.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import puppeteer from 'puppeteer-core';
const exec = promisify(execFile);
const root = await mkdtemp(join(tmpdir(), 'kikimimi-minio-'));
const network = `kikimimi-minio-${randomUUID().slice(0,8)}`;
const minio = `${network}-server`;
const endpoint = 'http://minio:9000';
const bucket = 'team-test';
const date = new Date().toISOString().slice(0,10);
const prefix = `shared/kikimimi.v1/events/dt=${date}/`;
const secret = randomUUID();
const readerSecret = randomUUID();
const baseEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('AWS_')));
const env = { ...baseEnv, AWS_ACCESS_KEY_ID:'local-test-admin', AWS_SECRET_ACCESS_KEY:secret, AWS_DEFAULT_REGION:'us-east-1', AWS_EC2_METADATA_DISABLED:'true', AWS_MAX_ATTEMPTS:'1', AWS_CONFIG_FILE:'/dev/null', AWS_SHARED_CREDENTIALS_FILE:'/dev/null', AWS_PAGER:'', MINIO_ROOT_USER:'local-test-admin', MINIO_ROOT_PASSWORD:secret, TEST_READER_SECRET:readerSecret };
const docker = args => exec('docker', args, { env, maxBuffer: 1024*1024 });
const mount = ['-v',`${root}:${root}`];
const mc = script => docker(['run','--rm','--network',network,...mount,'-e','MINIO_ROOT_USER','-e','MINIO_ROOT_PASSWORD','-e','TEST_READER_SECRET','--entrypoint','/bin/sh','minio/mc:RELEASE.2025-08-13T08-35-41Z','-ec',`attempt=0\nuntil mc alias set test ${endpoint} "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" >/dev/null 2>&1; do\n  attempt=$((attempt+1))\n  [ "$attempt" -lt 30 ] || exit 1\n  sleep 1\ndone\n${script}`]);
const aws = args => exec(join(root,'bin/aws'), [...args,'--endpoint-url',endpoint], {env,maxBuffer:1024*1024});
const upload = async host => {
  await exec(resolve(import.meta.dirname,'../../target/debug/examples/s3_smoke'), [`s3://${bucket}/shared`,endpoint,join(root,`writer-${host}`),host,date,`event-${host}`], {env:{...env,PATH:`${root}/bin:${env.PATH}`}});
};
let child, lines, browser, startupTimer;
try {
  await mkdir(join(root,'bin')); await mkdir(join(root,'viewer'));
  // This wrapper only transports the real AWS CLI into a container, with identical arguments.
  await writeFile(join(root,'bin/aws'), `#!/bin/sh\nexec docker run --rm --network '${network}' -v '${root}:${root}' -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY -e AWS_DEFAULT_REGION -e AWS_EC2_METADATA_DISABLED -e AWS_MAX_ATTEMPTS -e AWS_CONFIG_FILE -e AWS_SHARED_CREDENTIALS_FILE -e AWS_PAGER amazon/aws-cli:2.34.7 "$@"\n`, {mode:0o700});
  await writeFile(join(root,'reader.json'), JSON.stringify({Version:'2012-10-17',Statement:[{Effect:'Allow',Action:['s3:ListBucket'],Resource:[`arn:aws:s3:::${bucket}`],Condition:{StringLike:{'s3:prefix':['shared/kikimimi.v1/events/*']}}},{Effect:'Allow',Action:['s3:GetObject'],Resource:[`arn:aws:s3:::${bucket}/shared/kikimimi.v1/events/*`]}]}));
  await docker(['network','create',network]);
  await docker(['run','-d','--rm','--name',minio,'--network',network,'--network-alias','minio','-e','MINIO_ROOT_USER','-e','MINIO_ROOT_PASSWORD','minio/minio:RELEASE.2025-07-23T15-54-02Z','server','/data']);
  // mc retries readiness; no host port, real accounts or persistent volumes are used.
  await mc(`mc ready test\nmc mb test/${bucket}\nmc admin user add test local-test-reader "$TEST_READER_SECRET"\nmc admin policy create test dashboard-reader '${root}/reader.json'\nmc admin policy attach test dashboard-reader --user local-test-reader`);
  await upload('machine-a'); await upload('machine-b');
  let listing = JSON.parse((await aws(['s3api','list-objects-v2','--bucket',bucket,'--prefix',prefix])).stdout);
  assert.equal(listing.Contents.length,2,'production sink uploaded both machines');
  const a = listing.Contents.find(o=>o.Key.split('/').at(-1).startsWith('machine-'));
  // All host prefixes begin machine-; use the first object only for a duplicate check.
  await aws(['s3api','copy-object','--bucket',bucket,'--copy-source',`${bucket}/${a.Key}`,'--key',`${prefix}duplicate.parquet`]);
  console.log('PASS: production S3 sink uploaded two machines via real AWS CLI and MinIO.');
  const readerEnv = {...env,AWS_ACCESS_KEY_ID:'local-test-reader',AWS_SECRET_ACCESS_KEY:readerSecret,KIKIMIMI_DIR:join(root,'viewer'),PATH:`${root}/bin:${env.PATH}`};
  await assert.rejects(exec(join(root,'bin/aws'),['s3api','put-object','--bucket',bucket,'--key',`${prefix}forbidden`,'--endpoint-url',endpoint],{env:readerEnv}), 'reader must not write');
  const standalone = process.env.S3_READER_STANDALONE === '1';
  child = spawn(resolve(import.meta.dirname,'../../target/debug/kikimimi'),standalone?['web','--read-only']:['desktop','serve-dashboard'],{env:readerEnv,stdio:['pipe','pipe','pipe']});
  child.stderr.resume(); lines = createInterface({input:child.stdout});
  const [line] = await Promise.race([once(lines,'line'),new Promise((_,reject)=>{startupTimer=setTimeout(()=>reject(new Error('viewer startup timeout')),10000);})]);
  clearTimeout(startupTimer);
  const url = new URL(standalone?line.replace(/^Open /,''):JSON.parse(line).url);
  browser = await puppeteer.launch({executablePath:process.env.CHROME_BINARY || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true,args:['--no-sandbox']});
  const page = await browser.newPage(); const errors=[];
  page.on('pageerror',e=>errors.push(e.message)); await page.setViewport({width:1200,height:900});
  await page.goto(url.href); await page.waitForSelector('.topbar a[href="/storage"]');
  await page.click('.topbar a[href="/storage"]'); await page.waitForSelector('#read-s3-url');
  await page.type('#read-s3-url',`s3://${bucket}/shared`);
  await page.click('.source-form summary'); await page.type('#read-s3-endpoint',endpoint);
  await page.click('.source-form button');
  await page.waitForSelector('.s3-snapshot-bar',{timeout:180000});
  await page.waitForFunction(()=>document.querySelector('.app-main')?.textContent.includes('machine-a'));
  const request = (path,method='GET') => page.evaluate(async ({path,method})=>{const r=await fetch(path,{method});return {status:r.status,body:await r.json()};},{path,method});
  const machines = async () => (await request('/web/q/machines')).body.rows.map(r=>r[0]).sort();
  assert.deepEqual(await machines(),['machine-a','machine-b']);
  assert.equal((await request('/web/q/overview')).body.rows.reduce((sum,row)=>sum+row[1],0),2,'duplicate event counted once');
  for (const route of ['sessions','tools','models','mcp','skills','unused-mcp','unused-skills','patterns','subagents','coverage','session?session_id=machine-a-session']) assert.equal((await request(`/web/q/${route}`)).status,200,route);
  console.log('PASS: separate read-only viewer renders both machines, deduplicates and queries all analysis pages.');
  if(process.env.S3_SCREENSHOT) await page.screenshot({path:process.env.S3_SCREENSHOT,fullPage:true});
  const before=(await request('/web/source')).body.refreshed_at;
  await mc('mc admin user disable test local-test-reader');
  await assert.rejects(exec(join(root,'bin/aws'),['s3api','list-objects-v2','--bucket',bucket,'--prefix',prefix,'--endpoint-url',endpoint],{env:readerEnv}));
  assert.equal((await request('/web/source/refresh','POST')).status,502);
  assert.equal((await request('/web/source')).body.refreshed_at,before);
  assert.deepEqual(await machines(),['machine-a','machine-b']);
  await mc('mc admin user enable test local-test-reader');
  await upload('machine-c');
  // Remove one original machine and its duplicate, retaining the other and machine-c.
  listing=JSON.parse((await aws(['s3api','list-objects-v2','--bucket',bucket,'--prefix',prefix])).stdout);
  const originalKeys = listing.Contents.filter(o=>o.Key===`${prefix}duplicate.parquet` || o.Key===a.Key);
  for(const o of originalKeys) await aws(['s3api','delete-object','--bucket',bucket,'--key',o.Key]);
  assert.equal((await request('/web/source/refresh','POST')).status,200);
  const remaining=await machines(); assert.equal(remaining.length,2); assert(remaining.includes('machine-c'));
  console.log('PASS: revoked reader access fails refresh while preserving snapshot; restored access reflects additions and deletions.');
  await docker(['stop',minio]);
  const beforeOutage=(await request('/web/source')).body.refreshed_at;
  assert.equal((await request('/web/source/refresh','POST')).status,502);
  assert.equal((await request('/web/source')).body.refreshed_at,beforeOutage);
  assert.deepEqual(await machines(),remaining);
  assert.deepEqual(errors,[]);
  console.log(`PASS: MinIO outage preserves snapshot; zero browser errors (${standalone?'local web':'desktop viewer'}).`);
} finally {
  clearTimeout(startupTimer); await browser?.close(); lines?.close();
  if(child && child.exitCode===null) {const done=once(child,'exit');child.kill('SIGINT');const timer=setTimeout(()=>child.kill('SIGKILL'),5000);await done;clearTimeout(timer);}
  await docker(['rm','-f',minio]).catch(()=>{});
  await docker(['network','rm',network]).catch(()=>{});
  await rm(root,{recursive:true,force:true});
}
