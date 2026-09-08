import assert from 'node:assert/strict';
import { test } from 'node:test';
import { selectDesktopRelease, findDesktopRelease } from '../src/lib/desktop-release.js';
const release = (version, extra = {}) => ({
  tag_name: `desktop-v${version}`, draft: false, prerelease: false,
  assets: ['aarch64', 'x86_64'].map((arch) => ({
    name: `kikimimi-desktop-${version}-${arch}.dmg`,
    browser_download_url: `https://github.com/isamisushi/kikimimi/releases/download/desktop-v${version}/kikimimi-desktop-${version}-${arch}.dmg`,
  })), ...extra,
});
test('selects a complete stable desktop release independently of CLI/channel releases', () => {
  const result = selectDesktopRelease([
    { tag_name: 'v9.0.0' }, { tag_name: 'desktop-updates' },
    release('0.8.0'), release('0.10.0'), release('0.11.0', { prerelease: true }),
    release('0.12.0', { draft: true }), release('0.13.0', { assets: [] }),
  ]);
  assert.equal(result.version, '0.10.0');
  assert.ok(result.urls[0].endsWith('-aarch64.dmg'));
  assert.ok(result.urls[1].endsWith('-x86_64.dmg'));
});
test('never offers incomplete or off-repository installers', () => {
  const incomplete = release('0.7.1');
  incomplete.assets.pop();
  const unsafe = release('0.7.2');
  unsafe.assets[0].browser_download_url = 'https://example.org/installer.dmg';
  assert.equal(selectDesktopRelease([incomplete, unsafe]), null);
  assert.equal(selectDesktopRelease([]), null);
});
test('paginates past CLI releases', async () => {
  const calls = [];
  const result = await findDesktopRelease(async (url) => {
    calls.push(url);
    return { ok: true, json: async () => calls.length === 1 ? Array.from({ length: 100 }, () => ({ tag_name: 'v1.0.0' })) : [release('0.7.1')] };
  });
  assert.equal(result.version, '0.7.1');
  assert.match(calls[1], /page=2$/);
});
test('distinguishes unpublished releases from network/API errors', async () => {
  assert.equal(await findDesktopRelease(async () => ({ ok: true, json: async () => [] })), null);
  await assert.rejects(findDesktopRelease(async () => ({ ok: false })), /unavailable/);
  await assert.rejects(findDesktopRelease(async () => ({ ok: true, json: async () => ({}) })), /Invalid/);
});
