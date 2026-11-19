/** Starts a real camfunge-service (test-pattern camera, no virtual camera) on a free port for tests. */
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../..', import.meta.url));
export const SERVICE_BIN = `${root}target/debug/camfunge-service${process.platform === 'win32' ? '.exe' : ''}`;

/** Resolves to { url, port, proc, stop() }, or { skip: reason } when this machine cannot render (no GPU). */
export async function startService({ port = 0, args: extraArgs = [] } = {}) {
  if (!existsSync(SERVICE_BIN)) throw new Error(`${SERVICE_BIN} not built — run: cargo build -p camfunge-service`);
  const defaults = extraArgs.includes('--virtual-camera') ? [] : ['--virtual-camera', 'none'];
  const proc = spawn(SERVICE_BIN, ['--port', String(port), '--camera', 'test-pattern', '--exit-with-stdin', ...defaults, ...extraArgs], {
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  proc.stderr.on('data', d => (stderr += d));
  const url = await new Promise((resolve, reject) => {
    let stdout = '';
    proc.stdout.on('data', d => {
      stdout += d;
      const m = stdout.match(/listening on (ws:\/\/\S+)/);
      if (m) resolve(m[1]);
    });
    proc.on('exit', code => reject(Object.assign(new Error(`service exited (${code}): ${stderr}`), { stderr })));
    setTimeout(() => reject(new Error(`service did not start: ${stderr}`)), 30000).unref();
  }).catch(e => {
    if (/no GPU adapter/.test(e.stderr ?? '') && !process.env.CAMFUNGE_REQUIRE_GPU) return { skip: 'no GPU adapter on this machine' };
    throw e;
  });
  if (url.skip) return url;
  const stop = () =>
    new Promise(resolve => {
      if (proc.exitCode !== null) return resolve(proc.exitCode);
      proc.once('exit', resolve);
      proc.stdin.end(); // --exit-with-stdin: graceful shutdown, like the GUI does
    });
  return { url, port: +new URL(url).port, proc, stop };
}

/** Resolves with the first `type` event from `target` whose detail satisfies `pred`. */
export function next(target, type, pred = () => true, ms = 10000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${type}`)), ms);
    timer.unref?.();
    target.addEventListener(type, function on(e) {
      if (!pred(e.detail)) return;
      clearTimeout(timer);
      target.removeEventListener(type, on);
      resolve(e.detail);
    });
  });
}
