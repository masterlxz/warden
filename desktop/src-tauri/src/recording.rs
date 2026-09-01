//! Native microphone capture for voice input (P28). WebKitGTK denies every browser
//! `getUserMedia` request by default on Linux — Tauri never wires up the `permission-request`
//! signal WebKit needs to grant it (https://github.com/tauri-apps/tauri/issues/12547), and doing
//! that by hand is a GTK-specific hack the Tauri team itself doesn't consider safe to ship.
//! Recording here in the Rust backend instead sidesteps the webview permission system
//! altogether, and behaves the same on every platform.
//!
//! `cpal::Stream` isn't reliably `Send` across all platform backends, so the stream never leaves
//! the OS thread that created it: `start()` spawns a dedicated thread that owns the stream for
//! its whole lifetime, blocking on a channel until `stop()` signals it to tear down.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct ActiveRecording {
    stop_tx: mpsc::Sender<()>,
    join_handle: JoinHandle<Result<(Vec<i16>, u32), String>>,
}

/// Downmixes every input format cpal might hand back to mono `i16` PCM — simplest common
/// denominator that `hound`/Whisper both take with no further conversion.
fn push_f32(buffer: &Arc<Mutex<Vec<i16>>>, data: &[f32], channels: usize) {
    let mut buf = buffer.lock().unwrap();
    for frame in data.chunks(channels.max(1)) {
        let avg = frame.iter().sum::<f32>() / frame.len() as f32;
        buf.push((avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
    }
}

fn push_i16(buffer: &Arc<Mutex<Vec<i16>>>, data: &[i16], channels: usize) {
    let mut buf = buffer.lock().unwrap();
    for frame in data.chunks(channels.max(1)) {
        let avg = frame.iter().map(|&s| s as i32).sum::<i32>() / frame.len() as i32;
        buf.push(avg as i16);
    }
}

fn push_u16(buffer: &Arc<Mutex<Vec<i16>>>, data: &[u16], channels: usize) {
    let mut buf = buffer.lock().unwrap();
    for frame in data.chunks(channels.max(1)) {
        let avg = frame.iter().map(|&s| s as i32 - i32::from(u16::MAX / 2)).sum::<i32>() / frame.len() as i32;
        buf.push(avg as i16);
    }
}

/// Opens the default input device and starts capturing. Fails fast (before returning) if no
/// device exists or the stream can't be built, instead of only surfacing that once `stop()` is
/// called.
fn build_stream(buffer: &Arc<Mutex<Vec<i16>>>) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| "No microphone found".to_string())?;
    let supported = device.default_input_config().map_err(|e| format!("{e}"))?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let err_fn = |err| eprintln!("audio input stream error: {err}");

    let buffer_cb = buffer.clone();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| push_f32(&buffer_cb, data, channels),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| push_i16(&buffer_cb, data, channels),
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| push_u16(&buffer_cb, data, channels),
            err_fn,
            None,
        ),
        other => return Err(format!("Unsupported microphone sample format: {other:?}")),
    }
    .map_err(|e| format!("{e}"))?;

    stream.play().map_err(|e| format!("{e}"))?;
    Ok((stream, sample_rate))
}

/// Starts recording on a dedicated thread. Blocks briefly (device open, not the recording
/// itself) so the caller finds out immediately if there's no microphone, rather than only on
/// `stop()`.
pub fn start() -> Result<ActiveRecording, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let join_handle = std::thread::spawn(move || -> Result<(Vec<i16>, u32), String> {
        let buffer = Arc::new(Mutex::new(Vec::<i16>::new()));
        match build_stream(&buffer) {
            Ok((stream, sample_rate)) => {
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv();
                drop(stream);
                let samples = std::mem::take(&mut *buffer.lock().unwrap());
                Ok((samples, sample_rate))
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e.clone()));
                Err(e)
            }
        }
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(ActiveRecording { stop_tx, join_handle }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Recording thread stopped unexpectedly".to_string()),
    }
}

impl ActiveRecording {
    /// Signals the recording thread to stop and waits for it to hand back the captured samples.
    pub fn stop(self) -> Result<(Vec<i16>, u32), String> {
        let _ = self.stop_tx.send(());
        self.join_handle.join().map_err(|_| "Recording thread panicked".to_string())?
    }
}

/// Encodes mono 16-bit PCM samples as a WAV file — the simplest format both `hound` and Whisper
/// take with no further conversion.
pub fn encode_wav(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec { channels: 1, sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| format!("{e}"))?;
        for &sample in samples {
            writer.write_sample(sample).map_err(|e| format!("{e}"))?;
        }
        writer.finalize().map_err(|e| format!("{e}"))?;
    }
    Ok(cursor.into_inner())
}
