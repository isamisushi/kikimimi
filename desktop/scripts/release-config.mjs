import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const outputPath = fileURLToPath(new URL('../src-tauri/tauri.release.generated.conf.json', import.meta.url));
const template = JSON.parse(readFileSync(new URL('../src-tauri/tauri.release.conf.json', import.meta.url), 'utf8'));

export function releaseConfig(env = process.env) {
  const pubkey = env.KIKIMIMI_UPDATER_PUBLIC_KEY?.trim();
  const decoded = pubkey && Buffer.from(pubkey, 'base64').toString('utf8').trim().split(/\r?\n/);
  const packet = decoded?.length === 2 ? Buffer.from(decoded[1], 'base64') : Buffer.alloc(0);
  if (!decoded?.[0]?.startsWith('untrusted comment:') || packet.length !== 42 || packet.subarray(0, 2).toString() !== 'Ed') {
    throw new Error('KIKIMIMI_UPDATER_PUBLIC_KEY must contain the Tauri .pub file contents');
  }
  // The same environment variable is embedded into the app by Rust. Only the
  // public key belongs in bundler configuration; never serialize the environment.
  return {
    ...template,
    plugins: { ...template.plugins, updater: { ...template.plugins?.updater, pubkey } },
  };
}

export function writeReleaseConfig(env = process.env, destination = outputPath) {
  writeFileSync(destination, `${JSON.stringify(releaseConfig(env), null, 2)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { writeReleaseConfig(); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
