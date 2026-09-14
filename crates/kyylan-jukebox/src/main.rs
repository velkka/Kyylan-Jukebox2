//! The Kyylan Jukebox program: the server, playing through this machine's audio output.
//!
//!   kyylan-jukebox [--data-dir <path>]
//!
//! On Windows and macOS it lives in the tray. On Linux it has no local presence at all. The
//! command line, logging and running as a service grow in phase 7.

#[cfg(any(windows, target_os = "macos"))]
mod desktop;
mod instance;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use jukebox_audio::{AudioPlayer, CpalBackend};
use jukebox_core::config::ConfigStore;
use jukebox_core::library::query::track_path;
use jukebox_core::paths::DataDir;
use jukebox_server::net::{FolderPicker, SystemNetwork};
use jukebox_server::{serve, App, Options, SetupAccess};
use rusqlite::{Connection, OpenFlags};

/// Tells the person running the jukebox something went wrong at start-up: a dialog on a
/// desktop, and the log everywhere.
fn fatal(message: &str) -> ExitCode {
    tracing::error!("{message}");
    #[cfg(any(windows, target_os = "macos"))]
    desktop::error_dialog(message);
    ExitCode::FAILURE
}

fn main() -> ExitCode {
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
            None => return fatal("No data directory on this platform; pass --data-dir."),
        },
        _ => {
            eprintln!("usage: kyylan-jukebox [--data-dir <path>]");
            return ExitCode::from(2);
        }
    };
    if let Err(err) = std::fs::create_dir_all(dir.root()) {
        return fatal(&format!("Can't create {}: {err}", dir.root().display()));
    }
    let config = match ConfigStore::open(dir.config_path()) {
        Ok(config) => Arc::new(config),
        Err(err) => return fatal(&format!("Can't read the settings: {err}")),
    };
    let port = config.get().port;

    // A second copy for the same data would only find the port taken: point the person who
    // started it at the one that's running instead.
    let _instance = match instance::acquire(dir.root()) {
        Ok(lock) => lock,
        Err(instance::AcquireError::Held) => {
            tracing::info!("Kyylan Jukebox is already running");
            #[cfg(any(windows, target_os = "macos"))]
            desktop::open_console(port);
            return ExitCode::SUCCESS;
        }
        Err(instance::AcquireError::Io(err)) => {
            return fatal(&format!("Can't start: {err}"));
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("server")
        .build()
        .expect("starting the async runtime");
    let listener = match runtime.block_on(tokio::net::TcpListener::bind(("0.0.0.0", port))) {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            return fatal(&format!(
                "Port {port} is already in use. Change the port in the app settings and restart."
            ));
        }
        Err(err) => return fatal(&format!("Failed to start the jukebox server: {err}")),
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

    #[cfg(any(windows, target_os = "macos"))]
    let folder_picker: Arc<dyn FolderPicker> = Arc::new(desktop::DialogFolderPicker);
    #[cfg(not(any(windows, target_os = "macos")))]
    let folder_picker: Arc<dyn FolderPicker> = Arc::new(jukebox_server::net::NoFolderPicker);

    let network = Arc::new(SystemNetwork::default());
    let first_run = !config.get().configured;
    let app = match App::new(Options {
        config,
        database,
        player,
        network: network.clone(),
        folder_picker,
        running_port: port,
        version: env!("CARGO_PKG_VERSION").into(),
        // With no window of its own, the page is the only way to set up — and the first
        // guest to open it mustn't be the one who chooses the admin password.
        setup: SetupAccess::HostOnly,
    }) {
        Ok(app) => Arc::new(app),
        Err(err) => return fatal(&format!("Can't open the database: {err}")),
    };
    if let Err(err) = app.start_playback() {
        tracing::error!(%err, "starting playback failed");
    }
    tracing::info!("serving {} on port {port}", dir.root().display());
    let serving = app.clone();
    let server = runtime.spawn(async move { serve(&serving, listener).await });

    #[cfg(any(windows, target_os = "macos"))]
    {
        // The server runs on the runtime's threads until the process ends.
        drop(server);
        desktop::run(desktop::Desktop {
            port,
            network,
            first_run,
            on_quit: Box::new(move || {
                tracing::info!("quitting");
                runtime.shutdown_background();
                drop(app);
            }),
        })
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (network, first_run);
        runtime.block_on(async {
            tokio::select! {
                result = server => match result {
                    Ok(Err(err)) => return fatal(&format!("The server stopped: {err}")),
                    Err(err) => return fatal(&format!("The server stopped: {err}")),
                    Ok(Ok(())) => {}
                },
                _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
            }
            ExitCode::SUCCESS
        })
    }
}
