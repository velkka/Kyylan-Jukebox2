//! Where an install keeps its data. This has to be Electron's `app.getPath('userData')`
//! exactly, or everyone who upgrades opens an empty jukebox.

use std::path::{Path, PathBuf};

/// Electron names the folder after package.json's `productName`, falling back to `name`.
/// Kyylan Jukebox only sets `name`, and packaged builds keep it that way.
pub const APP_DIR_NAME: &str = "kyylan-jukebox";

/// The folder a build would have used had it shipped with `productName` set. Only used
/// when the primary folder doesn't exist, so a wrong assumption can't orphan anyone's data.
const PRODUCT_DIR_NAME: &str = "Kyylan Jukebox";

pub const CONFIG_FILE: &str = "config.json";
pub const DATABASE_FILE: &str = "jukebox.db";

/// Overrides the data directory — for a headless server, or tests.
pub const DATA_DIR_ENV: &str = "KYYLAN_DATA_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        DataDir { root: root.into() }
    }

    /// `KYYLAN_DATA_DIR` if set, otherwise the OS default. `None` only on a platform with
    /// no config directory at all.
    pub fn resolve() -> Option<Self> {
        match std::env::var_os(DATA_DIR_ENV) {
            Some(dir) if !dir.is_empty() => Some(DataDir::at(dir)),
            _ => dirs::config_dir().map(|base| DataDir::at(default_root_in(&base))),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    pub fn database_path(&self) -> PathBuf {
        self.root.join(DATABASE_FILE)
    }
}

/// The data directory inside an OS config directory. `dirs::config_dir()` is exactly
/// Electron's `appData` on every platform: `~/Library/Application Support` on macOS,
/// `%APPDATA%` on Windows, and `$XDG_CONFIG_HOME` or `~/.config` on Linux.
pub fn default_root_in(config_base: &Path) -> PathBuf {
    let primary = config_base.join(APP_DIR_NAME);
    let alternate = config_base.join(PRODUCT_DIR_NAME);
    if !primary.exists() && alternate.is_dir() {
        alternate
    } else {
        primary
    }
}
