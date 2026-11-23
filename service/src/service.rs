//! The headless service: camera → engine → GPU → virtual camera + previews, steered over WebSocket.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::audio::Microphone;
use crate::camera::{self, CameraFeed};
use crate::engine::{Engine, PROBE_H, PROBE_W};
use crate::filters::FILTERS;
use crate::protocol::{CameraInfo, ClientMessage, Config, FilterInfo, OutputInfo, ReactTo, ServerMessage, VirtualCameraInfo};
use crate::reactivity::downsample;
use crate::renderer::Renderer;
use crate::server::{ClientId, Event, OriginPolicy, Server};
use crate::vcam::{self, VirtualCamera};

pub const DEFAULT_PORT: u16 = 7272;
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_PREVIEW_SIZE: u32 = 640;
const PREVIEW_QUALITY: u8 = 70;

pub struct Options {
    pub listen: SocketAddr,
    pub origins: OriginPolicy,
    pub size: [u32; 2],
    pub fps: u32,
    pub virtual_camera: vcam::Choice,
    pub config: Config,
}

/// Set to stop [`run`] cleanly (Ctrl-C, parent exit…).
pub type Shutdown = Arc<AtomicBool>;

struct State {
    engine: Engine,
    renderer: Renderer,
    vcam: Option<Box<dyn VirtualCamera>>,
    vcam_info: VirtualCameraInfo,
    vcam_error: Option<String>,
    cameras: Vec<CameraInfo>,
    feed: Option<CameraFeed>,
    last_seq: u64,
    mic: Option<Microphone>,
    /// Clients that asked for preview frames, with the longest edge they want.
    previews: Vec<(ClientId, u32)>,
    fps: u32,
}

impl State {
    fn camera_id(&self) -> String {
        self.engine.config().camera.clone().unwrap_or_else(|| camera::default_id(&self.cameras, self.vcam_info.device.as_deref()))
    }

    fn sync_camera(&mut self) {
        let want = self.camera_id();
        if self.feed.as_ref().is_none_or(|f| f.id != want) {
            self.feed = None; // release the old camera before opening the new one
            self.feed = Some(CameraFeed::open(&want));
            self.last_seq = 0;
            self.engine.camera_restarted();
        }
    }

    fn sync_microphone(&mut self) {
        match (self.engine.config().react_to, &self.mic) {
            (ReactTo::Sound, None) => self.mic = Some(Microphone::open()),
            (ReactTo::Sound, Some(_)) => {}
            (_, Some(_)) => self.mic = None,
            _ => {}
        }
    }

    fn hello(&self) -> ServerMessage {
        let ours = self.vcam_info.device.as_deref();
        ServerMessage::Hello {
            version: env!("CARGO_PKG_VERSION").into(),
            filters: FILTERS.iter().map(|f| FilterInfo { name: f.name.into(), artist: f.artist.into() }).collect(),
            // Our own output is never offered as an input.
            cameras: self.cameras.iter().filter(|c| ours.is_none_or(|o| !o.eq_ignore_ascii_case(&c.name))).cloned().collect(),
            config: self.engine.config().clone(),
            virtual_camera: self.vcam_info.clone(),
            output: OutputInfo { width: self.renderer.size()[0], height: self.renderer.size()[1], fps: self.fps },
            gpu: self.renderer.gpu.clone(),
        }
    }

    fn input_error(&self) -> Option<String> {
        let camera = self.feed.as_ref().and_then(|f| f.error());
        let mic = self.mic.as_ref().and_then(|m| m.error());
        camera.or(mic).or_else(|| self.vcam_error.clone())
    }

    fn handle(&mut self, server: &Server, event: Event, now: f64) {
        match event {
            Event::Connected(id) => server.send(id, &self.hello()),
            Event::Disconnected(id) => self.previews.retain(|(c, _)| *c != id),
            Event::Invalid(id, why) => server.send(id, &ServerMessage::Error { message: format!("bad message: {why}") }),
            Event::Message(id, msg) => match msg {
                ClientMessage::Set { config } => {
                    self.engine.apply(&config, now);
                    self.sync_camera();
                    self.sync_microphone();
                }
                ClientMessage::Step { by } => self.engine.step(by, now),
                ClientMessage::GrabBackground => self.engine.grab_background(),
                ClientMessage::Preview { enabled, max_size } => {
                    self.previews.retain(|(c, _)| *c != id);
                    if enabled {
                        self.previews.push((id, max_size.unwrap_or(DEFAULT_PREVIEW_SIZE).clamp(16, 4096)));
                    }
                }
                ClientMessage::Refresh => {
                    self.cameras = camera::list();
                    server.send(id, &self.hello());
                }
            },
        }
    }

    fn send_previews(&self, server: &Server, frame: &[u8]) {
        let [w, h] = self.renderer.size();
        let mut sizes: Vec<u32> = self.previews.iter().map(|(_, s)| *s).collect();
        sizes.sort_unstable();
        sizes.dedup();
        for max in sizes {
            let Some(jpeg) = preview_jpeg(frame, [w, h], max) else { continue };
            for (id, _) in self.previews.iter().filter(|(_, s)| *s == max) {
                server.send_preview(*id, jpeg.clone());
            }
        }
    }
}

