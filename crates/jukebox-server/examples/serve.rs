//! Runs the server on a data directory, with a player that makes no sound, for trying the
//! web UI against the Rust server before the audio engine and the real program exist.
//!
//!   cargo run -p jukebox-server --example serve -- <data-dir> [port]
//!
//! The data directory is used as-is and written to, so point it at a copy.

use std::sync::Arc;

use jukebox_core::config::ConfigStore;
use jukebox_core::paths::DataDir;
use jukebox_core::player::SilentPlayer;
use jukebox_server::net::{Network, NoFolderPicker, SystemNetwork};
use jukebox_server::{serve, App, Options};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next().map(DataDir::at) else {
        eprintln!("usage: serve <data-dir> [port]");
        std::process::exit(2);
    };
    let config = Arc::new(ConfigStore::open(dir.config_path()).expect("config.json"));
    let port = args
        .next()
        .map(|p| p.parse().expect("port"))
        .unwrap_or(config.get().port);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("port in use");
    let network = Arc::new(SystemNetwork::default());
    let app = App::new(Options {
        config,
        database: dir.database_path(),
        player: Arc::new(SilentPlayer::new()),
        network: network.clone(),
        folder_picker: Arc::new(NoFolderPicker),
        running_port: port,
        version: env!("CARGO_PKG_VERSION").into(),
    })
    .expect("opening the database");
    app.start_playback().expect("starting playback");
    println!(
        "serving {} on http://127.0.0.1:{port}/",
        dir.root().display()
    );
    for ip in network.lan_addresses() {
        println!("guests: http://{ip}:{port}/");
    }
    serve(&app, listener).await.expect("serving");
}
