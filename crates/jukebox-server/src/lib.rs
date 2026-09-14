//! Kyylan Jukebox's server: the HTTP API under `/api`, live updates on `/ws`, and the web UI
//! for everything else. Ports src/main/server.ts, api.ts, auth.ts and realtime.ts, and answers
//! every request the way the Electron build's Express server did — tests/api_electron.rs
//! holds it to responses recorded from that server.

// A handler's early exits are finished responses — a 401, a 400 with its message — returned
// as the `Err` of a `Result` so `?` can send them. Boxing them would buy nothing.
#![allow(clippy::result_large_err)]

pub mod auth;
pub mod http;
pub mod net;
pub mod realtime;
mod routes;
pub mod ui;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use jukebox_core::config::ConfigStore;
use jukebox_core::db;
use jukebox_core::engine::Engine;
use jukebox_core::library::Scanner;
use jukebox_core::player::Player;
use rusqlite::{Connection, OpenFlags};
use tokio::net::TcpListener;

use crate::auth::Sessions;
use crate::net::{FolderPicker, Network};
use crate::realtime::Hub;

/// What the server is built from. The platform-facing parts are injected so the parity
/// harness can fix them and each build can supply its own.
pub struct Options {
    pub config: Arc<ConfigStore>,
    pub database: PathBuf,
    pub player: Arc<dyn Player>,
    pub network: Arc<dyn Network>,
    pub folder_picker: Arc<dyn FolderPicker>,
    /// The port actually listened on, which setup and settings compare a new port with.
    pub running_port: u16,
    pub version: String,
}

pub(crate) struct AppState {
    config: Arc<ConfigStore>,
    engine: Arc<Engine>,
    player: Arc<dyn Player>,
    scanner: Arc<Scanner>,
    reads: ReadPool,
    sessions: Sessions,
    hub: Arc<Hub>,
    network: Arc<dyn Network>,
    folder_picker: Arc<dyn FolderPicker>,
    running_port: u16,
    version: String,
}

pub struct App {
    state: Arc<AppState>,
}

impl App {
    /// Opens and migrates the database, clears what a previous run left mid-play, and wires
    /// the engine, player and live updates together. Nothing starts playing until
    /// [`App::start_playback`].
    pub fn new(options: Options) -> Result<App, db::DbError> {
        let (conn, _report) = db::open(&options.database)?;
        let engine = Engine::new(options.config.clone(), options.player.clone(), conn);
        engine.init_queue().map_err(|e| match e {
            jukebox_core::engine::EngineError::Database(e) => db::DbError::Sqlite(e),
            other => unreachable!("clearing the queue rejects nothing: {other}"),
        })?;

        let hub = Hub::new();
        hub.attach(&engine);
        let progress = Arc::downgrade(&hub);
        options.player.on_state_change(Arc::new(move |state| {
            if let Some(hub) = progress.upgrade() {
                hub.push_progress(state);
            }
        }));
        let queue = Arc::downgrade(&hub);
        engine.on_queue_change(Arc::new(move || {
            if let Some(hub) = queue.upgrade() {
                hub.broadcast_queue();
            }
        }));

        Ok(App {
            state: Arc::new(AppState {
                config: options.config,
                engine,
                player: options.player,
                scanner: Arc::new(Scanner::new()),
                reads: ReadPool::new(options.database),
                sessions: Sessions::default(),
                hub,
                network: options.network,
                folder_picker: options.folder_picker,
                running_port: options.running_port,
                version: options.version,
            }),
        })
    }

    /// Starts playing whatever is queued — what Electron did once its player window loaded.
    pub fn start_playback(&self) -> Result<(), jukebox_core::engine::EngineError> {
        self.state.engine.maybe_start()
    }

    pub fn router(&self) -> Router {
        routes::router(self.state.clone())
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.state.engine
    }

    pub fn scanner(&self) -> &Arc<Scanner> {
        &self.state.scanner
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.state.hub
    }
}

/// Serves the app on a bound listener until the task is dropped.
pub async fn serve(app: &App, listener: TcpListener) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.router()
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

/// Read-only connections for browsing and search, so they never wait behind the engine.
pub(crate) struct ReadPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl ReadPool {
    fn new(path: PathBuf) -> Self {
        ReadPool {
            path,
            idle: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with<R>(
        &self,
        read: impl FnOnce(&Connection) -> rusqlite::Result<R>,
    ) -> rusqlite::Result<R> {
        let pooled = self.idle.lock().expect("pool lock poisoned").pop();
        let conn = match pooled {
            Some(conn) => conn,
            None => Connection::open_with_flags(
                &self.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?,
        };
        let result = read(&conn);
        self.idle.lock().expect("pool lock poisoned").push(conn);
        result
    }
}
