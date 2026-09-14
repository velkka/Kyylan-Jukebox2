//! Live updates over `/ws`. Mirrors src/main/realtime.ts.
//!
//! Each client gets the whole queue on connecting and again whenever it changes, built for
//! it — `mine` and "downvoted by me" depend on who's asking. Playback progress is the same
//! for everyone: sent at once when a song starts, stops or changes, and otherwise at most
//! about once a second.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use jukebox_core::engine::Engine;
use jukebox_core::types::{PlaybackState, ProgressPayload, RealtimeMessage};
use tokio::sync::mpsc;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(900);

struct Subscriber {
    id: u64,
    ip: String,
    tx: mpsc::UnboundedSender<String>,
}

#[derive(Default)]
struct Throttle {
    last_sent: Option<Instant>,
    playing: bool,
    track_id: Option<i64>,
}

#[derive(Default)]
pub struct Hub {
    engine: OnceLock<Weak<Engine>>,
    subscribers: Mutex<Vec<Subscriber>>,
    next_id: AtomicU64,
    /// Bumped by every queue broadcast, so a client joining mid-broadcast can tell it may
    /// have missed one.
    broadcasts: AtomicU64,
    throttle: Mutex<Throttle>,
}

/// One client's subscription. Dropping it unsubscribes.
pub struct Subscription {
    hub: Arc<Hub>,
    id: u64,
    pub messages: mpsc::UnboundedReceiver<String>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub
            .subscribers
            .lock()
            .expect("hub lock poisoned")
            .retain(|s| s.id != self.id);
    }
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Hub::default())
    }

    pub fn attach(&self, engine: &Arc<Engine>) {
        let _ = self.engine.set(Arc::downgrade(engine));
    }

    fn queue_message(&self, ip: &str) -> Option<String> {
        let engine = self.engine.get()?.upgrade()?;
        match engine.queue_state(ip) {
            Ok(state) => serde_json::to_string(&RealtimeMessage::Queue(Box::new(state))).ok(),
            Err(err) => {
                tracing::warn!(%err, "couldn't build the queue for a realtime update");
                None
            }
        }
    }

    /// Adds a client and sends it the queue as it stands.
    pub fn subscribe(self: &Arc<Self>, ip: &str) -> Subscription {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let before = self.broadcasts.load(Ordering::SeqCst);
        if let Some(message) = self.queue_message(ip) {
            let _ = tx.send(message);
        }
        self.subscribers
            .lock()
            .expect("hub lock poisoned")
            .push(Subscriber {
                id,
                ip: ip.to_string(),
                tx: tx.clone(),
            });
        // The queue can't be built while holding the subscriber list — the engine may be
        // sending progress through it — so a broadcast can slip between building this
        // client's first view and adding the client. If one did, send a fresh view.
        if self.broadcasts.load(Ordering::SeqCst) != before {
            if let Some(message) = self.queue_message(ip) {
                let _ = tx.send(message);
            }
        }
        Subscription {
            hub: Arc::clone(self),
            id,
            messages: rx,
        }
    }

    /// Sends every client its own view of the queue.
    pub fn broadcast_queue(&self) {
        self.broadcasts.fetch_add(1, Ordering::SeqCst);
        let clients: Vec<(String, mpsc::UnboundedSender<String>)> = self
            .subscribers
            .lock()
            .expect("hub lock poisoned")
            .iter()
            .map(|s| (s.ip.clone(), s.tx.clone()))
            .collect();
        for (ip, tx) in clients {
            if let Some(message) = self.queue_message(&ip) {
                let _ = tx.send(message);
            }
        }
    }

    /// Sends playback progress to everyone, throttled.
    pub fn push_progress(&self, state: &PlaybackState) {
        {
            let mut throttle = self.throttle.lock().expect("hub lock poisoned");
            let changed = state.playing != throttle.playing || state.track_id != throttle.track_id;
            let recent = throttle
                .last_sent
                .is_some_and(|at| at.elapsed() < PROGRESS_INTERVAL);
            if !changed && recent {
                return;
            }
            throttle.last_sent = Some(Instant::now());
            throttle.playing = state.playing;
            throttle.track_id = state.track_id;
        }
        let message = RealtimeMessage::Progress(ProgressPayload {
            position: state.position,
            duration: state.duration,
            playing: state.playing,
            track_id: state.track_id,
        });
        let Ok(message) = serde_json::to_string(&message) else {
            return;
        };
        for subscriber in self.subscribers.lock().expect("hub lock poisoned").iter() {
            let _ = subscriber.tx.send(message.clone());
        }
    }
}

/// Runs one WebSocket connection until the client goes away. Anything the client sends is
/// ignored, as it was.
pub async fn serve_socket(hub: Arc<Hub>, ip: String, mut socket: WebSocket) {
    let mut subscription = hub.subscribe(&ip);
    loop {
        tokio::select! {
            outgoing = subscription.messages.recv() => {
                let Some(text) = outgoing else { break };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}
