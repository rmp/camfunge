# CamFunge

Music-inspired psychedelic camera filters (31 "trips" — The Beatles, Hendrix, Joplin, Pink Floyd, Rob Zombie)
that react to movement, colour or sound, published as a **virtual camera** you can pick in Zoom, Meet, OBS, etc.

## Architecture

```
┌──────────────────────── camfunge-service (headless CLI, Rust) ───────────────────────┐
│ camera (nokhwa) ──┐                                                                  │
│ microphone (cpal) ┤→ Engine: reactivity, auto-cycle, wave gestures → uniforms        │
│                   └→ Renderer (wgpu: Metal/Vulkan/DX12/GL; the GLSL filters via naga)│
│                        │ BGRA frame                                                  │
│                        ├→ virtual camera  (OBS ext. · OBS shm · v4l2loopback · none) │
│                        └→ JPEG preview to subscribed clients                         │
│ WebSocket control server  ws://127.0.0.1:7272  (JSON, see service/src/protocol.rs)   │
└──────────────────────────────────────────────▲───────────────────────────────────────┘
                                               │ set / step / grabBackground / preview
┌────────── CamFunge GUI (Tauri shell + webview: src/) ─────────────────────────────────┐
│ starts the service (or attaches to a running one), sends config, and displays the     │
│ virtual camera device itself via getUserMedia — falling back to the direct preview.   │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

| Path | What |
|---|---|
| `service/src/filters.rs` | the filter shaders (GLSL bodies, unchanged from the WebGL original) |
| `service/src/shaders.rs` | GLSL 4.50 prelude that runs them on every wgpu backend |
| `service/src/renderer.rs` | GPU passes: motion, 64-frame history, filter feedback, BGRA readback |
| `service/src/engine.rs` | per-frame behaviour (pure, unit-tested): reactivity, gestures, auto-cycle |
| `service/src/vcam/` | virtual camera backends per OS + BGRA→YUYV/NV12 |
| `service/src/server.rs`, `protocol.rs` | WebSocket control protocol |
| `src/` | GUI: `client.js` (protocol client, DOM-free), `app.js`, `index.html` |
| `src-tauri/` | GUI shell: spawns/attaches to the service, ties its lifetime to the GUI's |

Everything is cross-platform; only the virtual camera needs a per-OS driver.

## Virtual camera setup

| OS | Backend | One-time setup | Device name in other apps |
|---|---|---|---|
| macOS 13+ | `obs-macos` | Install [OBS Studio](https://obsproject.com) 30+, click **Start Virtual Camera** once and approve the system extension, then quit OBS. | OBS Virtual Camera |
| Windows 10/11 | `obs-windows` | Install OBS Studio 26+ (registers its virtual camera). | OBS Virtual Camera |
| Linux | `v4l2loopback` | `sudo modprobe v4l2loopback devices=1 card_label=CamFunge exclusive_caps=1` | CamFunge |

CamFunge feeds these existing drivers instead of shipping its own, which on macOS would mean a separately signed
and notarised system extension and on Windows a registered COM media source. OBS's own virtual camera must be
stopped while CamFunge uses it. Without a driver the service still runs; the GUI shows the direct preview and
explains why there is no virtual camera.

## Running

```sh
npm install
npm run dev                  # GUI (builds and starts the service for you)

npm run service -- --help    # headless only
npm run service -- --camera test-pattern --set filter=1 --set reactTo=sound
cargo run -p camfunge-service -- --list-cameras
```

Useful service flags: `--port` (0 = any, printed on stdout), `--width/--height/--fps` (default 1280×720@30),
`--virtual-camera auto|none|obs|v4l2loopback`, `--v4l2-device /dev/videoN`, `--allow-origin <origin>` for browser
clients other than the GUI, `--exit-with-stdin` (used by the GUI so the service never outlives it).

The GUI attaches to a service already listening on its port (`CAMFUNGE_PORT`, default 7272), so you can run the
service at login or on another account and use the GUI only to steer it.

### Protocol

Clients send JSON text frames: `{"type":"set","config":{"filter":3,"reactTo":"sound"}}`, `{"type":"step","by":-1}`,
`{"type":"grabBackground"}`, `{"type":"preview","enabled":true,"maxSize":640}`, `{"type":"refresh"}`.
The service answers with `hello` (filters, cameras, config, virtual camera, output, GPU) on connect, `config`
whenever anything changes it, `status` (level, fps, error) five times a second, and binary JPEG frames to
preview subscribers. Browser origins other than Tauri's are refused unless allowed with `--allow-origin`.

## Tests

```sh
npm test            # cargo test (service) + node --test (GUI client against a real service)
npm run check:gui   # the real GUI in headless Chrome driving a real service (direct preview)
npm run check:vcam  # same, through this machine's virtual camera driver
```

- **Rust unit tests**: reactivity analysis, engine behaviour (gestures, auto-cycle, frame-rate-independent
  smoothing), protocol (de)serialisation, every shader compiling under naga, YUV conversions, OBS shared-memory
  layout, v4l2 struct layout, camera selection, preview encoding.
- **Rust GPU tests** (`service/tests/render.rs`): every filter renders, motion is detected, the picture is upright
  and mirrors on request. They skip when no GPU or software adapter exists; set `CAMFUNGE_REQUIRE_GPU=1` in CI
  (on Linux, Mesa's lavapipe is enough).
- **Node tests** (`tests/*.test.mjs`): the GUI's protocol client against a real headless service — hello,
  broadcasts, clamping, errors, previews, reconnecting after a service restart, origin refusal.

## Notes

- Output runs at a fixed 30 fps (virtual cameras' usual rate) rather than the display rate; reactivity smoothing
  is time-based so it feels the same, but feedback trails (which decay per frame) last about twice as long as
  they did at 60 Hz.
- The macOS bundle keeps `NSCameraUsageDescription`/`NSMicrophoneUsageDescription`: the service it launches asks
  for camera and microphone access on the GUI's behalf.
