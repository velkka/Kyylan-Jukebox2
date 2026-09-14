//! Where decoded audio goes: an output device, fed from a lock-free ring buffer.
//!
//! [`render`] is the realtime half, run for every buffer the device asks for. It reads only
//! atomics and the ring buffer — no locks, no allocation — applies a smoothed volume, and
//! counts the frames that actually reached the device, which is what the position is built
//! from.
//!
//! A [`Backend`] opens devices. [`CpalBackend`] is the real one. [`VirtualBackend`] plays
//! into nothing at whatever speed a test asks for, with devices that can be unplugged,
//! so the player's logic runs in CI where there's no sound card.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use jukebox_core::types::AudioDevice;
use rtrb::{Consumer, Producer, RingBuffer};

/// How much decoded audio the ring buffer holds.
const BUFFER_SECS: f64 = 0.5;

/// State shared by the player, the decoder and the output callback.
#[derive(Default)]
pub struct Shared {
    /// Audio should be heard.
    pub(crate) playing: AtomicBool,
    /// Set to have the callback discard everything buffered; cleared by the callback.
    pub(crate) flush: AtomicBool,
    /// The decoder has reached the end of the file.
    pub(crate) eof: AtomicBool,
    /// Playing, at the end of the file, and nothing left in the buffer.
    pub(crate) drained: AtomicBool,
    pub(crate) gain_bits: AtomicU32,
    pub(crate) out_rate: AtomicU32,
    /// The file position, in seconds, that `frames_played` counts from.
    pub(crate) base_pos_bits: AtomicU64,
    pub(crate) frames_played: AtomicU64,
    /// The output reported an error it won't recover from, such as its device vanishing.
    pub(crate) failed: AtomicBool,
}

impl Shared {
    pub(crate) fn position(&self) -> f64 {
        let base = f64::from_bits(self.base_pos_bits.load(Relaxed));
        let rate = f64::from(self.out_rate.load(Relaxed).max(1));
        base + self.frames_played.load(Relaxed) as f64 / rate
    }
}

/// Ring buffer for an output of this shape.
pub(crate) fn ring(rate: u32, channels: usize) -> (Producer<f32>, Consumer<f32>) {
    RingBuffer::new((BUFFER_SECS * f64::from(rate)) as usize * channels.max(1))
}

/// Fills one device buffer. `gain` is the callback's own smoothed volume.
pub(crate) fn render<T: SizedSample + FromSample<f32>>(
    shared: &Shared,
    consumer: &mut Consumer<f32>,
    data: &mut [T],
    channels: usize,
    gain: &mut f32,
) {
    if shared.flush.load(Relaxed) {
        let n = consumer.slots();
        if let Ok(chunk) = consumer.read_chunk(n) {
            chunk.commit_all();
        }
        shared.flush.store(false, Relaxed);
    }
    let playing = shared.playing.load(Relaxed);
    let target = f32::from_bits(shared.gain_bits.load(Relaxed));
    // One-pole smoothing, ~10 ms at 48 kHz: volume changes glide instead of clicking.
    const SMOOTHING: f32 = 1.0 / 480.0;
    let frames = data.len() / channels.max(1);
    let mut delivered = 0u64;
    for frame in data.chunks_mut(channels.max(1)) {
        if playing && consumer.slots() >= channels {
            *gain += (target - *gain) * SMOOTHING;
            for sample in frame.iter_mut() {
                *sample = T::from_sample(consumer.pop().unwrap_or(0.0) * *gain);
            }
            delivered += 1;
        } else {
            for sample in frame.iter_mut() {
                *sample = T::EQUILIBRIUM;
            }
        }
    }
    if playing && delivered < frames as u64 && shared.eof.load(Relaxed) {
        shared.drained.store(true, Relaxed);
    }
    shared.frames_played.fetch_add(delivered, Relaxed);
}

/// An open output. Dropping it stops the device.
pub trait Output {
    fn rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// The device's id, as [`Backend::devices`] lists it.
    fn device_id(&self) -> &str;
}

pub trait Backend: Send + 'static {
    /// Output devices, as the admin panel lists them.
    fn devices(&self) -> Vec<AudioDevice>;
    /// The system default device's name, if there is one.
    fn default_name(&self) -> Option<String>;
    /// Opens a device — `None` for the system default — feeding it from a new ring buffer,
    /// whose writing end is returned.
    fn open(
        &self,
        device: Option<&str>,
        shared: Arc<Shared>,
    ) -> Result<(Box<dyn Output>, Producer<f32>), String>;
}

// ---- cpal ------------------------------------------------------------------------------

/// The platform's audio devices, through cpal: CoreAudio, WASAPI, or ALSA.
pub struct CpalBackend {
    host: cpal::Host,
}

