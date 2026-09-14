//! The jukebox engine: the queue, standby playlist and random fill, cooldowns, rate limit,
//! bans, downvotes, and the history and stats they leave behind. Ports src/main/queue.ts,
//! standby.ts, bans.ts and stats.ts.
//!
//! Node ran each of those functions to completion before starting another, so their
//! check-then-act sequences — the per-guest limit check and the insert after it, the
//! downvote count and the skip it triggers — could never interleave. Here every operation
//! takes the engine's lock for its whole run, which keeps that guarantee under a
//! multi-threaded server. The lock covers the queue's in-memory state and the database
//! connection the engine writes through; the library scanner shares that connection batch by
//! batch, and browsing uses connections of its own.

mod bans;
mod queue;
mod standby;
mod stats;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use rusqlite::Connection;

use crate::config::ConfigStore;
use crate::player::{LoadId, Player, PlayerEvent};
use crate::types::{BanEntry, QueueState, StandbyEntry, StatsResponse};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A request the rules turn down, with the HTTP status and message a guest sees.
    #[error("{message}")]
    Rejected { status: u16, message: String },
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
}

impl EngineError {
    fn rejected(status: u16, message: impl Into<String>) -> Self {
        EngineError::Rejected {
            status,
            message: message.into(),
        }
    }
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

/// Called after an operation changes what guests see in the queue.
pub type QueueListener = Arc<dyn Fn() + Send + Sync>;

/// Queue state that lives only in memory, as it did in queue.ts.
#[derive(Default)]
struct Runtime {
    /// The playing entry the current downvotes are for, and who cast them.
    downvote_entry: Option<i64>,
    downvoters: HashSet<String>,
    /// Position in the standby playlist when it plays in order, starting before the first.
    standby_cursor: Option<usize>,
    /// The last standby track, so shuffle doesn't repeat it straight away.
    last_standby: Option<i64>,
    /// The song the engine last sent to the player.
    playing: Option<Playing>,
    /// Songs in a row the player couldn't play.
    failures: u32,
    /// Why playback stopped by itself, shown to everyone until something plays again.
    problem: Option<String>,
}

/// A song handed to the player, and what's known about how it went.
struct Playing {
    load: LoadId,
    /// Its play_history row, written when it was sent: removed again if it never plays.
    history_id: i64,
    title: String,
    started: bool,
}

/// After this many songs in a row fail, the engine stops rather than trying the rest of the
/// queue — or the whole library, with random fill on and the music drive unplugged.
pub const MAX_FAILURES_IN_A_ROW: u32 = 5;

pub struct Engine {
    config: Arc<ConfigStore>,
    player: Arc<dyn Player>,
    runtime: Mutex<Runtime>,
    db: Mutex<Connection>,
    listener: RwLock<Option<QueueListener>>,
}

/// Everything an operation works with while it holds the engine.
struct Ctx<'a> {
    db: &'a Connection,
    rt: &'a mut Runtime,
    config: &'a ConfigStore,
    player: &'a dyn Player,
    /// Set by `broadcast`: guests' view of the queue changed.
    changed: bool,
}

impl Ctx<'_> {
    /// queue.ts's `broadcastQueue()`: sent once the operation has released the engine.
    fn broadcast(&mut self) {
        self.changed = true;
    }
}

impl Engine {
    pub fn new(config: Arc<ConfigStore>, player: Arc<dyn Player>, db: Connection) -> Arc<Self> {
        Arc::new(Engine {
            config,
            player,
            runtime: Mutex::new(Runtime::default()),
            db: Mutex::new(db),
            listener: RwLock::new(None),
        })
    }

    pub fn on_queue_change(&self, listener: QueueListener) {
        *self.listener.write().expect("listener lock poisoned") = Some(listener);
    }

    /// The engine's database connection, for the library scanner to write through.
    pub fn db(&self) -> &Mutex<Connection> {
        &self.db
    }

    pub fn player(&self) -> &Arc<dyn Player> {
        &self.player
    }

