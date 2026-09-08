import { execFileSync } from 'node:child_process';

export function verifyArchitecture(binary, arch, run = execFileSync) {
  // -verify_arch consumes all remaining arguments as architectures.
  run('/usr/bin/lipo', [binary, '-verify_arch', arch], { stdio: 'pipe' });
}