impl Default for CpalBackend {
    fn default() -> Self {
        CpalBackend {
            host: cpal::default_host(),
        }
    }
}

struct CpalOutput {
    _stream: cpal::Stream,
    id: String,
    rate: u32,
    channels: usize,
}

impl Output for CpalOutput {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn device_id(&self) -> &str {
        &self.id
    }
}

fn device_id(device: &cpal::Device) -> String {
    device.id().map(|i| i.to_string()).unwrap_or_default()
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| device_id(device))
}

impl CpalBackend {
    fn open_device(
        &self,
        device: &cpal::Device,
        shared: Arc<Shared>,
    ) -> Result<(Box<dyn Output>, Producer<f32>), String> {
        let supported = device
            .default_output_config()
            .map_err(|e| format!("{}: {e}", device_name(device)))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let (rate, channels) = (config.sample_rate, config.channels as usize);
        let (producer, consumer) = ring(rate, channels);
        let stream = match format {
            SampleFormat::F32 => build::<f32>(device, config, shared, consumer),
            SampleFormat::I16 => build::<i16>(device, config, shared, consumer),
            SampleFormat::I32 => build::<i32>(device, config, shared, consumer),
            SampleFormat::U16 => build::<u16>(device, config, shared, consumer),
            other => Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| format!("{}: {e}", device_name(device)))?;
        stream
            .play()
            .map_err(|e| format!("{}: {e}", device_name(device)))?;
        Ok((
            Box::new(CpalOutput {
                _stream: stream,
                id: device_id(device),
                rate,
                channels,
            }),
            producer,
        ))
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    shared: Arc<Shared>,
    mut consumer: Consumer<f32>,
) -> Result<cpal::Stream, String> {
    let channels = config.channels as usize;
    let mut gain = 0.0f32;
    let on_error = shared.clone();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                render(&shared, &mut consumer, data, channels, &mut gain);
            },
            move |err: cpal::Error| match err.kind() {
                // A glitch, not a lost device.
                cpal::ErrorKind::Xrun => {}
                _ => {
                    tracing::warn!(%err, "audio output failed");
                    on_error.failed.store(true, Relaxed);
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}

impl Backend for CpalBackend {
    fn devices(&self) -> Vec<AudioDevice> {
        self.host
            .output_devices()
            .map(|devices| {
                devices
                    .map(|d| AudioDevice {
                        device_id: device_id(&d),
                        label: device_name(&d),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn default_name(&self) -> Option<String> {
        self.host.default_output_device().map(|d| device_name(&d))
    }

    fn open(
        &self,
        device: Option<&str>,
        shared: Arc<Shared>,
    ) -> Result<(Box<dyn Output>, Producer<f32>), String> {
        if let Some(id) = device {
            let parsed: cpal::DeviceId =
                id.parse().map_err(|_| format!("no output device {id}"))?;
            let device = self
                .host
                .device_by_id(&parsed)
                .ok_or_else(|| format!("no output device {id}"))?;
            return self.open_device(&device, shared);
        }
        let mut errors = Vec::new();
        if let Some(default) = self.host.default_output_device() {
            match self.open_device(&default, shared.clone()) {
                Ok(opened) => return Ok(opened),
                Err(e) => errors.push(e),
            }
        }
        // A system service on Linux can't reach a desktop's sound server, and with
        // pipewire-alsa installed ALSA's "default" leads there. The cards themselves work:
        // `plughw` devices convert formats as needed.
        if cfg!(target_os = "linux") {
            if let Ok(devices) = self.host.output_devices() {
                for device in devices.filter(|d| device_id(d).contains("plughw")) {
                    match self.open_device(&device, shared.clone()) {
                        Ok(opened) => return Ok(opened),
                        Err(e) => errors.push(e),
                    }
                }
            }
        }
        Err(if errors.is_empty() {
            "no output device".into()
        } else {
            errors.join("; ")
        })
    }
}

// ---- Virtual devices ---------------------------------------------------------------------

/// Devices that play into nothing, `speed` times faster than real time, for tests.
#[derive(Clone)]
pub struct VirtualBackend {
    inner: Arc<VirtualInner>,
}

struct VirtualInner {
    devices: Mutex<Vec<VirtualDevice>>,
    speed: f64,
    /// Every sample rendered, per device id, when recording.
    recordings: Mutex<Vec<(String, Vec<f32>)>>,
    record: bool,
}

#[derive(Clone)]
struct VirtualDevice {
    id: String,
    label: String,
    rate: u32,
    channels: usize,
    connected: Arc<AtomicBool>,
}

struct VirtualOutput {
    id: String,
    rate: u32,
    channels: usize,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Output for VirtualOutput {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn device_id(&self) -> &str {
        &self.id
    }
}

impl Drop for VirtualOutput {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl VirtualBackend {
    /// No devices yet; add some with [`add_device`](Self::add_device).
    pub fn new(speed: f64) -> Self {
        VirtualBackend {
            inner: Arc::new(VirtualInner {
                devices: Mutex::new(Vec::new()),
                speed,
                recordings: Mutex::new(Vec::new()),
                record: false,
            }),
        }
    }

    /// Like [`new`](Self::new), keeping every sample each device renders.
    pub fn recording(speed: f64) -> Self {
        VirtualBackend {
            inner: Arc::new(VirtualInner {
                devices: Mutex::new(Vec::new()),
                speed,
                recordings: Mutex::new(Vec::new()),
                record: true,
            }),
        }
    }

    /// A device, connected. The first connected device is the system default.
    pub fn add_device(&self, id: &str, label: &str, rate: u32, channels: usize) {
        self.inner.devices.lock().unwrap().push(VirtualDevice {
            id: id.into(),
            label: label.into(),
            rate,
            channels,
            connected: Arc::new(AtomicBool::new(true)),
        });
    }

    /// Unplugs or replugs a device. Unplugging fails an output playing to it.
    pub fn set_connected(&self, id: &str, connected: bool) {
        if let Some(device) = self
            .inner
            .devices
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
        {
            device.connected.store(connected, Relaxed);
        }
    }

    /// The samples a device has rendered so far, interleaved.
    pub fn recorded(&self, id: &str) -> Vec<f32> {
        self.inner
            .recordings
            .lock()
            .unwrap()
            .iter()
            .filter(|(device, _)| device == id)
            .flat_map(|(_, samples)| samples.iter().copied())
            .collect()
    }
}

impl Backend for VirtualBackend {
    fn devices(&self) -> Vec<AudioDevice> {
        self.inner
            .devices
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.connected.load(Relaxed))
            .map(|d| AudioDevice {
                device_id: d.id.clone(),
                label: d.label.clone(),
            })
            .collect()
    }

    fn default_name(&self) -> Option<String> {
        self.devices().first().map(|d| d.label.clone())
    }

    fn open(
        &self,
        device: Option<&str>,
        shared: Arc<Shared>,
    ) -> Result<(Box<dyn Output>, Producer<f32>), String> {
        let found = {
            let devices = self.inner.devices.lock().unwrap();
            devices
                .iter()
                .filter(|d| d.connected.load(Relaxed))
                .find(|d| device.is_none_or(|id| d.id == id))
                .cloned()
        };
        let device = found.ok_or_else(|| match device {
            Some(id) => format!("no output device {id}"),
            None => "no output device".into(),
        })?;
        let (producer, mut consumer) = ring(device.rate, device.channels);
        let stop = Arc::new(AtomicBool::new(false));
        let inner = self.inner.clone();
        let (thread_stop, id, rate, channels) = (
            stop.clone(),
            device.id.clone(),
            device.rate,
            device.channels,
        );
        let thread = thread::Builder::new()
            .name(format!("virtual output {id}"))
            .spawn(move || {
                // Buffers of 5 ms of device time, delivered `speed` times faster.
                let frames = (f64::from(rate) * 0.005) as usize;
                let period = Duration::from_secs_f64(0.005 / inner.speed);
                let mut buffer = vec![0.0f32; frames * channels];
                let mut gain = 0.0f32;
                let mut next = Instant::now();
                while !thread_stop.load(Relaxed) {
                    if !device.connected.load(Relaxed) {
                        shared.failed.store(true, Relaxed);
                        return;
                    }
                    render(&shared, &mut consumer, &mut buffer, channels, &mut gain);
                    if inner.record {
                        let mut recordings = inner.recordings.lock().unwrap();
                        match recordings.last_mut() {
                            Some((device, samples)) if *device == id => samples.extend(&buffer),
                            _ => recordings.push((id.clone(), buffer.clone())),
                        }
                    }
                    next += period;
                    let now = Instant::now();
                    if next > now {
                        thread::sleep(next - now);
                    } else if now - next > period * 100 {
                        // Hopelessly behind — a stalled test machine: start counting afresh.
                        next = now;
                    }
                    // Otherwise go straight on to the next buffer. Sleeps can't be shorter
                    // than the system timer's tick (about 15 ms on Windows), so a device
                    // that dropped the buffers it owed would run far slower than asked.
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((
            Box::new(VirtualOutput {
                id: device.id,
                rate,
                channels,
                stop,
                thread: Some(thread),
            }),
            producer,
        ))
    }
}
