//! GPU smoke tests: every filter renders without a validation error, motion is detected, and the picture
//! comes out the right way up. Skipped when the machine has no GPU (not even a software one) unless
//! `CAMFUNGE_REQUIRE_GPU=1`.

use camfunge::engine::{Engine, PROBE_H, PROBE_W};
use camfunge::filters::FILTERS;
use camfunge::protocol::{Config, ConfigPatch};
use camfunge::reactivity::downsample;
use camfunge::renderer::Renderer;
use std::sync::{Mutex, MutexGuard};

const OUT: [u32; 2] = [320, 180];
const CAM: [u32; 2] = [320, 240];

/// Opening several GPU devices at once crashes some drivers (Mesa lavapipe segfaults ~50% of runs), and
/// the service only ever opens one, so the GPU tests take turns.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_turn() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn renderer() -> Option<Renderer> {
    match Renderer::new(OUT) {
        Ok(r) => Some(r),
        Err(e) if std::env::var_os("CAMFUNGE_REQUIRE_GPU").is_none() => {
            eprintln!("skipping GPU test: {e:#}");
            None
        }
        Err(e) => panic!("{e:#}"),
    }
}

/// Dark blue frame with a peach square whose x position is `x`.
fn scene(x: u32) -> Vec<u8> {
    let mut f = Vec::with_capacity((CAM[0] * CAM[1] * 4) as usize);
    for py in 0..CAM[1] {
        for px in 0..CAM[0] {
            let inside = (x..x + 80).contains(&px) && (80..160).contains(&py);
            f.extend(if inside { [0xff, 0xcc, 0x99, 0xff] } else { [0x22, 0x33, 0x44, 0xff] });
        }
    }
    f
}

fn feed(r: &mut Renderer, e: &mut Engine, frame: &[u8], t: f64) -> Vec<u8> {
    let probe = downsample(frame, CAM[0] as usize, CAM[1] as usize, PROBE_W, PROBE_H);
    let plan = e.ingest(&probe, CAM, OUT, t);
    e.tick(t, 1.0 / 30.0, 0.0);
    r.render(Some((frame, CAM, plan)), e.config().filter, &e.uniforms(OUT, t)).unwrap().to_vec()
}

#[test]
fn every_filter_renders_and_motion_is_detected() {
    let _gpu = gpu_turn();
    let Some(mut r) = renderer() else { return };
    let mut e = Engine::new(Config::default(), FILTERS.len());
    for (i, f) in FILTERS.iter().enumerate() {
        e.select_filter(i as i64, 0.0);
        for step in 0..3 {
            let t = (i * 3 + step) as f64 / 30.0;
            let out = feed(&mut r, &mut e, &scene(40 + 60 * (step as u32 % 2)), t);
            assert_eq!(out.len(), (OUT[0] * OUT[1] * 4) as usize, "{}", f.name);
        }
        let again = r.render(None, i, &e.uniforms(OUT, 1.0)).unwrap();
        assert!(again.chunks_exact(4).all(|p| p[3] == 255), "{}: output must be opaque", f.name);
    }
    let heat = r.motion_heat_max().unwrap();
    assert!(heat > 0.5, "motion pass saw no movement (max {heat})");
}

/// Mean BGRA of a 20×20 patch at the given output corner.
fn corner(out: &[u8], right: bool, bottom: bool) -> [f32; 3] {
    let (x0, y0) = (if right { OUT[0] - 20 } else { 0 }, if bottom { OUT[1] - 20 } else { 0 });
    let mut sum = [0.0; 3];
    for y in y0..y0 + 20 {
        for x in x0..x0 + 20 {
            let p = ((y * OUT[0] + x) * 4) as usize;
            for c in 0..3 {
                sum[c] += out[p + c] as f32 / 400.0;
            }
        }
    }
    sum
}

fn is_red([b, g, r]: [f32; 3]) -> bool {
    r > 200.0 && g < 50.0 && b < 50.0
}

#[test]
fn output_is_upright_and_mirrors_on_request() {
    let _gpu = gpu_turn();
    let Some(mut r) = renderer() else { return };
    // Red in the camera's top-left corner (camera rows run top to bottom), black elsewhere.
    let mut frame = vec![0u8; (CAM[0] * CAM[1] * 4) as usize];
    for y in 0..CAM[1] {
        for x in 0..CAM[0] {
            let p = ((y * CAM[0] + x) * 4) as usize;
            frame[p..p + 4].copy_from_slice(if x < 100 && y < 100 { &[255, 0, 0, 255] } else { &[0, 0, 0, 255] });
        }
    }
    for mirror in [false, true] {
        let mut e = Engine::new(Config::default(), FILTERS.len());
        e.apply(&ConfigPatch { mirror: Some(mirror), ..Default::default() }, 0.0);
        let out = feed(&mut r, &mut e, &frame, 0.0);
        assert!(is_red(corner(&out, mirror, false)), "mirror={mirror}: expected red top-{}", if mirror { "right" } else { "left" });
        assert!(!is_red(corner(&out, !mirror, false)), "mirror={mirror}: wrong corner red");
        assert!(!is_red(corner(&out, mirror, true)), "mirror={mirror}: picture is upside down");
    }
}

#[test]
fn renders_before_any_camera_frame() {
    let _gpu = gpu_turn();
    let Some(mut r) = renderer() else { return };
    let e = Engine::new(Config::default(), FILTERS.len());
    let out = r.render(None, 0, &e.uniforms(OUT, 0.0)).unwrap();
    assert!(out.chunks_exact(4).all(|p| p == [0, 0, 0, 255]), "no camera yet: opaque black");
}

#[test]
fn rejects_unusable_output_sizes() {
    let _gpu = gpu_turn();
    for size in [[0, 720], [1281, 720], [1280, 0]] {
        if let Err(e) = Renderer::new(size) {
            assert!(e.to_string().contains("output size") || e.to_string().contains("adapter"), "{e}");
        } else {
            panic!("{size:?} accepted");
        }
    }
}
