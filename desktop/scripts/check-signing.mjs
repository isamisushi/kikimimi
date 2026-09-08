// Generate throwaway test keys in a private temp directory. Never use production
// credentials here; capture signer output so private key material is not logged.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const scratch = mkdtempSync(join(tmpdir(), 'kikimimi-update-test-'));
const tauri = join(desktop, 'node_modules/.bin/tauri');
const cleanEnv = { ...process.env, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: '' };
delete cleanEnv.TAURI_SIGNING_PRIVATE_KEY;
delete cleanEnv.TAURI_SIGNING_PRIVATE_KEY_PATH;
function signer(args) {
  try { execFileSync(tauri, ['signer', ...args], { env: cleanEnv, stdio: 'pipe' }); }
  catch { throw new Error('Test signer failed (output withheld to avoid logging key material)'); }
}
try {
  const bundle = join(scratch, 'bundle');
  mkdirSync(bundle);
  const archive = join(bundle, 'kikimimi.app.tar.gz');
  writeFileSync(archive, 'fixture archive bytes');
  writeFileSync(join(bundle, 'kikimimi.dmg'), 'fixture dmg');
  const key = join(scratch, 'test.key');
  signer(['generate', '--ci', '-w', key]);
  signer(['sign', '-f', key, '-p', '', archive]);
  const publicKey = readFileSync(`${key}.pub`, 'utf8').trim();
  const stage = (publicKey, suffix) => spawnSync(process.execPath, [join(desktop, 'scripts/release.mjs'), 'stage', bundle, join(scratch, suffix), 'aarch64'], { env: { ...cleanEnv, KIKIMIMI_UPDATER_PUBLIC_KEY: publicKey }, stdio: 'pipe' });
  assert.equal(stage(publicKey, 'valid').status, 0, 'valid Tauri signature must verify');
  const otherKey = join(scratch, 'other.key');
  signer(['generate', '--ci', '-w', otherKey]);
  assert.notEqual(stage(readFileSync(`${otherKey}.pub`, 'utf8').trim(), 'wrong-key').status, 0, 'wrong signing key must be rejected');
  writeFileSync(archive, 'tampered archive bytes');
  assert.notEqual(stage(publicKey, 'tampered').status, 0, 'tampered archive must be rejected');
  console.log('PASS: real Tauri signature accepted; wrong public key and tampered artifact rejected. Ephemeral test keys only.');
} finally { rmSync(scratch, { recursive: true, force: true }); }
