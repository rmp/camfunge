//! Frame-to-frame behaviour that is not pixels: reactivity smoothing, auto-cycle, wave gestures, the motion
//! focus point. Pure and deterministic (time is passed in), so it is tested without a GPU or camera.

use crate::protocol::{Config, ConfigPatch, ReactTo};
use crate::reactivity::{analyse_frame, dominant_zone, Analysis, DEFAULT_ZONE_THRESHOLD};
use crate::shaders::HISTORY_FRAMES;

pub const AUTO_CYCLE_SECS: f64 = 15.0;
pub const GESTURE_COOLDOWN_SECS: f64 = 1.5;
/// Analysis probe size (downsampled camera frame).
pub const PROBE_W: usize = 64;
pub const PROBE_H: usize = 36;
/// The original tuned its per-frame smoothing constants at this display rate.
const TUNED_FPS: f64 = 60.0;

/// Video zones (row-major from the camera image's top-left) for the output's top corners.
fn corner_zones(mirror: bool) -> (usize, usize) {
    if mirror { (2, 0) } else { (0, 2) }
}

/// Converts a per-frame-at-60Hz smoothing factor to one for a frame of `dt` seconds.
fn per_dt(rate_per_frame: f64, dt: f64) -> f64 {
    1.0 - (1.0 - rate_per_frame).powf(dt * TUNED_FPS)
}

/// Scale that makes a `cam_w`×`cam_h` image cover an `out_w`×`out_h` frame, cropping the excess.
pub fn cover_scale(out_w: u32, out_h: u32, cam_w: u32, cam_h: u32) -> [f32; 2] {
    let out_aspect = out_w as f32 / out_h as f32;
    let cam_aspect = cam_w as f32 / cam_h as f32;
    if out_aspect > cam_aspect { [1.0, cam_aspect / out_aspect] } else { [out_aspect / cam_aspect, 1.0] }
}

/// What the renderer must do with a fresh camera frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IngestPlan {
    /// History tile to write this frame into.
    pub head: u32,
    /// First frame since (re)starting the camera: fill every history tile.
    pub fill_history: bool,
    pub grab_background: bool,
}

/// Per-draw shader inputs (see `shaders::PRELUDE`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uniforms {
    pub res: [f32; 2],
    pub cam_scale: [f32; 2],
    pub focus: [f32; 2],
    pub time: f32,
    pub amount: f32,
    pub clock: f32,
    pub burst: f32,
    pub head: f32,
    pub mirror: f32,
}

impl Uniforms {
    pub fn to_bytes(&self) -> [u8; crate::shaders::UNIFORM_BYTES as usize] {
        let floats = [
            self.res[0], self.res[1], self.cam_scale[0], self.cam_scale[1], self.focus[0], self.focus[1],
            self.time, self.amount, self.clock, self.burst, self.head, self.mirror,
        ];
        let mut out = [0u8; crate::shaders::UNIFORM_BYTES as usize];
        for (chunk, f) in out.chunks_exact_mut(4).zip(floats) {
            chunk.copy_from_slice(&f.to_le_bytes());
        }
        out
    }
}

pub struct Engine {
    config: Config,
    filter_count: usize,
    /// Set whenever `config` changes for any reason; cleared by [`Engine::take_config_changed`].
    config_changed: bool,
    time: f64,
    amount: f32,
    burst: f32,
    focus: [f32; 2],
    level: f32,
    move_level: f32,
    last_switch: f64,
    gesture_ready: f64,
    head: u32,
    prev_luma: Option<Vec<f32>>,
    analysis: Option<Analysis>,
    bg_wanted: bool,
    cam_scale: [f32; 2],
}

