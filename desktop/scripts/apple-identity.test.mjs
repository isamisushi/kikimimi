import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deriveIdentity, identityFromPem } from './apple-identity.mjs';

const scratch = mkdtempSync(join(tmpdir(), 'kikimimi-apple-identity-test-'));
after(() => rmSync(scratch, { recursive: true, force: true }));
const openssl = process.env.OPENSSL_BIN || 'openssl';
const password = 'throwaway-fixture-password';
const teamId = 'A1B2C3D4E5';
const identity = `Developer ID Application: Test, Inc. (${teamId})`;
function opensslRun(args) {
  try { return execFileSync(openssl, args, { stdio: 'pipe', env: { ...process.env, APPLE_CERTIFICATE_PASSWORD: password } }); }
  catch { throw new Error('Ephemeral certificate fixture generation failed (output withheld)'); }
}
let fixtureNumber = 0;
function certificate(cn = identity, ou = teamId) {
  const path = join(scratch, `fixture-${fixtureNumber++}`);
  opensslRun(['req', '-x509', '-newkey', 'rsa:2048', '-noenc', '-days', '1',
    '-subj', `/C=JP/OU=${ou}/CN=${cn}`, '-addext', 'basicConstraints=critical,CA:FALSE',
    '-keyout', `${path}.key`, '-out', `${path}.pem`]);
  return { path, pem: readFileSync(`${path}.pem`, 'utf8') };
}
const fixture = certificate();
function p12(legacy = false) {
  const output = `${fixture.path}-${legacy}.p12`;
  opensslRun(['pkcs12', '-export', '-inkey', `${fixture.path}.key`, '-in', `${fixture.path}.pem`,
    '-out', output, '-passout', 'env:APPLE_CERTIFICATE_PASSWORD', ...(legacy ? ['-legacy'] : [])]);
  return readFileSync(output).toString('base64');
}
const encoded = p12();
const env = { ...process.env, OPENSSL_BIN: openssl, APPLE_CERTIFICATE: encoded, APPLE_CERTIFICATE_PASSWORD: password };

test('derives exact identity and subject Team ID from a real encrypted P12', () => {
  assert.deepEqual(deriveIdentity(env), { identity, teamId });
});
test('supports legacy Keychain P12 encryption', () => {
  assert.deepEqual(deriveIdentity({ ...env, APPLE_CERTIFICATE: p12(true) }), { identity, teamId });
});
test('rejects wrong password and malformed input without exposing credentials', () => {
  assert.throws(() => deriveIdentity({ ...env, APPLE_CERTIFICATE_PASSWORD: 'incorrect' }), /Cannot read P12/);
  assert.throws(() => deriveIdentity({ ...env, APPLE_CERTIFICATE: 'invalid!' }), /base64 P12/);
  assert.throws(() => deriveIdentity({ ...env, APPLE_CERTIFICATE_PASSWORD: '' }), /Missing/);
});
test('rejects absent, wrong-type, and ambiguous signing certificates', () => {
  assert.throws(() => identityFromPem(''), /exactly one/);
  assert.throws(() => identityFromPem(certificate(`Apple Development: Test (${teamId})`).pem), /exactly one/);
  assert.throws(() => identityFromPem(fixture.pem + fixture.pem), /exactly one/);
});
test('rejects mismatched Team ID and invalid Team ID format', () => {
  assert.throws(() => identityFromPem(certificate(identity, 'Z9Y8X7W6V5').pem), /Invalid/);
  assert.throws(() => identityFromPem(certificate('Developer ID Application: Test (short)', 'short').pem), /Invalid/);
});
test('rejects line breaks that could inject additional GitHub environment variables', () => {
  assert.throws(() => identityFromPem(certificate(`Developer ID Application: Test\nINJECTED=yes (${teamId})`).pem), /Invalid/);
});
test('rejects certificates outside their validity period', () => {
  assert.throws(() => identityFromPem(fixture.pem, 0), /expired or not yet valid/);
  assert.throws(() => identityFromPem(fixture.pem, Date.now() + 7 * 86400_000), /expired or not yet valid/);
});
test('keeps password out of command arguments and requests no private keys', () => {
  deriveIdentity(env, (_, args, options) => {
    assert.ok(args.includes('-nokeys'));
    assert.ok(args.includes('env:APPLE_CERTIFICATE_PASSWORD'));
    assert.ok(!args.some((arg) => arg.includes(password)));
    assert.deepEqual(options.stdio, ['pipe', 'pipe', 'pipe']);
    return fixture.pem;
  });
});
test('CLI exports derived values to GITHUB_ENV, overriding stale settings without logging them', () => {
  const githubEnv = join(scratch, 'github-env');
  const script = fileURLToPath(new URL('./apple-identity.mjs', import.meta.url));
  const result = spawnSync(process.execPath, [script], { encoding: 'utf8', env: {
    ...env, GITHUB_ENV: githubEnv, APPLE_SIGNING_IDENTITY: 'stale', APPLE_TEAM_ID: 'stale',
  } });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(readFileSync(githubEnv, 'utf8'), `APPLE_SIGNING_IDENTITY=${identity}\nAPPLE_TEAM_ID=${teamId}\n`);
  const derivedEnv = Object.fromEntries(readFileSync(githubEnv, 'utf8').trimEnd().split('\n').map((line) => {
    const separator = line.indexOf('=');
    return [line.slice(0, separator), line.slice(separator + 1)];
  }));
  const preflight = spawnSync(process.execPath, [fileURLToPath(new URL('./check-release.mjs', import.meta.url))], {
    cwd: fileURLToPath(new URL('../../', import.meta.url)), encoding: 'utf8', env: {
      ...env, ...derivedEnv, KIKIMIMI_UPDATER_PUBLIC_KEY: 'fixture-public',
      TAURI_SIGNING_PRIVATE_KEY: 'fixture-private', APPLE_ID: 'test@example.invalid', APPLE_PASSWORD: 'fixture-password',
    },
  });
  assert.equal(preflight.status, 0, preflight.stderr);
  for (const secret of [password, encoded, identity, teamId]) {
    assert.ok(!(result.stdout + result.stderr).includes(secret));
  }
  const failure = spawnSync(process.execPath, [script], { encoding: 'utf8', env: {
    ...env, GITHUB_ENV: githubEnv, APPLE_CERTIFICATE_PASSWORD: 'incorrect',
  } });
  assert.equal(failure.status, 1);
  assert.equal(readFileSync(githubEnv, 'utf8'), `APPLE_SIGNING_IDENTITY=${identity}\nAPPLE_TEAM_ID=${teamId}\n`);
  assert.ok(!failure.stderr.includes(encoded));
});
