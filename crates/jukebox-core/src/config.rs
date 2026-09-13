//! `config.json`: plaintext settings, the admin password included by design.
//!
//! Mirrors src/main/config.ts. Stored values merge over the defaults, a missing file is
//! created with defaults, and the file is written the way `JSON.stringify(config, null, 2)`
//! writes it — so the Electron and Rust builds can take turns with the same file.
//!
//! Two deliberate improvements over config.ts: writes are atomic, and a file that fails to
//! parse is set aside rather than silently replaced with defaults on the next save.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Mirror of `AppConfig` in src/shared/types.ts, in the same field order, which is also the
/// key order of a file Electron wrote.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub configured: bool,
    pub port: u16,
    /// Plaintext by design — a LAN party convenience. Never logged: see the `Debug` impl.
    pub admin_password: String,
    pub library_paths: Vec<String>,
    /// 0 = no limit; negative applies the absolute value but hides the counter.
    pub per_user_queue_limit: i32,
    pub output_device_id: Option<String>,
    pub standby_enabled: bool,
    pub standby_shuffle: bool,
    pub standby_random_enabled: bool,
    /// 0 = disabled; negative uses the absolute value but hides the count.
    pub downvote_skip_threshold: i32,
    pub same_song_cooldown_minutes: u32,
    pub same_artist_cooldown_minutes: u32,
    pub add_rate_limit_minutes: u32,
    /// Keys this build doesn't know about, such as ones a newer version wrote. Kept and
    /// written back after the known keys, as config.ts's `{ ...defaults, ...raw }` did.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppConfig {
    /// `DEFAULT_CONFIG` in src/shared/types.ts.
    fn default() -> Self {
        AppConfig {
            configured: false,
            port: 8080,
            admin_password: String::new(),
            library_paths: Vec::new(),
            per_user_queue_limit: 3,
            output_device_id: None,
            standby_enabled: false,
            standby_shuffle: false,
            standby_random_enabled: false,
            downvote_skip_threshold: 0,
            same_song_cooldown_minutes: 0,
            same_artist_cooldown_minutes: 0,
            add_rate_limit_minutes: 0,
            extra: Map::new(),
        }
    }
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("configured", &self.configured)
            .field("port", &self.port)
            .field(
                "admin_password",
                &if self.admin_password.is_empty() {
                    "<unset>"
                } else {
                    "<redacted>"
                },
            )
            .field("library_paths", &self.library_paths)
            .field("per_user_queue_limit", &self.per_user_queue_limit)
            .field("output_device_id", &self.output_device_id)
            .field("standby_enabled", &self.standby_enabled)
            .field("standby_shuffle", &self.standby_shuffle)
            .field("standby_random_enabled", &self.standby_random_enabled)
            .field("downvote_skip_threshold", &self.downvote_skip_threshold)
            .field(
                "same_song_cooldown_minutes",
                &self.same_song_cooldown_minutes,
            )
            .field(
                "same_artist_cooldown_minutes",
                &self.same_artist_cooldown_minutes,
            )
            .field("add_rate_limit_minutes", &self.add_rate_limit_minutes)
            .field("extra", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("{path} is not valid (line {line}, column {column}): {message}")]
    Invalid {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
}

impl AppConfig {
    /// Parses config.json text, filling missing keys from the defaults.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Serializes exactly as `JSON.stringify(config, null, 2)`: two-space indent and no
    /// trailing newline.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("AppConfig always serializes")
    }

    /// Reads a config file without creating or changing anything.
    pub fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.into(),
            source,
        })?;
        Self::from_json(&text).map_err(|e| ConfigError::Invalid {
            path: path.into(),
            line: e.line(),
            column: e.column(),
            message: e.to_string(),
        })
    }
}

/// The live config: read once, updated through `update`, written back on every change.
pub struct ConfigStore {
    path: PathBuf,
    current: RwLock<AppConfig>,
    /// Set when the file on disk failed to parse and the store is running on defaults. The
    /// broken file is moved aside before the first save rather than overwritten.
    unparsed_file: RwLock<bool>,
}

impl ConfigStore {
    /// Loads the config, creating the file with defaults if there isn't one. A file that
    /// exists but doesn't parse is an error.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        match fs::metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let store = ConfigStore::with(path, AppConfig::default(), false);
                store.write(&AppConfig::default())?;
                Ok(store)
            }
            _ => Ok(ConfigStore::with(
                path.clone(),
                AppConfig::read(&path)?,
                false,
            )),
        }
    }

    /// Like `open`, but a file that doesn't parse leaves the store on defaults — as
    /// config.ts did — returning the problem for the caller to report. The unreadable file
    /// is kept, renamed aside, the first time settings are saved.
    pub fn open_or_defaults(path: impl Into<PathBuf>) -> (Self, Option<ConfigError>) {
        let path = path.into();
        match ConfigStore::open(path.clone()) {
            Ok(store) => (store, None),
            Err(err @ ConfigError::Invalid { .. }) => (
                ConfigStore::with(path, AppConfig::default(), true),
                Some(err),
            ),
            Err(err) => (
                ConfigStore::with(path, AppConfig::default(), false),
                Some(err),
            ),
        }
    }

    fn with(path: PathBuf, config: AppConfig, unparsed_file: bool) -> Self {
        ConfigStore {
            path,
            current: RwLock::new(config),
            unparsed_file: RwLock::new(unparsed_file),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> AppConfig {
        self.current.read().expect("config lock poisoned").clone()
    }

    /// Applies a change and saves it, returning the new config.
    pub fn update(&self, change: impl FnOnce(&mut AppConfig)) -> Result<AppConfig, ConfigError> {
        let mut current = self.current.write().expect("config lock poisoned");
        let mut next = current.clone();
        change(&mut next);
        self.write(&next)?;
        *current = next.clone();
        Ok(next)
    }

    fn write(&self, config: &AppConfig) -> Result<(), ConfigError> {
        let fail = |source| ConfigError::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(fail)?;
        }
        let mut unparsed = self.unparsed_file.write().expect("config lock poisoned");
        if *unparsed {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
            let aside = self
                .path
                .with_file_name(format!("config.json.invalid-{stamp}"));
            if self.path.exists() {
                fs::rename(&self.path, &aside).map_err(fail)?;
            }
            *unparsed = false;
        }
        // Write to a sibling and rename over, so a crash can't leave half a file.
        let tmp = self.path.with_file_name("config.json.tmp");
        fs::write(&tmp, config.to_json()).map_err(fail)?;
        fs::rename(&tmp, &self.path).map_err(fail)?;
        Ok(())
    }
}
