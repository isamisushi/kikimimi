// Build on the target Mac; bundle the CLI and DuckDB so end users need neither
// Homebrew nor a shell PATH. Never fetch an executable at application runtime.
import { execFileSync } from 'node:child_process';
import { copyFileSync, chmodSync, mkdirSync, realpathSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyArchitecture } from './macos-binary.mjs';

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const root = resolve(desktop, '..');
const debug = process.argv.includes('--debug');
if (process.platform !== 'darwin') throw new Error('Build desktop on macOS. CLI development remains cross-platform.');
const run = (command, args, cwd = root) => execFileSync(command, args, { cwd, stdio: 'inherit' });
run(resolve(desktop, 'node_modules/.bin/tauri'), ['icon', 'icon.svg', '--output', 'src-tauri/icons'], desktop);
const host = execFileSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' }).trim();
const target = process.env.TAURI_ENV_TARGET_TRIPLE || host;
if (target !== host) throw new Error('Use a native Mac runner for each architecture; cross builds are not supported yet.');
const duckdb = realpathSync(process.env.DUCKDB_BINARY || execFileSync('which', ['duckdb'], { encoding: 'utf8' }).trim());
// Fail before packaging if the dependency cannot run or targets the wrong CPU.
run(duckdb, ['--version']);
verifyArchitecture(duckdb, host.startsWith('aarch64') ? 'arm64' : 'x86_64');
const libraries = execFileSync('otool', ['-L', duckdb], { encoding: 'utf8' })
  .split('\n').slice(1).map((line) => line.trim().split(/\s+/)[0]).filter(Boolean);
const external = libraries.filter((library) => !library.startsWith('/usr/lib/') && !library.startsWith('/System/Library/'));
if (external.length) throw new Error(`DuckDB depends on unbundled libraries: ${external.join(', ')}. Set DUCKDB_BINARY to a standalone macOS CLI build.`);
run('npm', ['ci'], resolve(root, 'web'));
run('npm', ['run', 'build'], resolve(root, 'web'));
run('cargo', ['build', '--locked', ...(debug ? [] : ['--release', '--target', host]), '-p', 'kikimimi', '--bin', 'kikimimi']);
const output = resolve(desktop, 'src-tauri/binaries');
mkdirSync(output, { recursive: true });
const targetDir = process.env.CARGO_TARGET_DIR || resolve(root, 'target');
const cli = debug ? resolve(targetDir, 'debug/kikimimi') : resolve(targetDir, host, 'release/kikimimi');
for (const [name, source] of [['kikimimi', cli], ['duckdb', duckdb]]) {
  const destination = resolve(output, `${name}-${host}`);
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
}
