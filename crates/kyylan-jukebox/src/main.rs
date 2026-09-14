//! The Kyylan Jukebox program: the server, playing through this machine's audio output.
//!
//!   kyylan-jukebox [--data-dir <path>]
//!
//! For now this is the whole of it. The command line, logging and start-up as a service
//! come in phase 7, and the tray in phase 6.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use jukebox_audio::{AudioPlayer, CpalBackend};
use jukebox_core::config::ConfigStore;
use jukebox_core::library::query::track_path;
use jukebox_core::paths::DataDir;
use jukebox_server::net::{NoFolderPicker, SystemNetwork};
use jukebox_server::{serve, App, Options};
use rusqlite::{Connection, OpenFlags};

#[tokio::main]
async fn main() -> ExitCode {
    // Symphonia narrates every file it opens, and logs as errors what the player already
    // reports as a song that couldn't be played.
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::Level::INFO)
        .with_target("symphonia", tracing_subscriber::filter::LevelFilter::OFF);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(filter)
        .init();

    let mut args = std::env::args().skip(1);
    let dir = match (args.next().as_deref(), args.next()) {
        (Some("--data-dir"), Some(path)) => DataDir::at(path),
        (None, _) => match DataDir::resolve() {
            Some(dir) => dir,
            None => {
                eprintln!("no data directory on this platform; pass --data-dir");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: kyylan-jukebox [--data-dir <path>]");
            return ExitCode::from(2);
        }
    };
    if let Err(err) = std::fs::create_dir_all(dir.root()) {
        eprintln!("can't create {}: {err}", dir.root().display());
        return ExitCode::FAILURE;
    }
    let config = match ConfigStore::open(dir.config_path()) {
        Ok(config) => Arc::new(config),
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let port = config.get().port;
    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("Port {port} is already in use ({err}). Change the port in the settings and restart.");
            return ExitCode::FAILURE;
        }
    };

    // The player finds files through a connection of its own, opened once the server has
    // created the database.
    let database = dir.database_path();
    let reads: Mutex<Option<Connection>> = Mutex::new(None);
    let resolver_database: PathBuf = database.clone();
    let resolver = Arc::new(move |id: i64| {
        let mut reads = reads.lock().ok()?;
        if reads.is_none() {
            *reads = Connection::open_with_flags(
                &resolver_database,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok();
        }
        track_path(reads.as_ref()?, id)
            .ok()
            .flatten()
            .map(PathBuf::from)
    });
    let player = Arc::new(AudioPlayer::new(
        CpalBackend::default(),
        resolver,
        config.get().output_device_id,
    ));

    let network = Arc::new(SystemNetwork::default());
    let app = match App::new(Options {
        config,
        database,
        player,
        network: network.clone(),
        folder_picker: Arc::new(NoFolderPicker),
        running_port: port,
        version: env!("CARGO_PKG_VERSION").into(),
    }) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("can't open the database: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = app.start_playback() {
        tracing::error!(%err, "starting playback failed");
    }
    tracing::info!("serving {} on port {port}", dir.root().display());
    tokio::select! {
        result = serve(&app, listener) => {
            if let Err(err) = result {
                eprintln!("the server stopped: {err}");
                return ExitCode::FAILURE;
            }
        }
        _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
    }
    ExitCode::SUCCESS
}
