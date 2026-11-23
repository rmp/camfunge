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

const host = run('rustc', ['-vV']).match(/^host: (\S+)/m)[1];
// Set by the Tauri CLI; differs from the host under `tauri build --target`.
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const cross = triple !== host;
run('cargo', ['build', '-p', 'camfunge-service', ...(release ? ['--release'] : []), ...(cross ? ['--target', triple] : [])]);
const ext = process.platform === 'win32' ? '.exe' : '';
mkdirSync(`${root}/src-tauri/binaries`, { recursive: true });
copyFileSync(`${root}/target/${cross ? `${triple}/` : ''}${release ? 'release' : 'debug'}/camfunge-service${ext}`, `${root}/src-tauri/binaries/camfunge-service-${triple}${ext}`);
console.log(`staged camfunge-service for ${triple}`);
