import { test } from 'node:test';
import assert from 'node:assert/strict';
import { manifest, version, requireNewer } from './release.mjs';
import { publish } from './publish-release.mjs';
const signature = Buffer.from('untrusted comment: fixture\nfixture\ntrusted comment: fixture\nfixture\n').toString('base64');
const files = Object.fromEntries(['aarch64', 'x86_64'].map((arch) => [`kikimimi-desktop-${version}-${arch}.app.tar.gz`, signature]));
test('channel uses both architectures and immutable desktop release URLs', () => {
  const result = manifest(files, '2026-01-01T00:00:00Z');
  assert.deepEqual(Object.keys(result.platforms), ['darwin-aarch64', 'darwin-x86_64']);
  for (const [arch, platform] of Object.entries(result.platforms)) {
    assert.ok(platform.url.includes(`/desktop-v${version}/`));
    assert.ok(platform.url.endsWith(`${arch.slice(7)}.app.tar.gz`));
    assert.equal(platform.signature, signature);
  }
});
test('never advance the channel with a missing platform or malformed signature', () => {
  assert.throws(() => manifest({}), /Missing signature/);
  assert.throws(() => manifest({ ...files, [`kikimimi-desktop-${version}-x86_64.app.tar.gz`]: 'bad' }), /Invalid signature/);
});
test('channel cannot downgrade, repeat a version, or accept prereleases', () => {
  requireNewer('0.10.0', '0.9.9');
  assert.throws(() => requireNewer('0.9.9', '0.10.0'), /backward/);
  assert.throws(() => requireNewer('1.0.0', '1.0.0'), /repeat/);
  assert.throws(() => requireNewer('2.0.0-beta', '1.0.0'), /stable/);
});
function publishingFixture(pages, previousVersion = '0.0.1') {
  const writes = [];
  return { writes, options: {
    sha: 'review-fixture', list: () => ['fixture.app.tar.gz'],
    read: () => JSON.stringify(manifest(files)),
    run: (_, args) => {
      if (args[0] === 'api') {
        assert.ok(args.includes('--paginate'));
        assert.ok(args.includes('--slurp'));
        return JSON.stringify(pages);
      }
      if (args[1] === 'download') return JSON.stringify({ version: previousVersion });
      writes.push(args);
    },
  } };
}
test('an update channel beyond page one is reused and advanced last', () => {
  const firstPage = Array.from({ length: 100 }, (_, i) => ({ tag_name: `cli-${i}`, assets: [] }));
  const fixture = publishingFixture([firstPage, [{ tag_name: 'desktop-updates', assets: [{ name: 'latest.json' }] }]]);
  publish(fixture.options);
  assert.equal(fixture.writes.length, 2);
  assert.equal(fixture.writes[0][2], `desktop-v${version}`);
  assert.deepEqual(fixture.writes[1].slice(0, 3), ['release', 'upload', 'desktop-updates']);
});
test('an old channel with a newer version prevents all publication', () => {
  const fixture = publishingFixture([[], [{ tag_name: 'desktop-updates', assets: [{ name: 'latest.json' }] }]], '999.0.0');
  assert.throws(() => publish(fixture.options), /backward/);
  assert.deepEqual(fixture.writes, []);
});
test('a release on a later page is never overwritten', () => {
  const fixture = publishingFixture([[], [{ tag_name: `desktop-v${version}`, assets: [] }]]);
  assert.throws(() => publish(fixture.options), /already exists/);
  assert.deepEqual(fixture.writes, []);
});