/// Scales a BGRA frame so its longest edge is at most `max` and encodes it as JPEG.
pub fn preview_jpeg(bgra: &[u8], [w, h]: [u32; 2], max: u32) -> Option<Vec<u8>> {
    let scale = (max as f32 / w.max(h) as f32).min(1.0);
    let (pw, ph) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    let small;
    let pixels = if (pw, ph) == (w, h) {
        bgra
    } else {
        small = downsample(bgra, w as usize, h as usize, pw as usize, ph as usize);
        &small
    };
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, PREVIEW_QUALITY).encode(pixels, pw as u16, ph as u16, jpeg_encoder::ColorType::Bgra).ok()?;
    Some(out)
}

/// Runs until `shutdown` is set. `ready` is called once listening, with the bound address and initial hello.
pub fn run(opts: Options, shutdown: Shutdown, ready: impl FnOnce(SocketAddr, &ServerMessage)) -> Result<()> {
    let renderer = Renderer::new(opts.size)?;
    let (vcam, vcam_info) = vcam::open(opts.virtual_camera.clone(), opts.size, opts.fps);
    let server = Server::bind(opts.listen, opts.origins.clone())?;
    let mut state = State {
        engine: Engine::new(opts.config.clone(), FILTERS.len()),
        renderer,
        vcam,
        vcam_info,
        vcam_error: None,
        cameras: camera::list(),
        feed: None,
        last_seq: 0,
        mic: None,
        previews: Vec::new(),
        fps: opts.fps,
    };
    state.sync_camera();
    state.sync_microphone();
    ready(server.addr, &state.hello());

    let start = Instant::now();
    let frame_time = Duration::from_secs_f64(1.0 / opts.fps.max(1) as f64);
    let (mut last, mut next_frame) = (start, start);
    let (mut last_status, mut frames_since_status) = (start, 0u32);
    while !shutdown.load(Ordering::Relaxed) {
        let tick = Instant::now();
        let now = (tick - start).as_secs_f64();
        for event in server.poll() {
            state.handle(&server, event, now);
        }

        let size = state.renderer.size();
        let fresh = state.feed.as_ref().and_then(|f| f.latest()).filter(|f| f.seq != state.last_seq);
        let plan = fresh.as_ref().map(|f| {
            state.last_seq = f.seq;
            let probe = downsample(&f.rgba, f.size[0] as usize, f.size[1] as usize, PROBE_W, PROBE_H);
            state.engine.ingest(&probe, f.size, size, now)
        });
        let sound = state.mic.as_ref().map_or(0.0, |m| m.level());
        state.engine.tick(now, (tick - last).as_secs_f64(), sound);
        last = tick;

        let uniforms = state.engine.uniforms(size, now);
        let filter = state.engine.config().filter;
        let camera = fresh.as_ref().zip(plan).map(|(f, p)| (f.rgba.as_slice(), f.size, p));
        let frame = state.renderer.render(camera, filter, &uniforms)?.to_vec();
        if let Some(cam) = state.vcam.as_mut() {
            state.vcam_error = cam.send(&frame).err().map(|e| format!("virtual camera: {e:#}"));
        }
        if !state.previews.is_empty() {
            state.send_previews(&server, &frame);
        }
        if state.engine.take_config_changed() {
            server.broadcast(&ServerMessage::Config { config: state.engine.config().clone() });
        }
        frames_since_status += 1;
        if tick - last_status >= STATUS_INTERVAL && server.client_count() > 0 {
            let fps = frames_since_status as f32 / (tick - last_status).as_secs_f32();
            server.broadcast(&ServerMessage::Status { level: state.engine.level(), fps, error: state.input_error() });
            (last_status, frames_since_status) = (tick, 0);
        }

        next_frame += frame_time;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else if now - next_frame > frame_time * 4 {
            next_frame = now; // fell far behind (slow GPU, debugger): don't try to catch up in a burst
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_scaled_to_fit_and_decodes_as_jpeg() {
        let frame = [0u8, 0, 255, 255].repeat(1280 * 720);
        let jpeg = preview_jpeg(&frame, [1280, 720], 320).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8], "JPEG SOI marker");
        // SOF0 holds height then width.
        let sof = jpeg.windows(2).position(|w| w == [0xff, 0xc0]).expect("baseline SOF");
        let (h, w) = (u16::from_be_bytes([jpeg[sof + 5], jpeg[sof + 6]]), u16::from_be_bytes([jpeg[sof + 7], jpeg[sof + 8]]));
        assert_eq!((w, h), (320, 180));
    }

    #[test]
    fn small_frames_are_not_upscaled() {
        let frame = [0u8; 4].repeat(64 * 32);
        let jpeg = preview_jpeg(&frame, [64, 32], 640).unwrap();
        let sof = jpeg.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
        assert_eq!(u16::from_be_bytes([jpeg[sof + 7], jpeg[sof + 8]]), 64);
    }
}
