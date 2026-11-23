/** The GUI's protocol client against a real headless service. */
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { ServiceClient, pickVirtualCamera, serviceUrl, DEFAULT_SERVICE_URL } from '../src/client.js';
import { startService, next } from './support/service.mjs';

test('pickVirtualCamera matches the device label exactly, then loosely, ignoring non-cameras', () => {
  const devices = [
    { kind: 'audioinput', label: 'OBS Virtual Camera', deviceId: 'mic' },
    { kind: 'videoinput', label: 'FaceTime HD Camera', deviceId: 'a' },
    { kind: 'videoinput', label: 'OBS Virtual Camera Extra', deviceId: 'b' },
    { kind: 'videoinput', label: 'OBS Virtual Camera', deviceId: 'c' },
  ];
  assert.equal(pickVirtualCamera(devices, 'obs virtual camera').deviceId, 'c');
  assert.equal(pickVirtualCamera(devices.slice(0, 3), 'OBS Virtual Camera').deviceId, 'b');
  assert.equal(pickVirtualCamera(devices, 'CamFunge'), null);
  assert.equal(pickVirtualCamera(devices, null), null);
  assert.equal(pickVirtualCamera([{ kind: 'videoinput', label: '', deviceId: 'x' }], 'CamFunge'), null, 'labels hidden before permission');
});

test('serviceUrl: injected by the shell, else ?service=, else default', () => {
  assert.equal(serviceUrl({ CAMFUNGE_SERVICE: 'ws://127.0.0.1:1' }), 'ws://127.0.0.1:1');
  assert.equal(serviceUrl({ location: { search: '?service=ws%3A%2F%2Fh%3A2' } }), 'ws://h:2');
  assert.equal(serviceUrl({ location: { search: '' } }), DEFAULT_SERVICE_URL);
});

test('send is refused while disconnected', () => {
  assert.equal(new ServiceClient('ws://127.0.0.1:9').set({ filter: 1 }), false);
});

let service, client;
before(async () => {
  service = await startService();
  if (service.skip) return;
  client = new ServiceClient(service.url, { retryMs: 100 });
});
after(async () => {
  client?.stop();
  await service?.stop?.();
});

function needsService(t) {
  if (service.skip) t.skip(service.skip);
  return !service.skip;
}

test('hello describes filters, cameras, config, output and the (disabled) virtual camera', async t => {
  if (!needsService(t)) return;
  const hello = next(client, 'hello');
  client.start();
  const h = await hello;
  assert.equal(client.state, 'open');
  assert.ok(h.filters.length > 20);
  assert.deepEqual(h.filters[0], { name: 'Straight', artist: 'no filter' });
  assert.ok(h.cameras.some(c => c.id === 'test-pattern'));
  assert.equal(h.config.camera, 'test-pattern');
  assert.deepEqual(h.output, { width: 1280, height: 720, fps: 30 });
  assert.deepEqual(h.virtualCamera, { backend: 'none', device: null, error: null });
  assert.ok(h.gpu.length > 0);
});

test('set is applied, clamped and broadcast to every client', async t => {
  if (!needsService(t)) return;
  const other = new ServiceClient(service.url);
  const otherReady = next(other, 'hello');
  other.start();
  await otherReady;
  try {
    const [seen, mine] = [next(other, 'config', c => c.filter === 5), next(client, 'config', c => c.filter === 5)];
    client.set({ filter: 5, intensity: 7, reactTo: 'colour', mirror: false });
    const [c] = await Promise.all([seen, mine]);
    assert.equal(c.intensity, 1, 'clamped');
    assert.equal(c.reactTo, 'colour');
    assert.equal(c.mirror, false);
    assert.equal(client.config.filter, 5);
  } finally {
    other.stop();
  }
});

test('step wraps around the filter list', async t => {
  if (!needsService(t)) return;
  const count = client.hello.filters.length;
  if (client.config.filter !== 0) {
    const reset = next(client, 'config', c => c.filter === 0);
    client.set({ filter: 0 });
    await reset;
  }
  const wrapped = next(client, 'config', c => c.filter === count - 1);
  client.step(-1);
  await wrapped;
});

test('status reports level and frame rate', async t => {
  if (!needsService(t)) return;
  const s = await next(client, 'status');
  assert.ok(s.level >= 0 && s.level <= 1);
  assert.ok(s.fps > 5, `fps ${s.fps}`);
  assert.equal(s.error, null);
});

test('bad requests get an error, not a disconnect', async t => {
  if (!needsService(t)) return;
  const err = next(client, 'error');
  client.send({ type: 'set', config: { volume: 11 } });
  assert.match((await err).message, /volume/);
  assert.equal(client.state, 'open');
});

test('preview frames are JPEGs no larger than asked for', async t => {
  if (!needsService(t)) return;
  client.preview(320);
  const jpeg = new Uint8Array(await next(client, 'preview'));
  assert.deepEqual([...jpeg.slice(0, 2)], [0xff, 0xd8]);
  const sof = jpeg.findIndex((b, i) => b === 0xff && jpeg[i + 1] === 0xc0);
  const width = (jpeg[sof + 7] << 8) | jpeg[sof + 8];
  assert.equal(width, 320);
  client.preview(null);
});

test('the client reconnects to a restarted service and resubscribes to previews', async t => {
  if (!needsService(t)) return;
  const port = service.port;
  const lonely = new ServiceClient(`ws://127.0.0.1:${port}`, { retryMs: 100 });
  try {
    const firstHello = next(lonely, 'hello');
    lonely.start();
    await firstHello;
    lonely.preview(160);
    await next(lonely, 'preview');

    const closed = next(lonely, 'state', s => s === 'closed');
    await service.stop();
    await closed;
    const rehello = next(lonely, 'hello', () => true, 15000);
    service = await startService({ port });
    await rehello;
    await next(lonely, 'preview');
  } finally {
    lonely.stop();
  }
  // The suite's main client reconnects too.
  if (client.state !== 'open') await next(client, 'state', s => s === 'open', 15000);
});

test('browser pages from unknown origins are refused', async t => {
  if (!needsService(t)) return;
  const { request } = await import('node:http');
  const status = await new Promise((resolve, reject) => {
    request(`http://127.0.0.1:${service.port}/`, {
      headers: {
        Connection: 'Upgrade', Upgrade: 'websocket', 'Sec-WebSocket-Version': '13',
        'Sec-WebSocket-Key': 'dGhlIHNhbXBsZSBub25jZQ==', Origin: 'https://evil.example',
      },
    }).on('response', r => resolve(r.statusCode)).on('upgrade', r => resolve(r.statusCode)).on('error', reject).end();
  });
  assert.equal(status, 403);
});
