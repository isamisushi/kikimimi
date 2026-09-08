import { test } from 'node:test';
import assert from 'node:assert/strict';
import { verifyArchitecture } from './macos-binary.mjs';

test('lipo receives the binary before the variable-length architecture list', () => {
  for (const arch of ['arm64', 'x86_64']) {
    verifyArchitecture('/Applications/Test App.app/Contents/MacOS/duckdb', arch, (command, args) => {
      assert.equal(command, '/usr/bin/lipo');
      assert.deepEqual(args, ['/Applications/Test App.app/Contents/MacOS/duckdb', '-verify_arch', arch]);
    });
  }
});
test('architecture failures stop packaging', () => {
  assert.throws(() => verifyArchitecture('/fixture', 'arm64', () => { throw new Error('wrong architecture'); }), /wrong architecture/);
});
test('real macOS lipo accepts the current Node executable', { skip: process.platform !== 'darwin' }, () => {
  verifyArchitecture(process.execPath, process.arch === 'arm64' ? 'arm64' : 'x86_64');
});
