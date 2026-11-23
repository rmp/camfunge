//! Camera input on a background thread (nokhwa: AVFoundation / Media Foundation / V4L2), keeping only the
//! newest frame. Also a synthetic `test-pattern` camera for headless machines and tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::protocol::CameraInfo;

pub const TEST_PATTERN: &str = "test-pattern";

/// One RGBA camera frame, top row first.
pub struct Frame {
    pub rgba: Vec<u8>,
    pub size: [u32; 2],
    /// Increments per frame from this feed; lets the render loop spot fresh frames.
    pub seq: u64,
}

#[derive(Default)]
struct Shared {
    latest: Mutex<Option<Arc<Frame>>>,
    error: Mutex<Option<String>>,
    stop: AtomicBool,
}

/// A running camera. Dropping it stops the capture thread.
pub struct CameraFeed {
    pub id: String,
    shared: Arc<Shared>,
}

impl CameraFeed {
    pub fn open(id: &str) -> Self {
        let shared = Arc::new(Shared::default());
        let (thread_shared, thread_id) = (shared.clone(), id.to_string());
        let spawned = std::thread::Builder::new().name(format!("camera {id}")).spawn(move || {
            let result = if thread_id == TEST_PATTERN { test_pattern(&thread_shared) } else { capture(&thread_id, &thread_shared) };
            if let Err(e) = result {
                *thread_shared.error.lock().unwrap() = Some(e);
            }
        });
        if let Err(e) = spawned {
            *shared.error.lock().unwrap() = Some(format!("could not start camera thread: {e}"));
        }
        CameraFeed { id: id.to_string(), shared }
    }

    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.shared.latest.lock().unwrap().clone()
    }

    /// Why the camera is not delivering frames, if it failed.
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }
}

impl Drop for CameraFeed {
    fn drop(&mut self) {
        // Not joined: a stuck driver must not hang the render loop. The thread exits after its next frame.
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn publish(shared: &Shared, rgba: Vec<u8>, size: [u32; 2], seq: u64) {
    *shared.latest.lock().unwrap() = Some(Arc::new(Frame { rgba, size, seq }));
}

/// Real cameras, then the test pattern. Errors (e.g. no camera subsystem) yield just the test pattern.
pub fn list() -> Vec<CameraInfo> {
    let mut cams: Vec<CameraInfo> = nokhwa::query(nokhwa::utils::ApiBackend::Auto)
        .unwrap_or_default()
        .into_iter()
        .map(|c| CameraInfo { id: c.index().as_string(), name: c.human_name() })
        .collect();
    cams.push(CameraInfo { id: TEST_PATTERN.into(), name: "Test pattern".into() });
    cams
}

/// The camera to use when none is configured: the first one that is not a virtual camera (feeding our own
/// output back in would be a hall of mirrors), else the test pattern.
pub fn default_id(cams: &[CameraInfo], virtual_camera: Option<&str>) -> String {
    cams.iter()
        .find(|c| c.id != TEST_PATTERN && !is_virtual(&c.name, virtual_camera))
        .map_or_else(|| TEST_PATTERN.to_string(), |c| c.id.clone())
}

/// Whether a camera name looks like our (or any well-known) virtual camera.
pub fn is_virtual(name: &str, ours: Option<&str>) -> bool {
    let lower = name.to_lowercase();
    ours.is_some_and(|o| o.eq_ignore_ascii_case(name)) || lower.contains("virtual") || lower.contains("camfunge")
}

/// Asks the OS for camera access where that is a separate step (macOS). Blocks until answered.
pub fn request_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if nokhwa::nokhwa_check() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = tx.send(granted);
        });
        return match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("camera access denied — allow it in System Settings → Privacy & Security → Camera".into()),
            Err(_) => Err("timed out waiting for camera permission".into()),
        };
    }
    #[allow(unreachable_code)]
    Ok(())
}

