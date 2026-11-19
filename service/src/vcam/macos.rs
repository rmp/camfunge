//! macOS 13+: feeds OBS Studio's camera system extension ("OBS Virtual Camera") through its CoreMediaIO
//! *sink* stream, exactly as OBS does (obs-studio `plugins/mac-virtualcam/src/obs-plugin/plugin-main.mm`).
//! Requires OBS 30+ installed with its virtual camera started once (approving the system extension);
//! OBS itself must not be running its virtual camera at the same time.

#![allow(non_upper_case_globals, non_snake_case)]

use anyhow::{anyhow, bail, Result};
use std::ffi::c_void;
use std::ptr::{null, null_mut};

use super::VirtualCamera;

/// `OBSCameraDeviceUUID` from OBS's mac-virtualcam Info.plist.
const OBS_DEVICE_UID: &str = "7626645E-4425-469E-9D8B-97E0FA59AC75";

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CMIOObjectID = u32;

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const kCMIOObjectSystemObject: CMIOObjectID = 1;
const kCMIOObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
const kCMIOObjectPropertyElementMain: u32 = 0;
const kCMIOHardwarePropertyDevices: u32 = fourcc(b"dev#");
const kCMIOHardwarePropertyRunLoop: u32 = fourcc(b"rnlp");
const kCMIODevicePropertyDeviceUID: u32 = fourcc(b"uid ");
const kCMIODevicePropertyStreams: u32 = fourcc(b"stm#");
const kCMIOObjectPropertyName: u32 = fourcc(b"lnam");
const kCVPixelFormatType_32BGRA: u32 = fourcc(b"BGRA");
const kCFNumberSInt32Type: isize = 3;
const kCFStringEncodingUTF8: u32 = 0x0800_0100;

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
struct CMSampleTimingInfo {
    duration: CMTime,
    presentation: CMTime,
    decode: CMTime,
}

const INVALID_TIME: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };

#[link(name = "CoreMediaIO", kind = "framework")]
extern "C" {
    fn CMIOObjectGetPropertyDataSize(id: CMIOObjectID, addr: *const PropertyAddress, qsize: u32, qdata: *const c_void, size: *mut u32) -> OSStatus;
    fn CMIOObjectGetPropertyData(id: CMIOObjectID, addr: *const PropertyAddress, qsize: u32, qdata: *const c_void, size: u32, used: *mut u32, data: *mut c_void) -> OSStatus;
    fn CMIOObjectSetPropertyData(id: CMIOObjectID, addr: *const PropertyAddress, qsize: u32, qdata: *const c_void, size: u32, data: *const c_void) -> OSStatus;
    fn CMIOStreamCopyBufferQueue(stream: CMIOObjectID, altered: Option<unsafe extern "C" fn(CMIOObjectID, *mut c_void, *mut c_void)>, refcon: *mut c_void, queue: *mut CFTypeRef) -> OSStatus;
    fn CMIODeviceStartStream(device: CMIOObjectID, stream: CMIOObjectID) -> OSStatus;
    fn CMIODeviceStopStream(device: CMIOObjectID, stream: CMIOObjectID) -> OSStatus;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMVideoFormatDescriptionCreate(alloc: CFTypeRef, codec: u32, width: i32, height: i32, extensions: CFTypeRef, out: *mut CFTypeRef) -> OSStatus;
    fn CMSampleBufferCreateForImageBuffer(
        alloc: CFTypeRef,
        image: CFTypeRef,
        data_ready: u8,
        callback: *const c_void,
        refcon: *const c_void,
        format: CFTypeRef,
        timing: *const CMSampleTimingInfo,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMSimpleQueueEnqueue(queue: CFTypeRef, element: *const c_void) -> OSStatus;
    fn CMSimpleQueueGetCount(queue: CFTypeRef) -> i32;
    fn CMSimpleQueueGetCapacity(queue: CFTypeRef) -> i32;
    fn CMClockGetHostTimeClock() -> CFTypeRef;
    fn CMClockGetTime(clock: CFTypeRef) -> CMTime;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFTypeRef;
    static kCVPixelBufferWidthKey: CFTypeRef;
    static kCVPixelBufferHeightKey: CFTypeRef;
    static kCVPixelBufferIOSurfacePropertiesKey: CFTypeRef;
    fn CVPixelBufferPoolCreate(alloc: CFTypeRef, pool_attrs: CFTypeRef, buffer_attrs: CFTypeRef, out: *mut CFTypeRef) -> i32;
    fn CVPixelBufferPoolCreatePixelBuffer(alloc: CFTypeRef, pool: CFTypeRef, out: *mut CFTypeRef) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: CFTypeRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: CFTypeRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: CFTypeRef) -> *mut u8;
    fn CVPixelBufferGetBytesPerRow(buffer: CFTypeRef) -> usize;
    fn CVPixelBufferGetHeight(buffer: CFTypeRef) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: [usize; 6];
    static kCFTypeDictionaryValueCallBacks: [usize; 5];
    fn CFDictionaryCreate(alloc: CFTypeRef, keys: *const CFTypeRef, values: *const CFTypeRef, n: isize, kcb: *const c_void, vcb: *const c_void) -> CFTypeRef;
    fn CFNumberCreate(alloc: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut u8, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: CFTypeRef);
}

