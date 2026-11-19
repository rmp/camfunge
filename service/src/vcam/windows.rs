//! Windows: feeds OBS Studio's DirectShow virtual camera ("OBS Virtual Camera", installed with OBS 26+)
//! through the shared-memory queue it reads, `OBSVirtualCamVideo`. Layout and protocol follow
//! obs-studio `plugins/win-dshow/shared-memory-queue.c`: a header, then three NV12 frame slots each preceded
//! by a 32-byte frame header holding a u64 timestamp. OBS itself must not be running its virtual camera.
//!
//! The layout arithmetic below is platform-independent so it is unit-tested everywhere.

pub const DEVICE_NAME: &str = "OBS Virtual Camera";
pub const MAPPING_NAME: &str = "OBSVirtualCamVideo";

pub const STATE_STARTING: u32 = 1;
pub const STATE_READY: u32 = 2;
pub const STATE_STOPPING: u32 = 3;

/// `struct queue_header` field offsets (C layout: the u64 `interval` is 8-aligned).
pub mod header {
    pub const WRITE_IDX: usize = 0;
    pub const READ_IDX: usize = 4;
    pub const STATE: usize = 8;
    pub const OFFSETS: usize = 12;
    pub const TYPE: usize = 24;
    pub const CX: usize = 28;
    pub const CY: usize = 32;
    pub const INTERVAL: usize = 40;
    pub const SIZE: usize = 80;
}

const FRAME_HEADER_SIZE: u32 = 32;

fn align32(v: u32) -> u32 {
    (v + 31) & !31
}

/// Offsets of the three frame slots and the total mapping size for a `cx`×`cy` NV12 queue.
pub fn layout(cx: u32, cy: u32) -> ([u32; 3], u32) {
    let frame = cx * cy * 3 / 2;
    let mut size = align32(header::SIZE as u32);
    let mut offsets = [0; 3];
    for o in &mut offsets {
        *o = size;
        size = align32(size + frame + FRAME_HEADER_SIZE);
    }
    (offsets, size)
}

/// Frame interval in 100 ns units, as DirectShow expects.
pub fn interval(fps: u32) -> u64 {
    10_000_000 / fps.max(1) as u64
}

#[cfg(windows)]
pub use imp::ObsCamera;

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::vcam::convert::{bgra_to_nv12, nv12_len, BT709};
    use crate::vcam::VirtualCamera;
    use anyhow::{bail, Result};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, FILE_MAP_READ,
        MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
    };

    pub struct ObsCamera {
        handle: HANDLE,
        base: *mut u8,
        size: [u32; 2],
        offsets: [u32; 3],
        start: Instant,
        nv12: Vec<u8>,
    }

    // The mapping is only touched from the render thread that owns this value.
    unsafe impl Send for ObsCamera {}

    impl ObsCamera {
        pub fn open(size: [u32; 2], fps: u32) -> Result<Self> {
            let name: Vec<u16> = MAPPING_NAME.encode_utf16().chain([0]).collect();
            let (offsets, total) = layout(size[0], size[1]);
            unsafe {
                let existing = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
                if !existing.is_null() {
                    CloseHandle(existing);
                    bail!("the OBS virtual camera is already being fed (is OBS's own virtual camera running?)");
                }
                let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, total, name.as_ptr());
                if handle.is_null() {
                    bail!("could not create the OBS virtual camera queue: {}", std::io::Error::last_os_error());
                }
                let view: MEMORY_MAPPED_VIEW_ADDRESS = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
                if view.Value.is_null() {
                    CloseHandle(handle);
                    bail!("could not map the OBS virtual camera queue: {}", std::io::Error::last_os_error());
                }
                let base = view.Value as *mut u8;
                std::ptr::write_bytes(base, 0, header::SIZE);
                let put32 = |off: usize, v: u32| (base.add(off) as *mut u32).write_unaligned(v);
                put32(header::STATE, STATE_STARTING);
                for (i, o) in offsets.iter().enumerate() {
                    put32(header::OFFSETS + i * 4, *o);
                }
                put32(header::TYPE, 0);
                put32(header::CX, size[0]);
                put32(header::CY, size[1]);
                (base.add(header::INTERVAL) as *mut u64).write_unaligned(interval(fps));
                Ok(ObsCamera { handle, base, size, offsets, start: Instant::now(), nv12: vec![0; nv12_len(size[0] as usize, size[1] as usize)] })
            }
        }

        fn atomic(&self, off: usize) -> &AtomicU32 {
            unsafe { &*(self.base.add(off) as *const AtomicU32) }
        }
    }

    impl VirtualCamera for ObsCamera {
        fn send(&mut self, bgra: &[u8]) -> Result<()> {
            let [w, h] = self.size.map(|v| v as usize);
            bgra_to_nv12(bgra, w, h, BT709, &mut self.nv12);
            let inc = self.atomic(header::WRITE_IDX).fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            let slot = self.offsets[(inc % 3) as usize] as usize;
            unsafe {
                (self.base.add(slot) as *mut u64).write_unaligned(self.start.elapsed().as_nanos() as u64);
                std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), self.base.add(slot + FRAME_HEADER_SIZE as usize), self.nv12.len());
            }
            self.atomic(header::READ_IDX).store(inc, Ordering::Release);
            self.atomic(header::STATE).store(STATE_READY, Ordering::Release);
            Ok(())
        }
    }

    impl Drop for ObsCamera {
        fn drop(&mut self) {
            self.atomic(header::STATE).store(STATE_STOPPING, Ordering::Release);
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base as *mut _ });
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_obs_for_1080p() {
        // sizeof(queue_header) = 80 → first slot at 96; each slot = 32-byte header + 1920·1080·1.5.
        let (offsets, total) = layout(1920, 1080);
        assert_eq!(offsets, [96, 96 + 3110432, 96 + 2 * 3110432]);
        assert_eq!(total, 96 + 3 * 3110432);
        assert!(offsets.iter().all(|o| o % 32 == 0));
    }

    #[test]
    fn interval_is_in_100ns_units() {
        assert_eq!(interval(30), 333_333);
        assert_eq!(interval(0), 10_000_000);
    }
}
