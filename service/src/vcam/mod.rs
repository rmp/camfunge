//! Virtual camera outputs. Each OS has its own driver model; CamFunge feeds an existing, widely installed
//! driver rather than shipping a signed kernel/system extension of its own:
//!
//! | OS      | backend        | driver                                                                   |
//! |---------|----------------|--------------------------------------------------------------------------|
//! | macOS   | `obs-macos`    | OBS Studio 30+ camera extension, fed through its CoreMediaIO sink stream |
//! | Windows | `obs-windows`  | OBS Studio 26+ DirectShow camera, fed through its shared-memory queue     |
//! | Linux   | `v4l2loopback` | the v4l2loopback kernel module, fed by writing YUYV frames                |

use anyhow::Result;
use std::path::PathBuf;

use crate::protocol::VirtualCameraInfo;

pub mod convert;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(any(windows, test))]
pub mod windows;

/// A running virtual camera.
pub trait VirtualCamera: Send {
    /// Sends one frame: tightly packed BGRA, top row first, at the size given to [`open`].
    fn send(&mut self, bgra: &[u8]) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// This platform's backend.
    Auto,
    None,
    Obs,
    V4l2Loopback { device: Option<PathBuf> },
}

impl std::str::FromStr for Choice {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Choice::Auto),
            "none" | "off" => Ok(Choice::None),
            "obs" => Ok(Choice::Obs),
            "v4l2loopback" => Ok(Choice::V4l2Loopback { device: None }),
            _ => Err(format!("unknown virtual camera '{s}' (expected auto, none, obs or v4l2loopback)")),
        }
    }
}

impl Choice {
    fn resolve(self) -> Choice {
        match self {
            Choice::Auto if cfg!(target_os = "linux") => Choice::V4l2Loopback { device: None },
            Choice::Auto if cfg!(any(target_os = "macos", windows)) => Choice::Obs,
            Choice::Auto => Choice::None,
            other => other,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.clone().resolve() {
            Choice::Obs if cfg!(target_os = "macos") => "obs-macos",
            Choice::Obs => "obs-windows",
            Choice::V4l2Loopback { .. } => "v4l2loopback",
            _ => "none",
        }
    }
}

/// Opens the chosen virtual camera at `size` and `fps`. Failure is not fatal: the service still renders
/// (and previews), and the returned info says why there is no virtual camera.
pub fn open(choice: Choice, size: [u32; 2], fps: u32) -> (Option<Box<dyn VirtualCamera>>, VirtualCameraInfo) {
    let backend = choice.backend_name().to_string();
    let opened: Result<(Box<dyn VirtualCamera>, String)> = match choice.resolve() {
        Choice::None | Choice::Auto => return (None, VirtualCameraInfo { backend, device: None, error: None }),
        #[cfg(target_os = "macos")]
        Choice::Obs => macos::ObsCamera::open(size).map(|(c, name)| (Box::new(c) as Box<dyn VirtualCamera>, name)),
        #[cfg(windows)]
        Choice::Obs => windows::ObsCamera::open(size, fps).map(|c| (Box::new(c) as Box<dyn VirtualCamera>, windows::DEVICE_NAME.to_string())),
        #[cfg(target_os = "linux")]
        Choice::V4l2Loopback { device } => linux::Loopback::open(device, size, fps).map(|(c, name)| (Box::new(c) as Box<dyn VirtualCamera>, name)),
        #[allow(unreachable_patterns)]
        other => Err(anyhow::anyhow!("the {other:?} virtual camera is not available on {}", std::env::consts::OS)),
    };
    let _ = (size, fps);
    match opened {
        Ok((cam, name)) => (Some(cam), VirtualCameraInfo { backend, device: Some(name), error: None }),
        Err(e) => (None, VirtualCameraInfo { backend, device: None, error: Some(format!("{e:#}")) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_parse() {
        assert_eq!("auto".parse(), Ok(Choice::Auto));
        assert_eq!("off".parse(), Ok(Choice::None));
        assert!("bogus".parse::<Choice>().is_err());
    }

    #[test]
    fn auto_picks_this_platforms_backend() {
        let expected = if cfg!(target_os = "macos") {
            "obs-macos"
        } else if cfg!(windows) {
            "obs-windows"
        } else if cfg!(target_os = "linux") {
            "v4l2loopback"
        } else {
            "none"
        };
        assert_eq!(Choice::Auto.backend_name(), expected);
        assert_eq!(Choice::None.backend_name(), "none");
    }

    #[test]
    fn none_opens_nothing_without_error() {
        let (cam, info) = open(Choice::None, [1280, 720], 30);
        assert!(cam.is_none());
        assert_eq!(info, VirtualCameraInfo { backend: "none".into(), device: None, error: None });
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn foreign_backend_reports_an_error() {
        let (cam, info) = open(Choice::V4l2Loopback { device: None }, [1280, 720], 30);
        assert!(cam.is_none());
        assert!(info.error.unwrap().contains("not available"));
    }
}
