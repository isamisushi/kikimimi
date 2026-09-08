import { execFileSync } from 'node:child_process';
import { X509Certificate } from 'node:crypto';
import { appendFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export function identityFromPem(pem, now = Date.now()) {
  const blocks = pem.match(/-----BEGIN CERTIFICATE-----[\s\S]*?-----END CERTIFICATE-----/g) ?? [];
  const candidates = blocks.map((block) => new X509Certificate(block)).filter((cert) => {
    const cn = cert.toLegacyObject().subject.CN;
    return typeof cn === 'string' && cn.startsWith('Developer ID Application: ');
  });
  if (candidates.length !== 1) throw new Error('Expected exactly one Developer ID Application certificate in the P12');
  const cert = candidates[0];
  const { CN: identity, OU: teamId } = cert.toLegacyObject().subject;
  if (typeof teamId !== 'string' || !/^[A-Z0-9]{10}$/.test(teamId)
      || !identity.endsWith(` (${teamId})`) || /[\x00-\x1f\x7f]/.test(identity) || cert.ca) {
    throw new Error('Invalid Developer ID Application identity or Team ID');
  }
  if (!(Date.parse(cert.validFrom) <= now && now < Date.parse(cert.validTo))) {
    throw new Error('Developer ID Application certificate is expired or not yet valid');
  }
  return { identity, teamId };
}

export function deriveIdentity(env = process.env, run = execFileSync) {
  const encoded = env.APPLE_CERTIFICATE?.replace(/\s/g, '');
  if (!encoded || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(encoded)) {
    throw new Error('APPLE_CERTIFICATE must contain a base64 P12');
  }
  if (!env.APPLE_CERTIFICATE_PASSWORD) throw new Error('Missing APPLE_CERTIFICATE_PASSWORD');
  const options = {
    input: Buffer.from(encoded, 'base64'), encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'],
    env, timeout: 30_000, maxBuffer: 4 * 1024 * 1024,
  };
  // Never export private keys, write the P12 to disk, or place its password in argv.
  const args = ['pkcs12', '-clcerts', '-nokeys', '-passin', 'env:APPLE_CERTIFICATE_PASSWORD'];
  let pem;
  try { pem = run(env.OPENSSL_BIN || 'openssl', args, options); }
  catch {
    // macOS Keychain exports can use legacy PKCS#12 encryption (e.g. RC2).
    try { pem = run(env.OPENSSL_BIN || 'openssl', [...args, '-legacy'], options); }
    catch { throw new Error('Cannot read P12; check the certificate export and its password (OpenSSL output withheld)'); }
  }
  return identityFromPem(pem);
}

export function writeIdentity(env = process.env) {
  if (!env.GITHUB_ENV) throw new Error('GITHUB_ENV is required');
  const { identity, teamId } = deriveIdentity(env);
  appendFileSync(env.GITHUB_ENV, `APPLE_SIGNING_IDENTITY=${identity}\nAPPLE_TEAM_ID=${teamId}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    writeIdentity();
    console.log('Derived Apple signing identity and Team ID from the certificate.');
  } catch (error) {
    // Do not print subprocess error objects: they can include input and output.
    console.error(error.message);
    process.exitCode = 1;
  }
}
