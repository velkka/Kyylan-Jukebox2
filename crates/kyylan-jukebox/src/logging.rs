//! Where the program's log goes. On Linux that's standard output, which systemd hands to the
//! journal. On Windows and macOS nobody sees standard output — Task Scheduler and launchd
//! start the program — so it writes a file of its own, and a terminal too when it has one.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

/// Daily files, and this many days of them.
#[cfg(not(target_os = "linux"))]
const KEEP_DAYS: usize = 14;
pub const FILE_PREFIX: &str = "kyylan-jukebox";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl From<Level> for LevelFilter {
    fn from(level: Level) -> Self {
        match level {
            Level::Error => LevelFilter::ERROR,
            Level::Warn => LevelFilter::WARN,
            Level::Info => LevelFilter::INFO,
            Level::Debug => LevelFilter::DEBUG,
        }
    }
}

pub enum Destination {
    /// A command run by hand: messages to the terminal's standard error, keeping standard
    /// output for what the command prints.
    Terminal,
    /// The jukebox itself.
    Service {
        /// The log folder on platforms that keep a log file.
        #[cfg_attr(target_os = "linux", allow(dead_code))]
        folder: PathBuf,
    },
}

/// The log folder. The installed program uses the platform's place for logs; one pointed at
/// another data directory keeps its logs with that data, so a test or a second jukebox never
/// writes into the user's own.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn folder(data_root: &Path, data_dir_overridden: bool) -> PathBuf {
    if !data_dir_overridden && cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            return home.join("Library/Logs").join(FILE_PREFIX);
        }
    }
    data_root.join("logs")
}

pub fn init(level: Level, destination: Destination) {
    // Symphonia narrates every file it opens, and logs as errors what the player already
    // reports as a song that couldn't be played. Lofty warns about every odd tag it reads,
    // which at each start's rescan would bury the log.
    let filter = Targets::new()
        .with_default(LevelFilter::from(level))
        .with_target("symphonia", LevelFilter::OFF)
        .with_target("lofty", LevelFilter::ERROR);
    let registry = tracing_subscriber::registry().with(filter);

    match destination {
        Destination::Terminal => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_writer(std::io::stderr)
                    .with_ansi(std::io::stderr().is_terminal()),
            )
            .init(),
        #[cfg(target_os = "linux")]
        Destination::Service { .. } => {
            // Under systemd the journal stamps each line with the time and the unit, and
            // shows colour codes literally.
            let journal = std::env::var_os("JOURNAL_STREAM").is_some();
            let layer = tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(!journal && std::io::stdout().is_terminal());
            if journal {
                registry.with(layer.without_time()).init();
            } else {
                registry.with(layer).init();
            }
        }
        #[cfg(not(target_os = "linux"))]
        Destination::Service { folder } => {
            // The appender tidies old files before creating the folder, and complains.
            let _ = std::fs::create_dir_all(&folder);
            let file = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix(FILE_PREFIX)
                .filename_suffix("log")
                .max_log_files(KEEP_DAYS)
                .build(&folder);
            match file {
                // Written as each line is logged, not from a background thread, so nothing
                // is lost when the process exits straight from the tray's event loop.
                Ok(file) => registry
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_target(false)
                            .with_ansi(false)
                            .with_writer(file),
                    )
                    .with(terminal())
                    .init(),
                Err(err) => {
                    registry.with(terminal()).init();
                    tracing::warn!(folder = %folder.display(), %err, "can't write a log file");
                }
            }
        }
    }

    // A panic is logged like everything else, so it lands in the file nobody was watching.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(thread = thread.name().unwrap_or("unnamed"), "{info}");
        default_hook(info);
    }));
}

/// Standard error as well, when there's a terminal to see it.
#[cfg(not(target_os = "linux"))]
fn terminal<S>() -> Option<impl tracing_subscriber::Layer<S>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    std::io::stderr().is_terminal().then(|| {
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_writer(std::io::stderr)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_directory_given_on_the_command_line_keeps_its_logs() {
        let root = Path::new("/srv/jukebox-data");
        assert_eq!(folder(root, true), root.join("logs"));
        if cfg!(target_os = "macos") {
            assert!(folder(root, false).ends_with("Library/Logs/kyylan-jukebox"));
        } else {
            assert_eq!(folder(root, false), root.join("logs"));
        }
    }

    #[test]
    fn levels_are_named_as_documented() {
        let names: Vec<_> = Level::value_variants()
            .iter()
            .map(|l| l.to_possible_value().unwrap().get_name().to_string())
            .collect();
        assert_eq!(names, ["error", "warn", "info", "debug"]);
    }
}