    fn run<R>(&self, op: impl FnOnce(&mut Ctx<'_>) -> R) -> R {
        let (result, changed) = {
            let mut rt = self.runtime.lock().expect("engine lock poisoned");
            let db: MutexGuard<'_, Connection> = self.db.lock().expect("database lock poisoned");
            let mut ctx = Ctx {
                db: &db,
                rt: &mut rt,
                config: &self.config,
                player: self.player.as_ref(),
                changed: false,
            };
            let result = op(&mut ctx);
            (result, ctx.changed)
        };
        if changed {
            let listener = self
                .listener
                .read()
                .expect("listener lock poisoned")
                .clone();
            if let Some(listener) = listener {
                listener();
            }
        }
        result
    }

    // ---- Queue ---------------------------------------------------------------------

    /// Clears what a previous run left mid-play: filler rows go, and a guest's song that
    /// was playing waits at the front of the queue again.
    pub fn init_queue(&self) -> Result<()> {
        self.run(queue::init)
    }

    /// The queue as one client sees it: which entries are theirs, and their own vote.
    pub fn queue_state(&self, ip: &str) -> Result<QueueState> {
        self.run(|ctx| queue::state(ctx, ip))
    }

    /// Plays the next song: guests' first, then the standby playlist, then random fill.
    pub fn advance(&self) -> Result<()> {
        self.run(queue::advance)
    }

    /// The player finished the current song.
    pub fn track_ended(&self) -> Result<()> {
        self.advance()
    }

    /// Acts on what the player reports: an ended song advances the queue, a song that
    /// couldn't be played is skipped — and logged as no play — and a song that started
    /// clears any earlier failures. Events about a load that has been replaced are ignored,
    /// so a song ending just as it's skipped doesn't skip the next one too.
    pub fn handle_player_event(&self, event: PlayerEvent) -> Result<()> {
        self.run(|ctx| queue::player_event(ctx, event))
    }

    /// Admin: skip the current song.
    pub fn skip(&self) -> Result<()> {
        self.advance()
    }

    /// Starts playing if nothing is.
    pub fn maybe_start(&self) -> Result<()> {
        self.run(queue::maybe_start)
    }

    /// A guest adds a song. `name` is their device's hostname.
    pub fn enqueue(&self, track_id: i64, ip: &str, name: Option<&str>) -> Result<()> {
        self.run(|ctx| queue::enqueue(ctx, track_id, ip, name))
    }

    /// `entry_id` is `Number(req.params.id)`, which can be fractional or `NaN`.
    pub fn remove_entry(&self, entry_id: f64, ip: &str, is_admin: bool) -> Result<()> {
        self.run(|ctx| queue::remove_entry(ctx, entry_id, ip, is_admin))
    }

    /// Admin: move a pending entry to a position in the pending list. `to_index` is an
    /// integer.
    pub fn reorder(&self, entry_id: f64, to_index: f64) -> Result<()> {
        self.run(|ctx| queue::reorder(ctx, entry_id, to_index))
    }

    pub fn downvote(&self, ip: &str, name: Option<&str>) -> Result<()> {
        self.run(|ctx| queue::downvote(ctx, ip, name))
    }

    /// Admin: empty the queue, leaving the current song playing.
    pub fn clear_pending(&self) -> Result<()> {
        self.run(queue::clear_pending)
    }

    // ---- Standby playlist ------------------------------------------------------------

    pub fn standby_entries(&self) -> Result<Vec<StandbyEntry>> {
        self.run(|ctx| standby::list(ctx.db))
    }

    pub fn add_standby(&self, track_id: i64) -> Result<()> {
        self.run(|ctx| standby::add(ctx.db, track_id))
    }

    pub fn remove_standby(&self, id: f64) -> Result<()> {
        self.run(|ctx| standby::remove(ctx.db, id))
    }

    pub fn clear_standby(&self) -> Result<()> {
        self.run(|ctx| standby::clear(ctx.db))
    }

    // ---- Bans ------------------------------------------------------------------------

    pub fn bans(&self) -> Result<Vec<BanEntry>> {
        self.run(|ctx| bans::list(ctx.db))
    }

    /// Bans an address, for `minutes` or, with 0, permanently. Returns every ban.
    pub fn ban(&self, ip: &str, name: Option<&str>, minutes: i64) -> Result<Vec<BanEntry>> {
        self.run(|ctx| bans::ban(ctx.db, ip, name, minutes))
    }

    pub fn unban(&self, ip: &str) -> Result<Vec<BanEntry>> {
        self.run(|ctx| bans::unban(ctx.db, ip))
    }

    // ---- History and stats -----------------------------------------------------------

    /// Guests get the guest list without addresses or bans.
    pub fn stats(
        &self,
        history_limit: i64,
        top_limit: i64,
        for_admin: bool,
    ) -> Result<StatsResponse> {
        self.run(|ctx| stats::build(ctx.db, history_limit, top_limit, for_admin))
    }

    /// Wipes the logs, which also resets the repeat cooldowns.
    pub fn clear_stats(&self) -> Result<()> {
        self.run(|ctx| stats::clear(ctx.db))
    }
}
