import { test } from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
import { readFileSync } from 'node:fs';
const source = readFileSync(new URL('../src/api/inventory.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } });
const { importPeople, parseCsv, contractTotals } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
const header='email,name,department,tool,plan,allocated_model,monthly_amount,currency,account_id\n';
test('CSV handles BOM, quoted commas, escaped quotes and newlines', () => {
  assert.deepEqual(parseCsv('\uFEFFa,b\r\n"hello, there","a""b\nc"\r\n'), [['a','b'],['hello, there','a"b\nc']]);
  assert.throws(()=>parseCsv('"unfinished'));
});
test('roster includes unassigned people and groups assignments by normalized email', () => {
  const p=importPeople(header+'A@x.com,A,Eng,codex,Pro,,20,USD,\na@x.com,A,Eng,claude-code,Team,,,USD,\nb@x.com,B,Ops,,,,,,\n');
  assert.equal(p.length,2); assert.equal(p[0].assignments.length,2); assert.equal(p[1].assignments.length,0);
  assert.equal(p[0].assignments[1].monthly_amount,null);
});
test('CSV rejects conflicting identities, duplicate tools and invalid money',()=>{
  assert.throws(()=>importPeople(header+'a@x.com,A,Eng,codex,Pro,,-2,USD,'));
  assert.throws(()=>importPeople(header+'a@x.com,A,Eng,codex,Pro,,Infinity,USD,'));
  assert.throws(()=>importPeople(header+'a@x.com,A,Eng,codex,Pro,,20,USD,\na@x.com,B,Ops,cursor,Pro,,20,USD,'));
  assert.throws(()=>importPeople(header+'a@x.com,A,Eng,codex,Pro,,20,USD,\na@x.com,A,Eng,codex,Pro,,20,USD,'));
});
test('invalid currencies are rejected before rendering the preview',()=>{
  assert.throws(()=>importPeople(header+'a@x.com,A,Eng,codex,Pro,,20,INVALID,'));
});
test('contract totals never mix currencies or invent missing amounts',()=>{
  const p=importPeople(header+'a@x.com,A,Eng,codex,Pro,,20,USD,\na@x.com,A,Eng,cursor,Pro,,3000,JPY,\nb@x.com,B,Ops,codex,Pro,,,USD,');
  assert.deepEqual(contractTotals(p),[['JPY',3000],['USD',20]]);
});
