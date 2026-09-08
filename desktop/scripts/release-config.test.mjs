import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { releaseConfig, writeReleaseConfig, outputPath } from './release-config.mjs';

const packet = Buffer.alloc(42);
packet.write('Ed');
const pubkey = Buffer.from(`untrusted comment: fixture public key\n${packet.toString('base64')}\n`).toString('base64');

test('release config supplies updater plugin with the same public key embedded in Rust', () => {
  const config = releaseConfig({ KIKIMIMI_UPDATER_PUBLIC_KEY: `${pubkey}\n` });
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.equal(config.plugins.updater.pubkey, pubkey);
  const runtime = readFileSync(new URL('../src-tauri/src/updates.rs', import.meta.url), 'utf8');
  assert.ok(runtime.includes('option_env!("KIKIMIMI_UPDATER_PUBLIC_KEY")'));
  assert.ok(runtime.includes('.pubkey(PUBLIC_KEY.unwrap().trim())'));
});
test('missing, malformed and private key contents fail before building', () => {
  for (const value of [undefined, '', 'bad', Buffer.from('untrusted comment: private key\nsecret').toString('base64')]) {
    assert.throws(() => releaseConfig({ KIKIMIMI_UPDATER_PUBLIC_KEY: value }), /\.pub file contents/);
  }
});
test('generated JSON contains no signing secrets and workflow consumes the generated config', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'kikimimi-release-config-test-'));
  try {
    const path = join(scratch, 'config.json');
    writeReleaseConfig({ KIKIMIMI_UPDATER_PUBLIC_KEY: pubkey, TAURI_SIGNING_PRIVATE_KEY: 'SECRET-FIXTURE', APPLE_PASSWORD: 'SECRET-FIXTURE' }, path);
    const contents = readFileSync(path, 'utf8');
    assert.equal(JSON.parse(contents).plugins.updater.pubkey, pubkey);
    assert.ok(!contents.includes('SECRET-FIXTURE'));
    assert.ok(outputPath.endsWith('/src-tauri/tauri.release.generated.conf.json'));
    const workflow = readFileSync(new URL('../../.github/workflows/desktop-release.yml', import.meta.url), 'utf8');
    assert.ok(workflow.includes('--config src-tauri/tauri.release.generated.conf.json'));
    assert.ok(workflow.indexOf('node desktop/scripts/release-config.mjs') < workflow.indexOf('uses: tauri-apps/tauri-action'));
  } finally { rmSync(scratch, { recursive: true, force: true }); }
});
