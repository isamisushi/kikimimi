import assert from 'node:assert/strict';
import {mkdir, writeFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
const origin = 'http://127.0.0.1:5186';
const evidence = {};
async function query(name, team = true) {
  const r = await fetch(`${origin}/web/q/${name}`, {headers: {cookie: `demo_scope=${team ? 'team' : 'personal'}`}});
  assert.equal(r.status, 200, name);
  const json = await r.json(); evidence[`${team ? 'team' : 'personal'}/${name}`] = json; return json;
}
const team = await query('overview');
const personal = await query('overview', false);
const calls = data => data.rows.reduce((sum, row) => sum + row[2], 0);
assert.equal(calls(team), 357); assert.equal(calls(personal), 119);
const tools = await query('tools');
const playwright = tools.rows.find(r => r[0] === 'mcp__playwright__browser_navigate');
assert.deepEqual(playwright.slice(2), [42, 6, 900, 30000]);
const mcp = await query('unused-mcp');
assert.deepEqual(mcp.rows.find(r => r[0] === 'notion').slice(1, 4), [true, 0, 0]);
const skills = await query('skills');
assert.deepEqual(skills.rows[0].slice(0, 4), ['code-review', 21, 0, 21]);
assert.equal((await query('unused-skills')).rows.find(r => r[0] === 'release-notes')[3], 0);
const models = await query('models');
const opus = models.models.rows.find(r => r[0] === 'claude-opus-4-1');
assert.equal(opus[2], 28); assert.equal(opus[5], opus[2]); assert.equal(opus[6], 1176000);
const members = await query('members');
assert.equal(members.rows.length, 3);
assert.equal(members.rows.reduce((sum, row) => sum + row[3], 0), calls(team));
const output = fileURLToPath(new URL('../public/demo/evidence/', import.meta.url));
await mkdir(output, {recursive: true});
for (const command of ['unused-mcp', 'tools', 'skills']) {
  const r = await fetch(`${origin}/demo/run`, {method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify({command: `kikimimi query ${command}`})});
  assert.equal(r.status, 200);
  const result = await r.json(); assert.equal(result.exitCode, 0);
  if (command === 'unused-mcp') assert.match(result.stdout, /notion\s*│\s*true\s*│\s*0/);
  if (command === 'tools') assert.match(result.stdout, /mcp__playwright__browser_navigate\s*│\s*42\s*│\s*6/);
  if (command === 'skills') assert.match(result.stdout, /code-review\s*│\s*21/);
  await writeFile(`${output}/${command}.json`, JSON.stringify(result, null, 2));
}
await writeFile(`${output}/api.json`, JSON.stringify(evidence, null, 2));
console.log('Verified: workspace totals, subagent model attribution, unused extensions, CLI/UI parity.');