fn capture(id: &str, shared: &Shared) -> Result<(), String> {
    use nokhwa::pixel_format::RgbAFormat;
    use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution};

    request_permission()?;
    let index = id.parse::<u32>().map_or_else(|_| CameraIndex::String(id.to_string()), CameraIndex::Index);
    let wanted = |fourcc| RequestedFormatType::Closest(CameraFormat::new(Resolution::new(1280, 720), fourcc, 30));
    let mut camera = [wanted(FrameFormat::MJPEG), wanted(FrameFormat::YUYV), wanted(FrameFormat::NV12), RequestedFormatType::AbsoluteHighestFrameRate]
        .into_iter()
        .find_map(|f| nokhwa::Camera::new(index.clone(), RequestedFormat::new::<RgbAFormat>(f)).ok())
        .ok_or_else(|| format!("could not open camera {id}"))?;
    camera.open_stream().map_err(|e| format!("could not start camera {id}: {e}"))?;
    let mut seq = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        let buffer = camera.frame().map_err(|e| format!("camera {id} stopped: {e}"))?;
        let res = buffer.resolution();
        match buffer.decode_image::<RgbAFormat>() {
            Ok(img) => {
                seq += 1;
                publish(shared, img.into_raw(), [res.width(), res.height()], seq);
            }
            // A corrupt MJPEG frame now and then is normal; skip it.
            Err(e) => eprintln!("camera {id}: dropped undecodable frame: {e}"),
        }
    }
    let _ = camera.stop_stream();
    Ok(())
}

pub const TEST_PATTERN_SIZE: [u32; 2] = [640, 480];

/// Draws test-pattern frame `n`: a peach square sweeping over a slowly hue-cycling backdrop.
pub fn test_pattern_frame(n: u64) -> Vec<u8> {
    let [w, h] = TEST_PATTERN_SIZE;
    let t = n as f32 / 30.0;
    let bx = (w as f32 / 2.0 - 80.0 + 200.0 * (t * 2.0).sin()) as i64;
    let tint = [(t * 0.7).sin(), (t * 0.7 + 2.1).sin(), (t * 0.7 + 4.2).sin()].map(|v| (40.0 + 30.0 * v) as u8);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let inside = (bx..bx + 160).contains(&x) && (160..320).contains(&y);
            let stripe = ((x + y) / 40) % 2 == 0;
            out.extend(if inside {
                [0xff, 0xcc, 0x99, 0xff]
            } else if stripe {
                [tint[0], tint[1], tint[2].saturating_add(40), 0xff]
            } else {
                [tint[0] / 2, tint[1] / 2, tint[2], 0xff]
            });
        }
    }
    out
}

fn test_pattern(shared: &Shared) -> Result<(), String> {
    let start = Instant::now();
    let mut n = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        publish(shared, test_pattern_frame(n), TEST_PATTERN_SIZE, n + 1);
        n += 1;
        let next = start + Duration::from_secs_f64(n as f64 / 30.0);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(id: &str, name: &str) -> CameraInfo {
        CameraInfo { id: id.into(), name: name.into() }
    }

    #[test]
    fn default_camera_skips_virtual_cameras() {
        let cams = [cam("0", "OBS Virtual Camera"), cam("1", "CamFunge"), cam("2", "FaceTime HD Camera"), cam(TEST_PATTERN, "Test pattern")];
        assert_eq!(default_id(&cams, Some("OBS Virtual Camera")), "2");
        assert_eq!(default_id(&cams[..2], None), TEST_PATTERN);
        assert_eq!(default_id(&[], None), TEST_PATTERN);
    }

    #[test]
    fn our_own_virtual_camera_is_recognised_by_name() {
        assert!(is_virtual("Dummy video device (0x0000)", Some("Dummy video device (0x0000)")));
        assert!(!is_virtual("Integrated Webcam", Some("Dummy video device (0x0000)")));
    }

    #[test]
    fn test_pattern_moves() {
        let [w, h] = TEST_PATTERN_SIZE;
        let (a, b) = (test_pattern_frame(0), test_pattern_frame(10));
        assert_eq!(a.len(), (w * h * 4) as usize);
        assert_ne!(a, b);
    }

    #[test]
    fn test_pattern_feed_delivers_frames() {
        let feed = CameraFeed::open(TEST_PATTERN);
        let deadline = Instant::now() + Duration::from_secs(5);
        while feed.latest().is_none_or(|f| f.seq < 2) {
            assert!(Instant::now() < deadline, "no frames");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(feed.latest().unwrap().size, TEST_PATTERN_SIZE);
        assert_eq!(feed.error(), None);
    }
}
