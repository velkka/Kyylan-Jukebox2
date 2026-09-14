//! Kyylan Jukebox core: where an install's data lives, its config file, its database, the
//! music library, and the JSON types shared with the web UI. Every piece is compatible with what the Electron
//! build reads and writes, so the two can open each other's data.

pub mod config;
pub mod db;
pub mod engine;
pub mod js;
pub mod library;
pub mod paths;
pub mod player;
pub mod rows;
pub mod types;