/// Owns one CoreFoundation reference.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn dictionary(pairs: &[(CFTypeRef, CFTypeRef)]) -> Cf {
    let (keys, values): (Vec<_>, Vec<_>) = pairs.iter().copied().unzip();
    unsafe {
        Cf(CFDictionaryCreate(
            null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as isize,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        ))
    }
}

fn number(v: i32) -> Cf {
    Cf(unsafe { CFNumberCreate(null(), kCFNumberSInt32Type, &v as *const i32 as *const c_void) })
}

fn cf_string(s: CFTypeRef) -> String {
    let mut buf = [0u8; 512];
    if s.is_null() || unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, kCFStringEncodingUTF8) } == 0 {
        return String::new();
    }
    String::from_utf8_lossy(buf.split(|&b| b == 0).next().unwrap_or_default()).into_owned()
}

fn address(selector: u32) -> PropertyAddress {
    PropertyAddress { selector, scope: kCMIOObjectPropertyScopeGlobal, element: kCMIOObjectPropertyElementMain }
}

fn object_ids(object: CMIOObjectID, selector: u32) -> Vec<CMIOObjectID> {
    let addr = address(selector);
    let mut size = 0u32;
    if unsafe { CMIOObjectGetPropertyDataSize(object, &addr, 0, null(), &mut size) } != 0 {
        return Vec::new();
    }
    let mut ids = vec![0 as CMIOObjectID; size as usize / std::mem::size_of::<CMIOObjectID>()];
    let mut used = 0u32;
    if unsafe { CMIOObjectGetPropertyData(object, &addr, 0, null(), size, &mut used, ids.as_mut_ptr() as *mut c_void) } != 0 {
        return Vec::new();
    }
    ids.truncate(used as usize / std::mem::size_of::<CMIOObjectID>());
    ids
}

fn string_property(object: CMIOObjectID, selector: u32) -> String {
    let addr = address(selector);
    let mut s: CFTypeRef = null();
    let mut used = 0u32;
    let size = std::mem::size_of::<CFTypeRef>() as u32;
    if unsafe { CMIOObjectGetPropertyData(object, &addr, 0, null(), size, &mut used, &mut s as *mut CFTypeRef as *mut c_void) } != 0 {
        return String::new();
    }
    let s = Cf(s);
    cf_string(s.0)
}

unsafe extern "C" fn queue_altered(_stream: CMIOObjectID, _token: *mut c_void, _refcon: *mut c_void) {}

pub struct ObsCamera {
    device: CMIOObjectID,
    stream: CMIOObjectID,
    queue: Cf,
    format: Cf,
    pool: Cf,
    size: [u32; 2],
}

// CoreMedia/CoreVideo objects are thread-safe to use from the single render thread that owns this value.
unsafe impl Send for ObsCamera {}

