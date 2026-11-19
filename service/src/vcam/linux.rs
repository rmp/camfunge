//! Linux: writes YUYV frames to a v4l2loopback device. Load the module first, e.g.
//! `sudo modprobe v4l2loopback devices=1 card_label=CamFunge exclusive_caps=1`.

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use super::convert::{bgra_to_yuyv, BT601};
use super::VirtualCamera;

const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_FIELD_NONE: u32 = 1;
const V4L2_COLORSPACE_SRGB: u32 = 8;
const V4L2_YCBCR_ENC_601: u32 = 1;
const V4L2_QUANTIZATION_LIM_RANGE: u32 = 2;
const V4L2_PIX_FMT_YUYV: u32 = u32::from_le_bytes(*b"YUYV");

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

#[repr(C)]
union FormatUnion {
    pix: PixFormat,
    raw: [u8; 200],
    // The kernel union contains pointers, so it is pointer-aligned.
    _align: *mut libc::c_void,
}

#[repr(C)]
struct Format {
    type_: u32,
    fmt: FormatUnion,
}

#[repr(C)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | nr
}
const IOC_READ: u64 = 2;
const IOC_WRITE: u64 = 1;
const VIDIOC_QUERYCAP: u64 = ioc(IOC_READ, 0, std::mem::size_of::<Capability>());
const VIDIOC_S_FMT: u64 = ioc(IOC_READ | IOC_WRITE, 5, std::mem::size_of::<Format>());

fn c_str(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|&b| b == 0).next().unwrap_or_default()).into_owned()
}

fn query(file: &File) -> Option<Capability> {
    let mut cap: Capability = unsafe { std::mem::zeroed() };
    (unsafe { libc::ioctl(file.as_raw_fd(), VIDIOC_QUERYCAP as _, &mut cap) } == 0).then_some(cap)
}

/// The first `/dev/videoN` driven by v4l2loopback.
fn find_loopback() -> Option<PathBuf> {
    let mut devices: Vec<PathBuf> = std::fs::read_dir("/sys/class/video4linux")
        .ok()?
        .flatten()
        .map(|e| Path::new("/dev").join(e.file_name()))
        .collect();
    devices.sort_by_key(|p| p.to_string_lossy().trim_start_matches("/dev/video").parse::<u32>().unwrap_or(u32::MAX));
    devices.into_iter().find(|p| {
        OpenOptions::new().read(true).open(p).ok().and_then(|f| query(&f)).is_some_and(|c| c_str(&c.driver) == "v4l2 loopback")
    })
}

pub struct Loopback {
    file: File,
    width: usize,
    yuyv: Vec<u8>,
}

impl Loopback {
    /// Opens `device` (or the first loopback device) and returns it with the name other apps will show.
    pub fn open(device: Option<PathBuf>, size: [u32; 2], _fps: u32) -> Result<(Self, String)> {
        let path = match device {
            Some(p) => p,
            None => find_loopback().context(
                "no v4l2loopback device found — run: sudo modprobe v4l2loopback devices=1 card_label=CamFunge exclusive_caps=1",
            )?,
        };
        let file = OpenOptions::new().read(true).write(true).open(&path).with_context(|| format!("could not open {}", path.display()))?;
        let name = query(&file).map(|c| c_str(&c.card)).filter(|n| !n.is_empty()).unwrap_or_else(|| path.display().to_string());

        let [w, h] = size;
        let mut format = Format {
            type_: V4L2_BUF_TYPE_VIDEO_OUTPUT,
            fmt: FormatUnion { raw: [0; 200] },
        };
        format.fmt.pix = PixFormat {
            width: w,
            height: h,
            pixelformat: V4L2_PIX_FMT_YUYV,
            field: V4L2_FIELD_NONE,
            bytesperline: w * 2,
            sizeimage: w * h * 2,
            colorspace: V4L2_COLORSPACE_SRGB,
            ycbcr_enc: V4L2_YCBCR_ENC_601,
            quantization: V4L2_QUANTIZATION_LIM_RANGE,
            ..Default::default()
        };
        if unsafe { libc::ioctl(file.as_raw_fd(), VIDIOC_S_FMT as _, &mut format) } != 0 {
            bail!("{} rejected {w}x{h} YUYV output: {}", path.display(), std::io::Error::last_os_error());
        }
        Ok((Loopback { file, width: w as usize, yuyv: Vec::new() }, name))
    }
}

impl VirtualCamera for Loopback {
    fn send(&mut self, bgra: &[u8]) -> Result<()> {
        bgra_to_yuyv(bgra, self.width, BT601, &mut self.yuyv);
        self.file.write_all(&self.yuyv).context("writing to v4l2loopback")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_struct_sizes() {
        assert_eq!(std::mem::size_of::<PixFormat>(), 48);
        assert_eq!(std::mem::size_of::<Capability>(), 104);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(std::mem::size_of::<Format>(), 208);
            assert_eq!(VIDIOC_S_FMT, 0xC0D0_5605);
            assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        }
    }

    #[test]
    fn explicit_missing_device_is_an_error() {
        let err = Loopback::open(Some("/dev/does-not-exist".into()), [64, 64], 30).err().unwrap();
        assert!(format!("{err:#}").contains("/dev/does-not-exist"));
    }
}