impl Engine {
    pub fn new(config: Config, filter_count: usize) -> Self {
        Engine {
            config,
            filter_count,
            config_changed: false,
            time: 0.0,
            amount: 0.0,
            burst: 0.0,
            focus: [0.5, 0.5],
            level: 0.0,
            move_level: 0.0,
            last_switch: 0.0,
            gesture_ready: 0.0,
            head: 0,
            prev_luma: None,
            analysis: None,
            bg_wanted: true,
            cam_scale: [1.0, 1.0],
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn take_config_changed(&mut self) -> bool {
        std::mem::take(&mut self.config_changed)
    }

    /// Applies a client's patch. Changing filter restarts the auto-cycle countdown.
    pub fn apply(&mut self, patch: &ConfigPatch, now: f64) {
        let before = self.config.clone();
        self.config.apply(patch, self.filter_count);
        if self.config.filter != before.filter {
            self.last_switch = now;
        }
        if self.config.camera != before.camera {
            self.camera_restarted();
        }
        self.config_changed |= self.config != before;
    }

    pub fn select_filter(&mut self, index: i64, now: f64) {
        let filter = index.rem_euclid(self.filter_count as i64) as usize;
        self.last_switch = now;
        if filter != self.config.filter {
            self.config.filter = filter;
            self.config_changed = true;
        }
    }

    pub fn step(&mut self, by: i64, now: f64) {
        self.select_filter(self.config.filter as i64 + by, now);
    }

    pub fn grab_background(&mut self) {
        self.bg_wanted = true;
    }

    /// Call when the camera (re)opens: history and background are refilled from its first frame.
    pub fn camera_restarted(&mut self) {
        self.prev_luma = None;
        self.analysis = None;
        self.bg_wanted = true;
    }

    /// A new camera frame arrived. `probe` is it downsampled to `PROBE_W`×`PROBE_H` RGBA.
    pub fn ingest(&mut self, probe: &[u8], cam_size: [u32; 2], out_size: [u32; 2], now: f64) -> IngestPlan {
        self.cam_scale = cover_scale(out_size[0], out_size[1], cam_size[0], cam_size[1]);
        let plan = IngestPlan {
            head: (self.head + 1) % HISTORY_FRAMES,
            fill_history: self.prev_luma.is_none(),
            grab_background: std::mem::take(&mut self.bg_wanted),
        };
        self.head = plan.head;

        let analysis = analyse_frame(probe, PROBE_W, self.prev_luma.as_deref());
        if analysis.movement - self.move_level > 0.3 {
            self.burst = 1.0;
        }
        self.move_level += (analysis.movement - self.move_level) * 0.2;
        let (target, pull) = match analysis.focus {
            Some(f) => (self.video_to_screen(f), 0.2),
            None => ([0.5, 0.5], 0.02),
        };
        for i in 0..2 {
            self.focus[i] += (target[i] - self.focus[i]) * pull;
        }

        if self.config.gestures && now > self.gesture_ready {
            let (left, right) = corner_zones(self.config.mirror);
            match dominant_zone(&analysis.zones, DEFAULT_ZONE_THRESHOLD) {
                Some(z) if z == left => {
                    self.step(-1, now);
                    self.gesture_ready = now + GESTURE_COOLDOWN_SECS;
                }
                Some(z) if z == right => {
                    self.step(1, now);
                    self.gesture_ready = now + GESTURE_COOLDOWN_SECS;
                }
                _ => {}
            }
        }
        self.prev_luma = Some(analysis.luma.clone());
        self.analysis = Some(analysis);
        plan
    }

    /// Analysis focus (video coords, y down) → output uv (y up), undoing cover scaling and mirroring.
    fn video_to_screen(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let sx = if self.config.mirror { 0.5 - x } else { x - 0.5 };
        [sx / self.cam_scale[0] + 0.5, (0.5 - y) / self.cam_scale[1] + 0.5]
    }

    /// Advances one output frame of `dt` seconds. `sound` is the latest 0..1 microphone level.
    pub fn tick(&mut self, now: f64, dt: f64, sound: f32) {
        if self.config.auto_cycle && now - self.last_switch > AUTO_CYCLE_SECS {
            self.step(1, now);
        }
        let signal = match self.config.react_to {
            ReactTo::Sound => sound,
            ReactTo::Movement => self.analysis.as_ref().map_or(0.0, |a| a.movement),
            ReactTo::Colour => self.analysis.as_ref().map_or(0.0, |a| a.colour),
        };
        let rate = if signal > self.level { 0.5 } else { 0.05 };
        self.level += (signal - self.level) * per_dt(rate, dt) as f32;
        let boost = self.config.reactivity * self.level;
        self.amount = (self.config.intensity + boost).min(1.0);
        self.time += dt * self.config.speed as f64 * (1.0 + 2.0 * boost as f64);
        self.burst *= (0.93f64).powf(dt * TUNED_FPS) as f32;
    }

    pub fn uniforms(&self, out_size: [u32; 2], clock: f64) -> Uniforms {
        Uniforms {
            res: [out_size[0] as f32, out_size[1] as f32],
            cam_scale: self.cam_scale,
            focus: self.focus,
            // Wrapped to keep f32 precision in long sessions; filters only use it in periodic functions.
            time: (self.time % 3600.0) as f32,
            amount: self.amount,
            clock: (clock % 3600.0) as f32,
            burst: self.burst,
            head: self.head as f32,
            mirror: if self.config.mirror { 1.0 } else { 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: [u32; 2] = [1280, 720];
    const DT: f64 = 1.0 / 30.0;

    fn engine() -> Engine {
        Engine::new(Config::default(), 5)
    }

    fn probe(fill: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut p = vec![255u8; PROBE_W * PROBE_H * 4];
        for y in 0..PROBE_H {
            for x in 0..PROBE_W {
                p[(y * PROBE_W + x) * 4..][..3].fill(fill(x, y));
            }
        }
        p
    }

    /// A bright block in video zone `zone` (row-major 3×3 from the top-left) on black.
    fn block_in_zone(zone: usize) -> Vec<u8> {
        let (zx, zy) = (zone % 3, zone / 3);
        probe(|x, y| if 3 * x / PROBE_W == zx && 3 * y / PROBE_H == zy { 255 } else { 0 })
    }

    #[test]
    fn cover_scale_crops_the_longer_axis() {
        assert_eq!(cover_scale(1280, 720, 1280, 720), [1.0, 1.0]);
        let [x, y] = cover_scale(1280, 720, 640, 480);
        assert_eq!(x, 1.0);
        assert!((y - 0.75).abs() < 1e-6);
        let [x, y] = cover_scale(720, 720, 1280, 720);
        assert!((x - 0.5625).abs() < 1e-6);
        assert_eq!(y, 1.0);
    }

    #[test]
    fn first_frame_fills_history_and_grabs_background_once() {
        let mut e = engine();
        let black = probe(|_, _| 0);
        let p1 = e.ingest(&black, OUT, OUT, 0.0);
        assert_eq!(p1, IngestPlan { head: 1, fill_history: true, grab_background: true });
        let p2 = e.ingest(&black, OUT, OUT, 0.1);
        assert_eq!(p2, IngestPlan { head: 2, fill_history: false, grab_background: false });
        e.grab_background();
        assert!(e.ingest(&black, OUT, OUT, 0.2).grab_background);
        e.camera_restarted();
        let p = e.ingest(&black, OUT, OUT, 0.3);
        assert!(p.fill_history && p.grab_background);
    }

    #[test]
    fn history_head_wraps() {
        let mut e = engine();
        let black = probe(|_, _| 0);
        let heads: Vec<u32> = (0..HISTORY_FRAMES + 1).map(|i| e.ingest(&black, OUT, OUT, i as f64).head).collect();
        assert_eq!(heads[HISTORY_FRAMES as usize - 2], HISTORY_FRAMES - 1);
        assert_eq!(heads[HISTORY_FRAMES as usize - 1], 0);
    }

    #[test]
    fn movement_raises_level_amount_and_time_rate() {
        let mut e = engine();
        e.ingest(&probe(|_, _| 0), OUT, OUT, 0.0);
        e.tick(0.0, DT, 0.0);
        let still = e.uniforms(OUT, 0.0);
        assert_eq!(e.level(), 0.0);
        assert!((still.amount - 0.6).abs() < 1e-6);

        e.ingest(&probe(|_, _| 255), OUT, OUT, DT);
        let before = e.time;
        e.tick(DT, DT, 0.0);
        assert!(e.level() > 0.5, "level {}", e.level());
        assert!(e.uniforms(OUT, 0.0).amount > 0.85);
        assert!(e.time - before > DT, "speed boosted by reaction");
        assert!(e.uniforms(OUT, 0.0).burst > 0.8, "sudden movement bursts (one 30fps tick of decay: 0.93²)");
    }

    #[test]
    fn smoothing_is_frame_rate_independent() {
        let run = |fps: f64| {
            let mut e = engine();
            e.config.react_to = ReactTo::Sound;
            for i in 0..(fps as usize / 2) {
                e.tick(i as f64 / fps, 1.0 / fps, 1.0);
            }
            e.level()
        };
        assert!((run(30.0) - run(60.0)).abs() < 1e-3);
    }

    #[test]
    fn sound_drives_level_only_when_selected() {
        let mut e = engine();
        e.tick(0.0, DT, 1.0);
        assert_eq!(e.level(), 0.0);
        e.apply(&ConfigPatch { react_to: Some(ReactTo::Sound), ..Default::default() }, 0.0);
        e.tick(DT, DT, 1.0);
        assert!(e.level() > 0.5);
    }

    #[test]
    fn auto_cycle_advances_after_15s_and_reports_change() {
        let mut e = engine();
        e.apply(&ConfigPatch { auto_cycle: Some(true), ..Default::default() }, 0.0);
        assert!(e.take_config_changed());
        e.tick(14.9, DT, 0.0);
        assert_eq!(e.config().filter, 0);
        assert!(!e.take_config_changed());
        e.tick(15.1, DT, 0.0);
        assert_eq!(e.config().filter, 1);
        assert!(e.take_config_changed());
        e.tick(20.0, DT, 0.0);
        assert_eq!(e.config().filter, 1, "countdown restarted");
    }

    #[test]
    fn manual_filter_change_restarts_auto_cycle() {
        let mut e = engine();
        e.apply(&ConfigPatch { auto_cycle: Some(true), ..Default::default() }, 0.0);
        e.apply(&ConfigPatch { filter: Some(3), ..Default::default() }, 10.0);
        e.tick(16.0, DT, 0.0);
        assert_eq!(e.config().filter, 3);
    }

    #[test]
    fn step_wraps_both_ways() {
        let mut e = engine();
        e.step(-1, 0.0);
        assert_eq!(e.config().filter, 4);
        e.step(2, 0.0);
        assert_eq!(e.config().filter, 1);
    }

    fn wave(mirror: bool, zone: usize) -> usize {
        let mut e = engine();
        e.apply(&ConfigPatch { gestures: Some(true), mirror: Some(mirror), filter: Some(2), ..Default::default() }, 0.0);
        e.ingest(&probe(|_, _| 0), OUT, OUT, 0.0);
        e.ingest(&block_in_zone(zone), OUT, OUT, 0.1);
        e.config().filter
    }

    #[test]
    fn waving_in_the_output_corners_switches_filter() {
        // Mirrored: the output's top-left is the camera image's top-right (zone 2).
        assert_eq!(wave(true, 2), 1);
        assert_eq!(wave(true, 0), 3);
        assert_eq!(wave(false, 0), 1);
        assert_eq!(wave(false, 2), 3);
        assert_eq!(wave(true, 4), 2, "centre does nothing");
    }

    #[test]
    fn gestures_are_off_by_default_and_rate_limited() {
        assert_eq!(engine().config().gestures, false);
        let mut e = engine();
        e.apply(&ConfigPatch { gestures: Some(true), ..Default::default() }, 0.0);
        let (black, wave) = (probe(|_, _| 0), block_in_zone(0));
        e.ingest(&black, OUT, OUT, 0.0);
        e.ingest(&wave, OUT, OUT, 0.1);
        e.ingest(&black, OUT, OUT, 0.2);
        e.ingest(&wave, OUT, OUT, 0.3);
        assert_eq!(e.config().filter, 1, "second wave inside cooldown ignored");
        e.ingest(&black, OUT, OUT, 2.0);
        e.ingest(&wave, OUT, OUT, 2.1);
        assert_eq!(e.config().filter, 2);
    }

    #[test]
    fn focus_follows_motion_into_output_coordinates() {
        let mut e = engine();
        // Motion in the camera's bottom-left → mirrored output's bottom-right (uv y up → small y).
        e.ingest(&probe(|_, _| 0), OUT, OUT, 0.0);
        for i in 0..40 {
            let lit = i % 2 == 0;
            e.ingest(&probe(|x, y| if lit && x < 8 && y > 28 { 255 } else { 0 }), OUT, OUT, i as f64);
        }
        let [fx, fy] = e.uniforms(OUT, 0.0).focus;
        assert!(fx > 0.8 && fy < 0.2, "focus {fx},{fy}");
    }

    #[test]
    fn uniform_bytes_follow_std140_order() {
        let u = Uniforms { res: [1.0, 2.0], cam_scale: [3.0, 4.0], focus: [5.0, 6.0], time: 7.0, amount: 8.0, clock: 9.0, burst: 10.0, head: 11.0, mirror: 12.0 };
        let b = u.to_bytes();
        let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((0..12).map(f).collect::<Vec<_>>(), (1..=12).map(|v| v as f32).collect::<Vec<_>>());
    }
}