impl ObsCamera {
    /// Connects to the OBS camera extension's sink stream; returns it and the device name apps show.
    pub fn open(size: [u32; 2]) -> Result<(Self, String)> {
        unsafe {
            // Let CoreMediaIO deliver its notifications on its own thread: this process has no main run loop.
            let null_loop: CFTypeRef = null();
            let addr = address(kCMIOHardwarePropertyRunLoop);
            CMIOObjectSetPropertyData(kCMIOObjectSystemObject, &addr, 0, null(), std::mem::size_of::<CFTypeRef>() as u32, &null_loop as *const CFTypeRef as *const c_void);
        }
        let device = object_ids(kCMIOObjectSystemObject, kCMIOHardwarePropertyDevices)
            .into_iter()
            .find(|&d| string_property(d, kCMIODevicePropertyDeviceUID).eq_ignore_ascii_case(OBS_DEVICE_UID))
            .ok_or_else(|| anyhow!("OBS Virtual Camera not found — install OBS Studio 30+ and click \"Start Virtual Camera\" in it once to approve its system extension"))?;
        let name = Some(string_property(device, kCMIOObjectPropertyName)).filter(|n| !n.is_empty()).unwrap_or_else(|| "OBS Virtual Camera".into());
        let streams = object_ids(device, kCMIODevicePropertyStreams);
        let &[_, stream, ..] = streams.as_slice() else {
            bail!("the OBS Virtual Camera extension has no sink stream (try restarting the Mac after installing OBS)");
        };
        unsafe {
            let mut queue: CFTypeRef = null();
            // A callback is required: with none, CoreMediaIO reports success but hands back no queue.
            let status = CMIOStreamCopyBufferQueue(stream, Some(queue_altered), null_mut(), &mut queue);
            if status != 0 || queue.is_null() {
                bail!("could not attach to the OBS Virtual Camera sink (status {status})");
            }
            let queue = Cf(queue);
            let mut format: CFTypeRef = null();
            let status = CMVideoFormatDescriptionCreate(null(), kCVPixelFormatType_32BGRA, size[0] as i32, size[1] as i32, null(), &mut format);
            if status != 0 {
                bail!("CMVideoFormatDescriptionCreate failed ({status})");
            }
            let format = Cf(format);
            let (pixel_format, width, height, iosurface) = (number(kCVPixelFormatType_32BGRA as i32), number(size[0] as i32), number(size[1] as i32), dictionary(&[]));
            // IOSurface backing lets the frames cross into the extension's process without copying.
            let attrs = dictionary(&[
                (kCVPixelBufferPixelFormatTypeKey, pixel_format.0),
                (kCVPixelBufferWidthKey, width.0),
                (kCVPixelBufferHeightKey, height.0),
                (kCVPixelBufferIOSurfacePropertiesKey, iosurface.0),
            ]);
            let mut pool: CFTypeRef = null();
            if CVPixelBufferPoolCreate(null(), null(), attrs.0, &mut pool) != 0 {
                bail!("CVPixelBufferPoolCreate failed");
            }
            let pool = Cf(pool);
            let status = CMIODeviceStartStream(device, stream);
            if status != 0 {
                bail!("could not start the OBS Virtual Camera sink (status {status}) — is OBS's own virtual camera running?");
            }
            Ok((ObsCamera { device, stream, queue, format, pool, size }, name))
        }
    }
}

impl VirtualCamera for ObsCamera {
    fn send(&mut self, bgra: &[u8]) -> Result<()> {
        unsafe {
            // Nobody is draining the queue (no app is watching): drop the frame rather than block.
            if CMSimpleQueueGetCount(self.queue.0) >= CMSimpleQueueGetCapacity(self.queue.0) {
                return Ok(());
            }
            let mut buffer: CFTypeRef = null();
            if CVPixelBufferPoolCreatePixelBuffer(null(), self.pool.0, &mut buffer) != 0 {
                bail!("CVPixelBufferPoolCreatePixelBuffer failed");
            }
            let buffer = Cf(buffer);
            CVPixelBufferLockBaseAddress(buffer.0, 0);
            let base = CVPixelBufferGetBaseAddress(buffer.0);
            let stride = CVPixelBufferGetBytesPerRow(buffer.0);
            let row = self.size[0] as usize * 4;
            for y in 0..(self.size[1] as usize).min(CVPixelBufferGetHeight(buffer.0)) {
                std::ptr::copy_nonoverlapping(bgra.as_ptr().add(y * row), base.add(y * stride), row.min(stride));
            }
            CVPixelBufferUnlockBaseAddress(buffer.0, 0);

            let timing = CMSampleTimingInfo { duration: INVALID_TIME, presentation: CMClockGetTime(CMClockGetHostTimeClock()), decode: INVALID_TIME };
            let mut sample: CFTypeRef = null();
            let status = CMSampleBufferCreateForImageBuffer(null(), buffer.0, 1, null(), null(), self.format.0, &timing, &mut sample);
            if status != 0 {
                bail!("CMSampleBufferCreateForImageBuffer failed ({status})");
            }
            // On success the queue's consumer takes ownership of `sample`.
            if CMSimpleQueueEnqueue(self.queue.0, sample) != 0 {
                CFRelease(sample);
            }
        }
        Ok(())
    }
}

impl Drop for ObsCamera {
    fn drop(&mut self) {
        unsafe {
            CMIODeviceStopStream(self.device, self.stream);
        }
    }
}
