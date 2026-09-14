//! The Kyylan Jukebox program: the server, playing through this machine's audio output.
//!
//! On Windows and macOS it lives in the tray, started at log on by Task Scheduler or launchd.
//! On Linux it's a system service with no local presence at all. See `--help`.

// No console window at log on. Run from a terminal, it attaches to that one instead.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod check;
#[cfg(any(windows, target_os = "macos"))]
mod desktop;
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod import;
mod instance;
mod logging;
#[cfg(windows)]
mod session;
mod shutdown;
#[cfg(target_os = "macos")]
mod uninstall;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::Parser;
use jukebox_audio::output::Backend;
use jukebox_audio::{AudioPlayer, CpalBackend, VirtualBackend, DEFAULT_DEVICE};
use jukebox_core::config::ConfigStore;
use jukebox_core::library::query::track_path;
use jukebox_core::paths::{DataDir, DATA_DIR_ENV};
use jukebox_core::types::AudioDevice;
use jukebox_server::net::{FolderPicker, SystemNetwork};
use jukebox_server::{serve_until, App, Options, SetupAccess};
use rusqlite::{Connection, OpenFlags};

use crate::check::Setup;
use crate::logging::{Destination, Level};
use crate::shutdown::Shutdown;

/// Plays through virtual devices instead of the machine's own, for tests: `virtual`.
const AUDIO_ENV: &str = "KYYLAN_AUDIO";

#[derive(Parser)]
#[command(
    name = "kyylan-jukebox",
    version,
    about = "Kyylan Jukebox: plays local music through this machine's speakers while guests \
             queue songs from their browsers."
)]
struct Cli {
    /// Where config.json and jukebox.db live [env: KYYLAN_DATA_DIR]. Logs go in its logs
    /// folder too, when given.
    #[arg(long, value_name = "PATH", global = true)]
    data_dir: Option<PathBuf>,

    /// Print the output devices, to copy an id or name into outputDeviceId, then exit
    #[arg(long, conflicts_with = "check_config")]
    list_devices: bool,

    /// Check config.json and exit, non-zero if the jukebox wouldn't start with it
    #[arg(long)]
    check_config: bool,

    /// How much to log
    #[arg(long, value_enum, value_name = "LEVEL", default_value_t = Level::Info, global = true)]
    log_level: Level,

    #[cfg(unix)]
    #[command(subcommand)]
    command: Option<Command>,
}

#[cfg(unix)]
#[derive(clap::Subcommand)]
enum Command {
    /// Bring a v0.2.x data directory, such as ~/.config/kyylan-jukebox, into the service's
    /// /var/lib/kyylan-jukebox (run with sudo)
    #[cfg(target_os = "linux")]
    Import {
        #[arg(value_name = "DIR")]
        dir: PathBuf,
    },
    /// Remove the LaunchAgent and the app, keeping everyone's data (run with sudo)
    #[cfg(target_os = "macos")]
    Uninstall,
}

/// Tells the person running the jukebox something went wrong at start-up: a dialog on a
/// desktop, and the log everywhere.
fn fatal(message: &str) -> ExitCode {
    tracing::error!("{message}");
    #[cfg(any(windows, target_os = "macos"))]
    if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        desktop::error_dialog(message);
    }
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    #[cfg(windows)]
    attach_console();
    let cli = Cli::parse();

    #[cfg(unix)]
    if let Some(command) = cli.command {
        logging::init(cli.log_level, Destination::Terminal);
        return run_command(command, cli.data_dir);
    }
    if cli.list_devices {
        logging::init(cli.log_level, Destination::Terminal);
        return list_devices();
    }

    let overridden =
        cli.data_dir.is_some() || std::env::var_os(DATA_DIR_ENV).is_some_and(|dir| !dir.is_empty());
    let Some(dir) = cli.data_dir.map(DataDir::at).or_else(DataDir::resolve) else {
        logging::init(cli.log_level, Destination::Terminal);
        return fatal("No data directory on this platform; pass --data-dir.");
    };
    if cli.check_config {
        logging::init(cli.log_level, Destination::Terminal);
        return check_config(&dir);
    }
    logging::init(
        cli.log_level,
        Destination::Service {
            folder: logging::folder(dir.root(), overridden),
        },
    );
    run(dir)
}

