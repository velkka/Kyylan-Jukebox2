//! [`AudioPlayer`]: the [`Player`] the queue engine drives, playing through an output device.
//!
//! Calls return at once; a control thread does the work. It opens files, starts a decoder
//! per song, owns the output, and watches for what the engine needs to hear about: audio
//! starting, a song ending, a file that can't be played. It also keeps playback going when
//! the output misbehaves — an unplugged device moves playback to the default one from the
//! same position, and with no device at all it waits and tries again.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use jukebox_core::player::{EventListener, LoadId, Player, PlayerEvent, StateListener};
use jukebox_core::types::{AudioDevice, PlaybackState};
use rtrb::Producer;

use crate::output::{Backend, Output, Shared};
use crate::pipeline::{Command, Decoder, Failure};
use crate::source::Source;

/// How often playback position is reported while playing. The realtime broadcast throttles
/// further.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How often to try for an output again when there isn't one.
const RETRY_OUTPUT_EVERY: Duration = Duration::from_secs(2);
/// The id the admin panel lists the system default under, as Chromium did.
pub const DEFAULT_DEVICE: &str = "default";

/// Finds a track's file.
pub type TrackResolver = Arc<dyn Fn(i64) -> Option<PathBuf> + Send + Sync>;

enum Message {
    Load {
        load: LoadId,
        track_id: i64,
        autoplay: bool,
    },
    Play,
    Pause,
    Seek(f64),
    Volume(f64),
    Device(String),
    RefreshDevices,
    Shutdown,
}

#[derive(Default)]
struct Listeners {
    state: RwLock<Option<StateListener>>,
    events: RwLock<Option<EventListener>>,
}

impl Listeners {
    fn state(&self, state: &PlaybackState) {
        if let Some(listener) = self.state.read().unwrap().as_ref() {
            listener(state);
        }
    }

    fn event(&self, event: PlayerEvent) {
        if let Some(listener) = self.events.read().unwrap().clone() {
            listener(event);
        }
    }
}

