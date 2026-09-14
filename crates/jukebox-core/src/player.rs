//! What the queue engine and the admin controls drive: the player. Mirrors src/main/player.ts.
//!
//! [`Player`] is the contract the audio engine implements in jukebox-audio. [`SilentPlayer`]
//! keeps the same playback state player.ts kept and plays nothing — which is exactly what
//! player.ts did whenever no player window was reporting back, and what the API parity
//! harness runs against.

use std::sync::{Arc, Mutex, RwLock};

use crate::types::{AudioDevice, PlaybackState};

/// Called with the new playback state whenever the player reports a change.
pub type StateListener = Arc<dyn Fn(&PlaybackState) + Send + Sync>;

pub trait Player: Send + Sync {
    /// Loads a track, from the start, and plays it if `autoplay`.
    fn load(&self, track_id: i64, autoplay: bool);
    fn play(&self);
    fn pause(&self);
    /// Seconds. Reported through the next state update, not immediately.
    fn seek(&self, position: f64);
    /// Clamped to 0–1.
    fn set_volume(&self, value: f64);
    /// An empty id means the system default.
    fn set_output_device(&self, device_id: &str);
    /// Asks for a fresh device list; [`devices`](Self::devices) returns it once known.
    fn request_devices(&self);
    fn devices(&self) -> Vec<AudioDevice>;
    fn state(&self) -> PlaybackState;
    /// Where state changes go: the realtime progress broadcast.
    fn on_state_change(&self, listener: StateListener);
}

/// A player that keeps state and makes no sound.
pub struct SilentPlayer {
    state: Mutex<PlaybackState>,
    listener: RwLock<Option<StateListener>>,
}

impl Default for SilentPlayer {
    fn default() -> Self {
        SilentPlayer {
            state: Mutex::new(PlaybackState {
                track_id: None,
                playing: false,
                position: 0.0,
                duration: 0.0,
                volume: 1.0,
            }),
            listener: RwLock::new(None),
        }
    }
}

impl SilentPlayer {
    pub fn new() -> Self {
        SilentPlayer::default()
    }

    /// Applies a change and reports the new state, as player.ts's `emitState` did.
    fn change(&self, apply: impl FnOnce(&mut PlaybackState)) {
        let state = {
            let mut state = self.state.lock().expect("player state lock poisoned");
            apply(&mut state);
            state.clone()
        };
        if let Some(listener) = self
            .listener
            .read()
            .expect("listener lock poisoned")
            .as_ref()
        {
            listener(&state);
        }
    }
}

impl Player for SilentPlayer {
    fn load(&self, track_id: i64, autoplay: bool) {
        self.change(|s| {
            s.track_id = Some(track_id);
            s.position = 0.0;
            s.duration = 0.0;
            s.playing = autoplay;
        });
    }

    fn play(&self) {
        self.change(|s| s.playing = true);
    }

    fn pause(&self) {
        self.change(|s| s.playing = false);
    }

    fn seek(&self, _position: f64) {}

    fn set_volume(&self, value: f64) {
        self.change(|s| s.volume = value.clamp(0.0, 1.0));
    }

    fn set_output_device(&self, _device_id: &str) {}

    fn request_devices(&self) {}

    fn devices(&self) -> Vec<AudioDevice> {
        Vec::new()
    }

    fn state(&self) -> PlaybackState {
        self.state
            .lock()
            .expect("player state lock poisoned")
            .clone()
    }

    fn on_state_change(&self, listener: StateListener) {
        *self.listener.write().expect("listener lock poisoned") = Some(listener);
    }
}
