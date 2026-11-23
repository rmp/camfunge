/**
 * Talks to camfunge-service over its WebSocket control protocol (see service/src/protocol.rs).
 * DOM-free so it runs (and is tested) under Node as well as in the GUI's webview.
 */

export const DEFAULT_SERVICE_URL = 'ws://127.0.0.1:7272';

/**
 * Emits:
 *  - `state`   detail: 'connecting' | 'open' | 'closed'
 *  - `hello`   detail: hello message (filters, cameras, config, virtualCamera, output, gpu)
 *  - `config`  detail: the whole config, whenever anything changed it
 *  - `status`  detail: { level, fps, error }
 *  - `error`   detail: { message } — the service rejected a request
 *  - `preview` detail: ArrayBuffer holding one JPEG frame
 */
export class ServiceClient extends EventTarget {
  constructor(url = DEFAULT_SERVICE_URL, { WebSocketImpl = globalThis.WebSocket, retryMs = 1000 } = {}) {
    super();
    this.url = url;
    this.WebSocketImpl = WebSocketImpl;
    this.retryMs = retryMs;
    this.state = 'closed';
    this.hello = null;
    this.config = null;
    this.previewWanted = null;
    this.stopped = true;
  }

  start() {
    this.stopped = false;
    this.connect();
    return this;
  }

  stop() {
    this.stopped = true;
    clearTimeout(this.retry);
    this.ws?.close();
  }

  setState(state) {
    if (state === this.state) return;
    this.state = state;
    this.emit('state', state);
  }

  emit(type, detail) {
    this.dispatchEvent(Object.assign(new Event(type), { detail }));
  }

  connect() {
    this.setState('connecting');
    const ws = (this.ws = new this.WebSocketImpl(this.url));
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      this.setState('open');
      // A restarted service has forgotten our preview subscription.
      if (this.previewWanted) this.send({ type: 'preview', enabled: true, maxSize: this.previewWanted });
    };
    ws.onmessage = e => this.receive(e.data);
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.setState('closed');
      if (!this.stopped) this.retry = setTimeout(() => this.connect(), this.retryMs);
    };
    ws.onerror = () => {};
  }

  receive(data) {
    if (typeof data !== 'string') return this.emit('preview', data);
    let msg;
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }
    if (msg.type === 'hello') {
      this.hello = msg;
      this.config = msg.config;
    }
    if (msg.type === 'config') this.config = msg.config;
    this.emit(msg.type, msg.type === 'config' ? msg.config : msg);
  }

  /** Sends a raw protocol message; false if not connected. */
  send(msg) {
    if (this.ws?.readyState !== 1) return false;
    this.ws.send(JSON.stringify(msg));
    return true;
  }

  set(config) {
    return this.send({ type: 'set', config });
  }

  step(by) {
    return this.send({ type: 'step', by });
  }

  grabBackground() {
    return this.send({ type: 'grabBackground' });
  }

  refresh() {
    return this.send({ type: 'refresh' });
  }

  /** Ask for (maxSize > 0) or stop (falsy) JPEG preview frames. Survives reconnects. */
  preview(maxSize) {
    this.previewWanted = maxSize || null;
    return this.send(maxSize ? { type: 'preview', enabled: true, maxSize } : { type: 'preview', enabled: false });
  }
}

/** The MediaDeviceInfo for the virtual camera called `name` (exact match first, then loose), or null. */
export function pickVirtualCamera(devices, name) {
  if (!name) return null;
  const cams = devices.filter(d => d.kind === 'videoinput');
  const want = name.trim().toLowerCase();
  return cams.find(d => d.label.trim().toLowerCase() === want) ?? cams.find(d => d.label.toLowerCase().includes(want)) ?? null;
}

/** Resolves the service URL: injected by the Tauri shell, else `?service=`, else the default. */
export function serviceUrl(win = globalThis) {
  return win.CAMFUNGE_SERVICE ?? new URLSearchParams(win.location?.search ?? '').get('service') ?? DEFAULT_SERVICE_URL;
}