pub struct AudioPlayer {
    tx: Sender<Message>,
    state: Arc<Mutex<PlaybackState>>,
    devices: Arc<Mutex<Vec<AudioDevice>>>,
    listeners: Arc<Listeners>,
    loads: AtomicU64,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl AudioPlayer {
    /// Starts the player on `backend`, with `device` as the chosen output — `None` for the
    /// system default — at full volume.
    pub fn new(backend: impl Backend, resolver: TrackResolver, device: Option<String>) -> Self {
        let (tx, rx) = channel();
        let state = Arc::new(Mutex::new(PlaybackState {
            track_id: None,
            playing: false,
            position: 0.0,
            duration: 0.0,
            volume: 1.0,
        }));
        let devices = Arc::new(Mutex::new(Vec::new()));
        let listeners = Arc::new(Listeners::default());
        let shared = Arc::new(Shared::default());
        shared.gain_bits.store(1.0f32.to_bits(), Relaxed);

        let (failures_tx, failures) = channel();
        let wanted = device.filter(|d| !d.is_empty() && d != DEFAULT_DEVICE);
        let (thread_state, thread_devices, thread_listeners) =
            (state.clone(), devices.clone(), listeners.clone());
        // An open output can't move between threads on every platform, so the control
        // thread builds everything that will hold one.
        let thread = thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let mut control = Control {
                    backend: Box::new(backend),
                    resolver,
                    shared,
                    wanted,
                    output: None,
                    spare: None,
                    song: None,
                    state: thread_state,
                    devices: thread_devices,
                    listeners: thread_listeners,
                    failures_tx,
                    last_progress: Instant::now(),
                    last_output_attempt: None,
                };
                control.refresh_devices();
                control.run(rx, failures);
            })
            .expect("starting the audio thread");
        AudioPlayer {
            tx,
            state,
            devices,
            listeners,
            loads: AtomicU64::new(0),
            thread: Mutex::new(Some(thread)),
        }
    }

    fn change(&self, apply: impl FnOnce(&mut PlaybackState)) {
        let state = {
            let mut state = self.state.lock().unwrap();
            apply(&mut state);
            state.clone()
        };
        self.listeners.state(&state);
    }

    fn send(&self, message: Message) {
        let _ = self.tx.send(message);
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.send(Message::Shutdown);
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

impl Player for AudioPlayer {
    fn load(&self, track_id: i64, autoplay: bool) -> LoadId {
        let load = self.loads.fetch_add(1, Ordering::SeqCst) + 1;
        // Reported at once, as player.ts did, so the queue shows the new song straight away.
        self.change(|s| {
            s.track_id = Some(track_id);
            s.position = 0.0;
            s.duration = 0.0;
            s.playing = autoplay;
        });
        self.send(Message::Load {
            load,
            track_id,
            autoplay,
        });
        load
    }

    fn current_load(&self) -> Option<LoadId> {
        Some(self.loads.load(Ordering::SeqCst)).filter(|&l| l > 0)
    }

    fn play(&self) {
        self.change(|s| s.playing = true);
        self.send(Message::Play);
    }

    fn pause(&self) {
        self.change(|s| s.playing = false);
        self.send(Message::Pause);
    }

    fn seek(&self, position: f64) {
        self.send(Message::Seek(position));
    }

    fn set_volume(&self, value: f64) {
        let value = value.clamp(0.0, 1.0);
        self.change(|s| s.volume = value);
        self.send(Message::Volume(value));
    }

    fn set_output_device(&self, device_id: &str) {
        self.send(Message::Device(device_id.to_string()));
    }

    fn request_devices(&self) {
        self.send(Message::RefreshDevices);
    }

    fn devices(&self) -> Vec<AudioDevice> {
        self.devices.lock().unwrap().clone()
    }

    fn state(&self) -> PlaybackState {
        self.state.lock().unwrap().clone()
    }

    fn on_state_change(&self, listener: StateListener) {
        *self.listeners.state.write().unwrap() = Some(listener);
    }

    fn on_event(&self, listener: EventListener) {
        *self.listeners.events.write().unwrap() = Some(listener);
    }
}

/// The song being played.
struct Song {
    load: LoadId,
    /// Decoding, once there's an output to decode for.
    decoder: Option<Decoder>,
    /// Opened and waiting for an output.
    waiting: Option<Source>,
    /// Where to start once decoding begins, if a seek came first.
    start_at: Option<f64>,
    started: bool,
}

struct Control {
    backend: Box<dyn Backend>,
    resolver: TrackResolver,
    shared: Arc<Shared>,
    /// The device the admin chose; `None` follows the system default.
    wanted: Option<String>,
    output: Option<Box<dyn Output>>,
    /// The open output's buffer, while no decoder is writing to it.
    spare: Option<Producer<f32>>,
    song: Option<Song>,
    state: Arc<Mutex<PlaybackState>>,
    devices: Arc<Mutex<Vec<AudioDevice>>>,
    listeners: Arc<Listeners>,
    failures_tx: Sender<Failure>,
    last_progress: Instant,
    last_output_attempt: Option<Instant>,
}

impl Control {
    fn run(&mut self, rx: Receiver<Message>, failures: Receiver<Failure>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
            }
            while let Ok(failure) = failures.try_recv() {
                if self.song.as_ref().is_some_and(|s| s.load == failure.load) {
                    self.fail(failure.reason);
                }
            }
            self.watch();
        }
        self.stop_song();
        self.output = None;
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Load {
                load,
                track_id,
                autoplay,
            } => self.load(load, track_id, autoplay),
            Message::Play => self.shared.playing.store(true, Relaxed),
            Message::Pause => self.shared.playing.store(false, Relaxed),
            Message::Seek(secs) => match self.song.as_mut() {
                Some(Song {
                    decoder: Some(decoder),
                    ..
                }) => decoder.send(Command::Seek(secs)),
                Some(song) => song.start_at = Some(secs),
                None => {}
            },
            Message::Volume(value) => self
                .shared
                .gain_bits
                .store((value as f32).to_bits(), Relaxed),
            Message::Device(id) => {
                let wanted = Some(id).filter(|d| !d.is_empty() && d != DEFAULT_DEVICE);
                if wanted != self.wanted || self.output.is_none() {
                    self.wanted = wanted;
                    self.reopen_output();
                }
            }
            Message::RefreshDevices => self.refresh_devices(),
            Message::Shutdown => {}
        }
    }

    fn refresh_devices(&mut self) {
        let mut list = vec![AudioDevice {
            device_id: DEFAULT_DEVICE.into(),
            label: match self.backend.default_name() {
                Some(name) => format!("Default - {name}"),
                None => "Default".into(),
            },
        }];
        list.extend(self.backend.devices());
        *self.devices.lock().unwrap() = list;
    }

    fn update_state(&self, apply: impl FnOnce(&mut PlaybackState)) {
        let state = {
            let mut state = self.state.lock().unwrap();
            apply(&mut state);
            state.clone()
        };
        self.listeners.state(&state);
    }

    /// Stops the current song's decoder, keeping its buffer for the next one, and discards
    /// what's buffered.
    fn stop_song(&mut self) {
        if let Some(song) = self.song.take() {
            if let Some(producer) = song.decoder.and_then(Decoder::stop) {
                self.spare = Some(producer);
            }
        }
        if self.output.is_some() {
            self.shared.flush.store(true, Relaxed);
            let asked = Instant::now();
            while self.shared.flush.load(Relaxed) && asked.elapsed() < Duration::from_millis(200) {
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.shared.eof.store(false, Relaxed);
        self.shared.drained.store(false, Relaxed);
        self.shared.frames_played.store(0, Relaxed);
        self.shared.base_pos_bits.store(0f64.to_bits(), Relaxed);
    }

    fn load(&mut self, load: LoadId, track_id: i64, autoplay: bool) {
        self.stop_song();
        self.shared.playing.store(autoplay, Relaxed);
        self.song = Some(Song {
            load,
            decoder: None,
            waiting: None,
            start_at: None,
            started: false,
        });
        let Some(path) = (self.resolver)(track_id) else {
            return self.fail("the song is no longer in the library".into());
        };
        let source = match Source::open(&path) {
            Ok(source) => source,
            Err(reason) => return self.fail(reason),
        };
        if let Some(duration) = source.duration() {
            self.update_state(|s| s.duration = duration);
        }
        if self.output.is_none() {
            self.try_open_output();
        }
        if let Some(song) = self.song.as_mut() {
            song.waiting = Some(source);
        }
        self.start_decoding();
    }

    /// Starts the waiting song's decoder, if there's an output for it.
    fn start_decoding(&mut self) {
        let (Some(output), Some(song)) = (self.output.as_ref(), self.song.as_mut()) else {
            return;
        };
        let (Some(source), Some(producer)) = (song.waiting.take(), self.spare.take()) else {
            return;
        };
        self.shared.out_rate.store(output.rate(), Relaxed);
        let decoder = Decoder::start(
            source,
            Some((producer, output.rate(), output.channels())),
            self.shared.clone(),
            song.load,
            self.failures_tx.clone(),
        );
        if let Some(secs) = song.start_at.take() {
            decoder.send(Command::Seek(secs));
        }
        song.decoder = Some(decoder);
    }

    /// The song couldn't be played: report it and stop.
    fn fail(&mut self, reason: String) {
        let Some(load) = self.song.as_ref().map(|s| s.load) else {
            return;
        };
        self.stop_song();
        self.shared.playing.store(false, Relaxed);
        self.update_state(|s| s.playing = false);
        self.listeners.event(PlayerEvent::Failed { load, reason });
    }

    /// Opens the chosen output, or the default if that isn't there.
    fn try_open_output(&mut self) -> bool {
        self.last_output_attempt = Some(Instant::now());
        // The setting holds a device id, as the admin panel saves it, or a device's name, as
        // someone editing the file would write it.
        let wanted = self.wanted.as_deref().map(|wanted| {
            let devices = self.backend.devices();
            match devices.iter().any(|d| d.device_id == wanted) {
                true => wanted.to_string(),
                false => devices
                    .into_iter()
                    .find(|d| d.label == wanted)
                    .map_or_else(|| wanted.to_string(), |d| d.device_id),
            }
        });
        let opened = match wanted.as_deref() {
            Some(id) => self.backend.open(Some(id), self.shared.clone()).or_else(|err| {
                tracing::warn!(device = id, %err, "the chosen audio output isn't available; using the default");
                self.backend.open(None, self.shared.clone())
            }),
            None => self.backend.open(None, self.shared.clone()),
        };
        match opened {
            Ok((output, producer)) => {
                tracing::info!(device = output.device_id(), "audio output open");
                self.shared.failed.store(false, Relaxed);
                self.output = Some(output);
                self.spare = Some(producer);
                true
            }
            Err(err) => {
                tracing::warn!(%err, "no audio output; will keep trying");
                false
            }
        }
    }

    /// Replaces the output — after a failure, or to follow a new choice — carrying on from
    /// the same position.
    fn reopen_output(&mut self) {
        let position = self.shared.position();
        if let Some(Song {
            decoder: Some(decoder),
            ..
        }) = &self.song
        {
            decoder.send(Command::Detach);
        }
        self.output = None;
        self.spare = None;
        if !self.try_open_output() {
            return;
        }
        let output = self.output.as_ref().expect("just opened");
        let (rate, channels) = (output.rate(), output.channels());
        match self.song.as_mut() {
            Some(Song {
                decoder: Some(decoder),
                ..
            }) => {
                let producer = self.spare.take().expect("just opened");
                self.shared.out_rate.store(rate, Relaxed);
                decoder.send(Command::Retarget {
                    producer,
                    rate,
                    channels,
                    resume_at: position,
                });
            }
            _ => self.start_decoding(),
        }
    }

    /// Everything that isn't a message: the output's health, the song's progress.
    fn watch(&mut self) {
        if self.shared.failed.swap(false, Relaxed) {
            tracing::warn!("the audio output stopped; reopening it");
            self.reopen_output();
        }
        let retry_due = self
            .last_output_attempt
            .is_none_or(|at| at.elapsed() >= RETRY_OUTPUT_EVERY);
        if self.output.is_none() && retry_due && self.song.is_some() {
            self.reopen_output();
        }

        let Some(song) = self.song.as_mut() else {
            return;
        };
        let load = song.load;
        if !song.started && self.shared.frames_played.load(Relaxed) > 0 {
            song.started = true;
            self.listeners.event(PlayerEvent::Started { load });
        }
        if self.shared.drained.load(Relaxed) {
            let started = song.started;
            if !started {
                return self.fail("no audio came out of the file".into());
            }
            self.stop_song();
            self.shared.playing.store(false, Relaxed);
            let position = self.state.lock().unwrap().duration;
            self.update_state(|s| {
                s.playing = false;
                s.position = position;
            });
            self.listeners.event(PlayerEvent::Ended { load });
            return;
        }
        let playing = self.shared.playing.load(Relaxed);
        if playing && self.last_progress.elapsed() >= PROGRESS_EVERY {
            self.last_progress = Instant::now();
            let position = self.shared.position();
            self.update_state(|s| {
                s.position = position;
                s.playing = true;
            });
        }
    }
}
