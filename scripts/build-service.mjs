/**
 * Builds camfunge-service and stages it where Tauri bundles sidecars from
 * (src-tauri/binaries/camfunge-service-<target-triple>[.exe]). `--release` for bundles.
 */
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const release = process.argv.includes('--release');
const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: ['ignore', 'pipe', 'inherit'], encoding: 'utf8' });

run('cargo', ['build', '-p', 'camfunge-service', ...(release ? ['--release'] : [])]);
const triple = run('rustc', ['-vV']).match(/^host: (\S+)/m)[1];
const ext = process.platform === 'win32' ? '.exe' : '';
mkdirSync(`${root}/src-tauri/binaries`, { recursive: true });
copyFileSync(`${root}/target/${release ? 'release' : 'debug'}/camfunge-service${ext}`, `${root}/src-tauri/binaries/camfunge-service-${triple}${ext}`);
console.log(`staged camfunge-service for ${triple}`);
