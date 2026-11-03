import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Browsing, Connections, CollectionDestination } from '../src/workspace-domain.ts';

test('browsing never configures a connection or changes collection, including rapid navigation', async () => {
  const effects = [];
  const saved = {personal:{kind:'s3',url:'s3://saved/history'}};
  const connections = new Connections({s3:()=>effects.push('configure')},saved);
  const collection = new CollectionDestination(()=>effects.push('collect'));
  const browsing = new Browsing({history:(source)=>effects.push(source)});
  for (const target of ['dashboard','settings','dashboard','connection','dashboard']) {
    await browsing.present(browsing.select(target,'local'));
  }
  assert.deepEqual(effects,['local','local','local']);
  assert.deepEqual(connections.saved,saved);
  const stale = browsing.select('dashboard');
  const current = browsing.select('settings');
  await browsing.present(stale);
  await browsing.present(current);
  assert.equal(effects.at(-1),'local');
  assert.equal(effects.length,3);
  assert.ok(collection);
});

test('failed connection configuration preserves saved settings and never changes browsing', async () => {
  const browsing = new Browsing({}, {destination:'dashboard',reader:'local'});
  const old = {personal:{kind:'s3',url:'s3://old/history'}};
  const connections = new Connections({s3:async()=>{throw Error('offline');}},old);
  await assert.rejects(connections.configure('personal',{kind:'s3',url:'s3://new/history'}));
  assert.deepEqual(connections.saved,old);
  assert.deepEqual({destination:browsing.destination,reader:browsing.reader},{destination:'dashboard',reader:'local'});
});

test('collection changes require an explicit observed destination and cannot change browsing', async () => {
  const browsing = new Browsing({}, {destination:'dashboard'});
  const calls=[];
  const collection = new CollectionDestination(s=>calls.push(s));
  await assert.rejects(collection.change({kind:'local'}));
  assert.deepEqual(calls,[]);
  await collection.change({kind:'local',expected:{cloud:null,s3:null}});
  assert.equal(calls.length,1);
  assert.equal(browsing.destination,'dashboard');
});

test('login selects one workspace automatically, asks for multiple, and reuses browser selection', async () => {
  const { automaticWorkspace } = await import('../src/workspace-domain.ts');
  assert.equal(automaticWorkspace([]), null);
  assert.equal(automaticWorkspace([{slug:'one'}]), 'one');
  const multiple = [{slug:'one'}, {slug:'two'}];
  assert.equal(automaticWorkspace(multiple), null);
  assert.equal(automaticWorkspace(multiple, 'two'), 'two');
  assert.equal(automaticWorkspace(multiple, 'foreign'), null);
});
