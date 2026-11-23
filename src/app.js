import { ServiceClient, pickVirtualCamera, serviceUrl } from './client.js';

const $ = s => document.querySelector(s);
const feed = $('#feed'), preview = $('#preview'), msg = $('#msg'), note = $('#note'), bar = $('#bar'), conn = $('#conn');
const RANGES = ['intensity', 'speed', 'reactivity'], CHECKS = ['autoCycle', 'gestures', 'mirror'];

const client = new ServiceClient(serviceUrl()).start();
/** What the GUI is showing; read by the headless GUI check (tests/gui-check.mjs) and handy in devtools. */
const stats = { view: 'none', previewFrames: 0, virtualDevice: null, viewError: null };
window.camfunge = { client, stats };

const say = text => { msg.textContent = text; };
let noteTimer;
function flash(text, ms = 6000) {
  note.textContent = text;
  note.classList.add('on');
  clearTimeout(noteTimer);
  noteTimer = setTimeout(() => note.classList.remove('on'), ms);
}

function applyConfig(c) {
  for (const k of RANGES) if (document.activeElement !== $('#' + k)) $('#' + k).value = c[k];
  for (const k of CHECKS) $('#' + k).checked = c[k];
  $('#filter').value = c.filter;
  $('#reactTo').value = c.reactTo;
  $('#camera').value = c.camera ?? '';
}

// ---- service → GUI ----

client.addEventListener('state', ({ detail: state }) => {
  conn.dataset.state = state;
  $('#controls').disabled = state !== 'open';
  if (state === 'open') conn.textContent = 'connected';
  else {
    conn.textContent = state;
    say(`Waiting for the CamFunge service at ${client.url}…`);
  }
});

client.addEventListener('hello', ({ detail: hello }) => {
  say('');
  $('#filter').replaceChildren(...hello.filters.map((f, i) => new Option(`${f.name} — ${f.artist}`, i)));
  $('#camera').replaceChildren(new Option('Default camera', ''), ...hello.cameras.map(c => new Option(c.name, c.id)));
  applyConfig(hello.config);
  const vc = hello.virtualCamera;
  if (vc.error) flash(`No virtual camera (${vc.backend}): ${vc.error}`, 12000);
  updateView();
});

client.addEventListener('config', ({ detail }) => applyConfig(detail));

client.addEventListener('status', ({ detail: s }) => {
  $('#level').value = s.level;
  const vc = client.hello?.virtualCamera;
  conn.textContent = `${Math.round(s.fps)} fps${vc?.device ? ` → ${vc.device}` : ''}`;
  if (stats.view !== 'none' || s.error) say(s.error ?? '');
});

client.addEventListener('error', ({ detail }) => flash(detail.message));

// ---- GUI → service ----

for (const k of RANGES) $('#' + k).addEventListener('input', e => client.set({ [k]: +e.target.value }));
for (const k of CHECKS) $('#' + k).addEventListener('change', e => client.set({ [k]: e.target.checked }));
$('#filter').onchange = e => client.set({ filter: +e.target.value });
$('#reactTo').onchange = e => client.set({ reactTo: e.target.value });
$('#camera').onchange = e => client.set({ camera: e.target.value || null });
$('#grabBg').onclick = () => client.grabBackground();
addEventListener('keydown', e => {
  if (e.target.matches?.('select, input, button')) return;
  if (e.key === 'ArrowRight') client.step(1);
  if (e.key === 'ArrowLeft') client.step(-1);
  if (e.key.toLowerCase() === 'h') bar.classList.toggle('hidden');
  if (e.key.toLowerCase() === 'b') client.grabBackground();
});

// ---- what to show: the virtual camera device itself, or the service's direct preview ----

const viewSelect = $('#view');
viewSelect.value = localStorage.getItem('camfunge.view') ?? 'virtual';
viewSelect.onchange = () => {
  localStorage.setItem('camfunge.view', viewSelect.value);
  updateView();
};

let vcamStream = null;
function stopVirtualCamera() {
  vcamStream?.getTracks().forEach(t => t.stop());
  vcamStream = null;
  feed.srcObject = null;
  feed.classList.remove('on');
}

/** Opens the virtual camera like any other app would. Labels stay hidden until camera permission is granted. */
async function showVirtualCamera(name, output) {
  const media = navigator.mediaDevices;
  if (!media?.getUserMedia) throw new Error('this webview cannot open cameras');
  let device = pickVirtualCamera(await media.enumerateDevices(), name);
  if (!device) {
    (await media.getUserMedia({ video: true })).getTracks().forEach(t => t.stop());
    device = pickVirtualCamera(await media.enumerateDevices(), name);
  }
  if (!device) throw new Error(`"${name}" is not visible to this window`);
  const stream = await media.getUserMedia({ video: { deviceId: { exact: device.deviceId }, width: { ideal: output.width }, height: { ideal: output.height } } });
  stopVirtualCamera();
  vcamStream = feed.srcObject = stream;
  await feed.play();
  stats.virtualDevice = device.label;
}

let viewGeneration = 0;
async function updateView() {
  const hello = client.hello, generation = ++viewGeneration;
  if (!hello) return;
  const device = hello.virtualCamera.device;
  if (viewSelect.value === 'virtual' && device) {
    try {
      await showVirtualCamera(device, hello.output);
      if (generation !== viewGeneration) return;
      client.preview(null);
      preview.classList.remove('on');
      feed.classList.add('on');
      stats.view = 'virtual';
      stats.viewError = null;
      return;
    } catch (e) {
      if (generation !== viewGeneration) return;
      stats.viewError = e.message;
      flash(`Showing the direct feed — couldn't open the virtual camera: ${e.message}`);
    }
  }
  stopVirtualCamera();
  const longest = Math.max(innerWidth, innerHeight) * devicePixelRatio;
  client.preview(Math.round(Math.min(longest, Math.max(hello.output.width, hello.output.height))));
  preview.classList.add('on');
  stats.view = 'direct';
}

const ink = preview.getContext('2d');
let decoding = false;
client.addEventListener('preview', async ({ detail: jpeg }) => {
  if (decoding || stats.view !== 'direct') return; // drop frames rather than queue them
  decoding = true;
  try {
    const frame = await createImageBitmap(new Blob([jpeg], { type: 'image/jpeg' }));
    if (preview.width !== frame.width || preview.height !== frame.height) Object.assign(preview, { width: frame.width, height: frame.height });
    ink.drawImage(frame, 0, 0);
    frame.close();
    stats.previewFrames++;
  } catch {
    // a truncated frame; the next one will do
  } finally {
    decoding = false;
  }
});

navigator.mediaDevices?.addEventListener?.('devicechange', () => stats.view === 'virtual' || updateView());