/// A program built for the Windows GUI subsystem starts with no console. Run from a terminal,
/// it borrows that terminal's, so `--help` and the rest print where they were asked for. Output
/// redirected to a file or pipe is left as it is.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
    };
    unsafe {
        if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(unix)]
fn run_command(command: Command, data_dir: Option<PathBuf>) -> ExitCode {
    let result = match command {
        #[cfg(target_os = "linux")]
        Command::Import { dir } => {
            let destination = data_dir.unwrap_or_else(|| PathBuf::from(import::SERVICE_DATA_DIR));
            // Another data directory is someone's own jukebox, not the service's.
            let service: &dyn import::Service =
                if destination == std::path::Path::new(import::SERVICE_DATA_DIR) {
                    &import::Systemd
                } else {
                    &NoService
                };
            import::run(&dir, &destination, service, &mut std::io::stdout())
        }
        #[cfg(target_os = "macos")]
        Command::Uninstall => {
            let _ = data_dir;
            if !uninstall::is_root() {
                Err("run it with sudo: sudo kyylan-jukebox uninstall".into())
            } else {
                uninstall::run(
                    &uninstall::Layout::default(),
                    &uninstall::session_users(),
                    &uninstall::Launchctl,
                    &mut std::io::stdout(),
                )
            }
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// A jukebox run by hand: nothing to stop or start.
#[cfg(target_os = "linux")]
struct NoService;

#[cfg(target_os = "linux")]
impl import::Service for NoService {
    fn stop(&self) -> Result<bool, String> {
        Ok(false)
    }
    fn start(&self) -> Result<(), String> {
        Ok(())
    }
    fn can_read(&self, folder: &std::path::Path, _: u32, _: u32) -> Result<(), String> {
        std::fs::read_dir(folder)
            .map(drop)
            .map_err(|e| e.to_string())
    }
}

fn virtual_audio() -> bool {
    std::env::var(AUDIO_ENV).is_ok_and(|v| v == "virtual")
}

fn virtual_backend() -> VirtualBackend {
    let backend = VirtualBackend::new(1.0);
    backend.add_device("virtual", "Virtual output", 48_000, 2);
    backend
}

/// The devices as the admin panel lists them: the default first.
fn devices(backend: &dyn Backend) -> Vec<AudioDevice> {
    let mut list = vec![AudioDevice {
        device_id: DEFAULT_DEVICE.into(),
        label: match backend.default_name() {
            Some(name) => format!("Default - {name}"),
            None => "Default".into(),
        },
    }];
    list.extend(backend.devices());
    list
}

fn system_devices() -> Vec<AudioDevice> {
    if virtual_audio() {
        devices(&virtual_backend())
    } else {
        devices(&CpalBackend::default())
    }
}

fn list_devices() -> ExitCode {
    let list = system_devices();
    let width = list.iter().map(|d| d.device_id.len()).max().unwrap_or(0);
    println!("{:width$}  NAME", "ID");
    for device in list {
        println!("{:width$}  {}", device.device_id, device.label);
    }
    ExitCode::SUCCESS
}

fn check_config(dir: &DataDir) -> ExitCode {
    let path = dir.config_path();
    let findings = check::check(&path, Setup::PLATFORM, Some(&system_devices()));
    for finding in &findings {
        println!("{finding}");
    }
    let errors = findings
        .iter()
        .filter(|f| f.severity == check::Severity::Error)
        .count();
    match errors {
        0 => {
            println!("{} is valid", path.display());
            ExitCode::SUCCESS
        }
        n => {
            println!(
                "{} has {n} error{}",
                path.display(),
                if n == 1 { "" } else { "s" }
            );
            ExitCode::FAILURE
        }
    }
}

fn run(dir: DataDir) -> ExitCode {
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
    if let Err(err) = std::fs::create_dir_all(dir.root()) {
        return fatal(&format!("Can't create {}: {err}", dir.root().display()));
    }
    let config = match ConfigStore::open(dir.config_path()) {
        Ok(config) => Arc::new(config),
        Err(err) => return fatal(&format!("Can't read the settings: {err}")),
    };
    if let Some(reason) = check::refuses_to_start(&config.get(), config.path(), Setup::PLATFORM) {
        return fatal(&reason);
    }
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
    let signals = {
        let _entered = runtime.enter();
        shutdown::Signals::listen()
    };
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
    let device = config.get().output_device_id;
    let player = Arc::new(if virtual_audio() {
        AudioPlayer::new(virtual_backend(), resolver, device)
    } else {
        AudioPlayer::new(CpalBackend::default(), resolver, device)
    });

    #[cfg(any(windows, target_os = "macos"))]
    let folder_picker: Arc<dyn FolderPicker> = Arc::new(desktop::DialogFolderPicker);
    #[cfg(not(any(windows, target_os = "macos")))]
    let folder_picker: Arc<dyn FolderPicker> = Arc::new(jukebox_server::net::NoFolderPicker);

    let network = Arc::new(SystemNetwork::default());
    let first_run = !config.get().configured;
    let library_paths = config.get().library_paths;
    let app = match App::new(Options {
        config,
        database,
        player,
        network: network.clone(),
        folder_picker,
        running_port: port,
        version: env!("CARGO_PKG_VERSION").into(),
        setup: match Setup::PLATFORM {
            // With no window of its own, the page is the only way to set up — and the first
            // guest to open it mustn't be the one who chooses the admin password.
            Setup::InBrowser => SetupAccess::HostOnly,
            // The service's config file was set up at install.
            Setup::InFile => SetupAccess::Disabled,
        },
    }) {
        Ok(app) => Arc::new(app),
        Err(err) => return fatal(&format!("Can't open the database: {err}")),
    };
    if let Err(err) = app.start_playback() {
        tracing::error!(%err, "starting playback failed");
    }
    // Nothing from a folder that can't be read would play, and the scan alone doesn't say so.
    for folder in &library_paths {
        if let Err(err) = std::fs::read_dir(folder) {
            tracing::warn!(folder, %err, "a library folder can't be read");
        }
    }
    // Picks up what changed while the jukebox wasn't running. Unchanged files are skipped.
    app.start_scan();
    tracing::info!("serving {} on port {port}", dir.root().display());

    let (shutdown, serving) = Shutdown::new(app.clone());
    let server_app = app.clone();
    let server = runtime.spawn(async move {
        let stop = async move {
            let _ = serving.stop.await;
        };
        let result = serve_until(&server_app, listener, stop).await;
        let _ = serving.done.send(());
        result
    });
    runtime.spawn(shutdown::on_signals(signals, shutdown.clone()));
    #[cfg(windows)]
    {
        let shutdown = shutdown.clone();
        session::watch(move |reason| shutdown.stop(reason));
    }

    #[cfg(any(windows, target_os = "macos"))]
    {
        // The server runs on the runtime's threads until the process ends.
        drop(server);
        let _runtime = runtime;
        desktop::run(desktop::Desktop {
            port,
            network,
            first_run,
            on_quit: Box::new(move |reason| shutdown.stop(reason)),
        })
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (network, first_run);
        let served = runtime.block_on(server);
        // Serving ends only once stopping has begun, or on an error: either way, finish
        // stopping before the process ends.
        shutdown.stop("the server stopped");
        let failed = match served {
            Ok(Ok(())) => false,
            Ok(Err(err)) => {
                fatal(&format!("The server stopped: {err}"));
                true
            }
            Err(err) => {
                fatal(&format!("The server stopped: {err}"));
                true
            }
        };
        // Straight out: dropping the runtime would wait on requests that are now waiting,
        // forever, for the database stopping closed.
        std::process::exit(i32::from(failed))
    }
}
