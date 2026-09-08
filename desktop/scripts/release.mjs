// Stage immutable, per-version update archives and generate the complete channel
// manifest only after both architectures have built successfully.
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const version = JSON.parse(readFileSync(join(desktop, 'src-tauri/tauri.conf.json'))).version;
export function requireNewer(next, current) {
  const parse = (value) => {
    if (!/^\d+\.\d+\.\d+$/.test(value)) throw new Error('Expected stable semver');
    return value.split('.').map(Number);
  };
  const a = parse(next), b = parse(current);
  for (let i = 0; i < 3; i++) {
    if (a[i] > b[i]) return;
    if (a[i] < b[i]) break;
  }
  throw new Error(`Channel cannot go backward or repeat: ${current} -> ${next}`);
}
const repository = 'isamisushi/kikimimi';
const platforms = ['aarch64', 'x86_64'];

function walk(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => entry.isDirectory() ? walk(join(directory, entry.name)) : [join(directory, entry.name)]);
}
export function manifest(archives, date = new Date().toISOString()) {
  const entries = {};
  for (const arch of platforms) {
    const name = `kikimimi-desktop-${version}-${arch}.app.tar.gz`;
    const signature = archives[name];
    if (typeof signature !== 'string' || !signature.trim()) throw new Error(`Missing signature for ${arch}`);
    const decoded = Buffer.from(signature.trim(), 'base64').toString('utf8');
    if (!decoded.startsWith('untrusted comment:') || decoded.trim().split('\n').length !== 4) throw new Error(`Invalid signature for ${arch}`);
    entries[`darwin-${arch}`] = { signature: signature.trim(), url: `https://github.com/${repository}/releases/download/desktop-v${version}/${name}` };
  }
  return { version, notes: `kikimimi desktop ${version}`, pub_date: date, platforms: entries };
}

function main() {
  const [command, source, destination, arch] = process.argv.slice(2);
  if (command === 'stage') {
    if (!platforms.includes(arch)) throw new Error('Expected aarch64 or x86_64');
    const publicKey = process.env.KIKIMIMI_UPDATER_PUBLIC_KEY?.trim();
    if (!publicKey || !Buffer.from(publicKey, 'base64').toString('utf8').startsWith('untrusted comment:')) throw new Error('Release public key is required');
    const files = walk(resolve(source));
    const archives = files.filter((file) => file.endsWith('.app.tar.gz'));
    const dmgs = files.filter((file) => file.endsWith('.dmg'));
    if (archives.length !== 1 || dmgs.length !== 1) throw new Error('Expected exactly one app archive and one DMG');
    // Check the actual artifact with the same public key embedded in this build.
    // A mismatched CI private/public key must fail before anything is published.
    const scratch = mkdtempSync(join(tmpdir(), 'kikimimi-signature-'));
    try {
      const decoded = Buffer.from(readFileSync(`${archives[0]}.sig`, 'utf8').trim(), 'base64');
      const signatureFile = join(scratch, 'archive.minisig');
      writeFileSync(signatureFile, decoded);
      const key = Buffer.from(publicKey, 'base64').toString('utf8').trim().split('\n')[1];
      execFileSync('minisign', ['-V', '-P', key, '-m', archives[0], '-x', signatureFile], { stdio: 'pipe' });
    } finally { rmSync(scratch, { recursive: true, force: true }); }
    mkdirSync(destination, { recursive: true });
    const name = `kikimimi-desktop-${version}-${arch}`;
    copyFileSync(archives[0], join(destination, `${name}.app.tar.gz`));
    copyFileSync(`${archives[0]}.sig`, join(destination, `${name}.app.tar.gz.sig`));
    copyFileSync(dmgs[0], join(destination, `${name}.dmg`));
  } else if (command === 'manifest') {
    const signatures = {};
    for (const file of walk(resolve(source)).filter((file) => file.endsWith('.app.tar.gz.sig'))) {
      const name = basename(file).slice(0, -4);
      if (signatures[name]) throw new Error(`Duplicate signature: ${name}`);
      readFileSync(file.slice(0, -4)); // Must have its matching archive too.
      signatures[name] = readFileSync(file, 'utf8');
    }
    writeFileSync(destination, `${JSON.stringify(manifest(signatures), null, 2)}\n`);
  } else { throw new Error('Usage: release.mjs stage <bundle> <output> <arch> | manifest <artifacts> <output.json>'); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
