//! The library folders: `libraryPaths` in the config, and how many tracks sit under each.

use std::fs;
use std::io;

use rusqlite::Connection;

use super::js;
use crate::config::{ConfigError, ConfigStore};
use crate::types::{LibraryPath, LibraryPathsResponse};

#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error("Path is required")]
    Required,
    #[error("Folder does not exist")]
    NotFound,
    /// New in the Rust build. Electron added a folder it couldn't list and then quietly
    /// indexed nothing from it — the likely outcome for a system service pointed at a home
    /// directory — so this says so when the folder is added instead.
    #[error("Folder can't be read: {}", reason(.0))]
    Unreadable(io::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
}

fn reason(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => err.to_string(),
    }
}

/// Adds a folder, trimmed, unless it's already there. Returns the new list.
pub fn add(config: &ConfigStore, path: &str) -> Result<Vec<String>, FolderError> {
    let clean = js::trim(path);
    if clean.is_empty() {
        return Err(FolderError::Required);
    }
    if !fs::metadata(clean).is_ok_and(|m| m.is_dir()) {
        return Err(FolderError::NotFound);
    }
    fs::read_dir(clean).map_err(FolderError::Unreadable)?;
    let current = config.get().library_paths;
    if current.iter().any(|p| p == clean) {
        return Ok(current);
    }
    Ok(config
        .update(|c| c.library_paths.push(clean.to_string()))?
        .library_paths)
}

/// Removes a folder, matched exactly. Its tracks stay until the next scan prunes them.
pub fn remove(config: &ConfigStore, path: &str) -> Result<Vec<String>, ConfigError> {
    Ok(config
        .update(|c| c.library_paths.retain(|p| p != path))?
        .library_paths)
}

/// Each folder with the number of indexed tracks under it, plus the library's total.
pub fn with_counts(conn: &Connection, paths: &[String]) -> rusqlite::Result<LibraryPathsResponse> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) AS c FROM tracks WHERE substr(path, 1, length(@prefix)) = @prefix",
    )?;
    let paths = paths
        .iter()
        .map(|path| {
            let track_count = stmt.query_row(&[("@prefix", &folder_prefix(path))], |r| r.get(0))?;
            Ok(LibraryPath {
                path: path.clone(),
                track_count,
            })
        })
        .collect::<rusqlite::Result<_>>()?;
    let total = conn.query_row("SELECT COUNT(*) AS c FROM tracks", [], |r| r.get(0))?;
    Ok(LibraryPathsResponse { paths, total })
}

/// The folder spelled the way the scanner spells the paths under it, with exactly one
/// trailing separator — so a stored trailing slash, or the lack of one, doesn't change the
/// count.
///
/// Electron appended `/` to the folder as typed. That never matched a Windows track path,
/// so every Windows folder showed 0 tracks, and neither did a folder typed with a doubled
/// slash. Normalizing it as the scanner's paths are normalized fixes both.
pub fn folder_prefix(path: &str) -> String {
    if cfg!(windows) {
        format!("{}\\", js::normalize_win32(path).trim_end_matches('\\'))
    } else {
        format!("{}/", js::normalize_posix(path).trim_end_matches('/'))
    }
}
