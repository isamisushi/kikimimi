import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { version, requireNewer } from './release.mjs';

const repository = 'isamisushi/kikimimi';
const tag = `desktop-v${version}`;
export function publish({ run = execFileSync, read = readFileSync, list = readdirSync, sha = process.env.GITHUB_SHA } = {}) {
const gh = (...args) => run('gh', [...args, '--repo', repository], { stdio: 'inherit' });
// --slurp keeps pages separate; flatten only after every page has succeeded.
// A channel older than the latest 100 releases must still be found.
const releases = JSON.parse(run('gh', ['api', '--paginate', '--slurp', `repos/${repository}/releases?per_page=100`], { encoding: 'utf8' })).flat();
if (releases.some((release) => release.tag_name === tag)) throw new Error(`${tag} already exists`);
const assets = list('desktop/release-artifacts').map((file) => join('desktop/release-artifacts', file));
const manifest = JSON.parse(read('desktop/latest.json', 'utf8'));
if (manifest.version !== version || Object.keys(manifest.platforms).length !== 2) throw new Error('Incomplete update manifest');
const channel = releases.find((release) => release.tag_name === 'desktop-updates');
if (channel?.assets.some((asset) => asset.name === 'latest.json')) {
  const previous = JSON.parse(run('gh', ['release', 'download', 'desktop-updates', '--pattern', 'latest.json', '--output', '-', '--repo', repository], { encoding: 'utf8' }));
  requireNewer(version, previous.version);
}
gh('release', 'create', tag, ...assets, '--target', sha, '--title', `kikimimi desktop ${version}`, '--notes', 'Signed and notarized macOS desktop release.', '--latest=false');
if (!channel) {
  gh('release', 'create', 'desktop-updates', '--target', sha, '--title', 'Desktop update channel', '--notes', 'Update manifest for kikimimi desktop. Versioned archives live in desktop releases.', '--prerelease', '--latest=false');
}
// One file update, only after both platforms' immutable downloads are public.
gh('release', 'upload', 'desktop-updates', 'desktop/latest.json', '--clobber');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) publish();
