//! Microphone loudness for sound reactivity (cpal: CoreAudio / WASAPI / ALSA·PulseAudio·PipeWire).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

use crate::reactivity::sound_signal;

#[derive(Default)]
struct Shared {
    level: AtomicU32,
    stop: AtomicBool,
    error: Mutex<Option<String>>,
}

/// A running microphone. Dropping it releases the device.
pub struct Microphone {
    shared: Arc<Shared>,
}

impl Microphone {
    pub fn open() -> Self {
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        // cpal streams are not Send on every platform, so the stream lives and dies on its own thread.
        let _ = std::thread::Builder::new().name("microphone".into()).spawn(move || {
            if let Err(e) = run(&thread_shared) {
                *thread_shared.error.lock().unwrap() = Some(e);
            }
        });
        Microphone { shared }
    }

    /// Latest 0..1 loudness.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// Converts any integer or float sample block to -1..1 floats.
fn to_f32(data: &cpal::Data, out: &mut Vec<f32>) -> bool {
    out.clear();
    match data.sample_format() {
        SampleFormat::F32 => out.extend_from_slice(data.as_slice::<f32>().unwrap_or_default()),
        SampleFormat::I16 => out.extend(data.as_slice::<i16>().unwrap_or_default().iter().map(|&s| s as f32 / 32768.0)),
        SampleFormat::U16 => out.extend(data.as_slice::<u16>().unwrap_or_default().iter().map(|&s| (s as f32 - 32768.0) / 32768.0)),
        SampleFormat::I32 => out.extend(data.as_slice::<i32>().unwrap_or_default().iter().map(|&s| s as f32 / 2147483648.0)),
        SampleFormat::F64 => out.extend(data.as_slice::<f64>().unwrap_or_default().iter().map(|&s| s as f32)),
        _ => return false,
    }
    true
}

fn run(shared: &Arc<Shared>) -> Result<(), String> {
    let device = cpal::default_host().default_input_device().ok_or("no microphone found")?;
    let config = device.default_input_config().map_err(|e| format!("microphone unavailable: {e}"))?;
    let format = config.sample_format();
    let (cb_shared, err_shared) = (shared.clone(), shared.clone());
    let mut samples = Vec::new();
    let stream = device
        .build_input_stream_raw(
            config.config(),
            format,
            move |data: &cpal::Data, _: &cpal::InputCallbackInfo| {
                if to_f32(data, &mut samples) {
                    cb_shared.level.store(sound_signal(&samples).to_bits(), Ordering::Relaxed);
                }
            },
            move |e| *err_shared.error.lock().unwrap() = Some(format!("microphone error: {e}")),
            None,
        )
        .map_err(|e| format!("could not open microphone ({format:?}): {e}"))?;
    stream.play().map_err(|e| format!("could not start microphone: {e}"))?;
    while !shared.stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
