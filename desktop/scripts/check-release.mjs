import { readFileSync } from 'node:fs';
import { version } from './release.mjs';
for (const name of ['KIKIMIMI_UPDATER_PUBLIC_KEY', 'TAURI_SIGNING_PRIVATE_KEY', 'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_ID', 'APPLE_PASSWORD']) {
  if (!process.env[name]?.trim()) throw new Error(`Missing release setting: ${name}`);
}
for (const name of ['APPLE_SIGNING_IDENTITY', 'APPLE_TEAM_ID']) {
  if (!process.env[name]?.trim()) throw new Error(`Missing derived setting: ${name}; run apple-identity.mjs first`);
}
if (!process.env.APPLE_SIGNING_IDENTITY.startsWith('Developer ID Application:')) throw new Error('A Developer ID Application signing identity is required');
for (const file of ['Cargo.toml', 'desktop/src-tauri/Cargo.toml']) {
  const source = readFileSync(file, 'utf8');
  const found = source.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (found !== version) throw new Error(`${file} version must match desktop config ${version}`);
}
if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('The stable channel requires a stable semver version');
