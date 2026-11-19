/**
 * End-to-end GUI check: the real GUI page in headless Chrome, driving a real camfunge-service.
 *
 *   node tests/gui-check.mjs          direct-preview view (works anywhere with Chrome + a GPU or software adapter)
 *   node tests/gui-check.mjs --vcam   also: the service feeds the OS virtual camera and the GUI shows that device
 *
 * CHROME=/path/to/chrome overrides the browser.
 */
import { createServer } from 'node:http';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ServiceClient } from '../src/client.js';
import { startService, next } from './support/service.mjs';

const CHROME = process.env.CHROME ?? {
  darwin: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  win32: 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
}[process.platform] ?? 'google-chrome';
const VCAM = process.argv.includes('--vcam');
const SRC = fileURLToPath(new URL('../src/', import.meta.url));
const TYPES = { '.html': 'text/html', '.js': 'text/javascript' };

const http = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://x').pathname.replace(/^\/$/, '/index.html');
  try {
    const body = await readFile(join(SRC, path.replace(/\.\.+/g, '')));
    res.writeHead(200, { 'content-type': TYPES[extname(path)] ?? 'application/octet-stream' }).end(body);
  } catch {
    res.writeHead(404).end();
  }
}).listen(0, '127.0.0.1');
await new Promise(r => http.once('listening', r));
const origin = `http://127.0.0.1:${http.address().port}`;

/** Minimal Chrome DevTools Protocol session on one page. */
async function openPage(chromeArgs) {
  const profile = await mkdtemp(join(tmpdir(), 'camfunge-chrome-'));
  const chrome = spawn(CHROME, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run',
    '--enable-unsafe-swiftshader', '--autoplay-policy=no-user-gesture-required', '--window-size=960,600', ...chromeArgs, 'about:blank'],
    { stdio: ['ignore', 'ignore', 'pipe'] });
  const devtools = await new Promise((resolve, reject) => {
    let err = '';
    chrome.stderr.on('data', d => {
      err += d;
      const m = err.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) resolve(m[1]);
    });
    chrome.on('exit', code => reject(new Error(`Chrome exited (${code}): ${err}`)));
  });
  const targets = await (await fetch(`http://${new URL(devtools).host}/json/list`)).json();
  const ws = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((resolve, reject) => Object.assign(ws, { onopen: resolve, onerror: reject }));
  let id = 0;
  const pending = new Map();
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    pending.get(m.id)?.(m);
    pending.delete(m.id);
  };
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      pending.set(++id, m => (m.error ? reject(new Error(`${method}: ${m.error.message}`)) : resolve(m.result)));
      ws.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async expression => {
    const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(`${expression}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  };
  const close = async () => {
    ws.close();
    chrome.kill();
    await new Promise(r => chrome.once('exit', r));
    await rm(profile, { recursive: true, force: true });
  };
  return { call, evaluate, close };
}

const failures = [];
async function waitFor(page, what, expression, ms = 15000) {
  const deadline = Date.now() + ms;
  let last;
  while (Date.now() < deadline) {
    try {
      if ((last = await page.evaluate(expression))) return last;
    } catch (e) {
      last = e.message;
    }
    await new Promise(r => setTimeout(r, 100));
  }
  failures.push(`${what} (last: ${JSON.stringify(last)})`);
  return null;
}

/** Mean brightness of what the GUI is showing (video or canvas), 0..255. */
const SHOWN_BRIGHTNESS = `(() => {
  const el = camfunge.stats.view === 'virtual' ? document.querySelector('#feed') : document.querySelector('#preview');
  const w = el.videoWidth || el.width, h = el.videoHeight || el.height;
  if (!w || !h) return 0;
  const c = Object.assign(document.createElement('canvas'), { width: 64, height: 36 });
  const g = c.getContext('2d', { willReadFrequently: true });
  g.drawImage(el, 0, 0, 64, 36);
  const d = g.getImageData(0, 0, 64, 36).data;
  let sum = 0;
  for (let i = 0; i < d.length; i += 4) sum += d[i] + d[i + 1] + d[i + 2];
  return sum / (d.length / 4) / 3;
})()`;

const service = await startService({ args: ['--allow-origin', origin, ...(VCAM ? ['--virtual-camera', 'auto'] : [])] });
if (service.skip) {
  console.log(`skipped: ${service.skip}`);
  http.close();
  process.exit(0);
}
// An independent client: proves GUI actions really reach the service, and pushes changes the other way.
const observer = new ServiceClient(service.url);
const observed = next(observer, 'hello');
observer.start();
const hello = await observed;
if (VCAM && !hello.virtualCamera.device) throw new Error(`--vcam: service has no virtual camera: ${hello.virtualCamera.error}`);

const page = await openPage(VCAM ? ['--use-fake-ui-for-media-stream'] : []);
try {
  await page.call('Page.navigate', { url: `${origin}/index.html?service=${encodeURIComponent(service.url)}` });

  const filters = await waitFor(page, 'GUI received hello and listed the filters', `document.querySelectorAll('#filter option').length`);
  if (filters !== hello.filters.length) failures.push(`GUI lists ${filters} filters, service has ${hello.filters.length}`);
  await waitFor(page, 'controls enabled once connected', `!document.querySelector('#controls').disabled`);

  const view = VCAM ? 'virtual' : 'direct';
  await waitFor(page, `GUI shows the ${view} view`, `camfunge.stats.view === '${view}' || camfunge.stats.viewError`);
  const err = await page.evaluate('camfunge.stats.viewError');
  if (err) failures.push(`view error: ${err}`);
  if (VCAM) {
    await waitFor(page, 'virtual camera video is playing', `document.querySelector('#feed').videoWidth > 0`);
    console.log(`GUI opened virtual camera device: ${await page.evaluate('camfunge.stats.virtualDevice')}`);
  } else {
    await waitFor(page, 'preview frames are drawn', `camfunge.stats.previewFrames > 5`);
  }
  await waitFor(page, 'the picture is not black', `${SHOWN_BRIGHTNESS} > 10`);

  const picked = next(observer, 'config', c => c.filter === 7);
  await page.evaluate(`(() => { const s = document.querySelector('#filter'); s.value = '7'; s.dispatchEvent(new Event('change')); })()`);
  await picked.catch(() => failures.push('choosing a trip in the GUI did not reach the service'));

  const stepped = next(observer, 'config', c => c.filter === 8);
  await page.evaluate(`document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }))`);
  await stepped.catch(() => failures.push('→ key did not step the trip'));

  observer.set({ intensity: 0.2, autoCycle: true });
  await waitFor(page, 'changes made elsewhere show up in the GUI', `document.querySelector('#intensity').value === '0.2' && document.querySelector('#autoCycle').checked`);
  await waitFor(page, 'level meter and fps update', `/\\d+ fps/.test(document.querySelector('#conn').textContent)`);

  observer.stop();
  await service.stop();
  await waitFor(page, 'GUI notices the service went away', `/Waiting for the CamFunge service/.test(document.querySelector('#msg').textContent)`);
} finally {
  await page.close();
  observer.stop();
  await service.stop();
  http.close();
}

if (failures.length) {
  console.error(failures.join('\n'));
  process.exitCode = 1;
} else {
  console.log(`ok: GUI drove the service and showed its ${VCAM ? 'virtual camera' : 'direct preview'} feed`);
}
