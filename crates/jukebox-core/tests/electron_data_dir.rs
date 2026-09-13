//! Phase 1's exit check, runnable on every platform: a data directory the Electron v0.2.15
//! build wrote reads completely — config, migrations and every row — and opens for writing
//! without needing any migration. See tests/fixtures/electron-v0.2.15/README.md.

use std::fs;
use std::path::Path;

use jukebox_core::config::{AppConfig, ConfigStore};
use jukebox_core::db;
use jukebox_core::paths::DataDir;
use jukebox_core::rows::{read_every_table, scan, BanRow, PlayHistoryRow, QueueRow, TrackRow};

mod common;
use common::{schema_tsv, ELECTRON_SCHEMA};

/// A throwaway copy, so opening the database never touches the committed fixture.
fn copy_of_fixture() -> (tempfile::TempDir, DataDir) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/electron-v0.2.15");
    let dir = tempfile::tempdir().unwrap();
    for file in ["config.json", "jukebox.db"] {
        fs::copy(fixture.join(file), dir.path().join(file)).unwrap();
    }
    let data = DataDir::at(dir.path());
    (dir, data)
}

#[test]
fn config_reads_and_saves_back_unchanged() {
    let (_dir, data) = copy_of_fixture();
    let written = fs::read(data.config_path()).unwrap();

    let config = AppConfig::read(&data.config_path()).unwrap();
    assert!(config.configured);
    assert_eq!(config.port, 8094);
    assert_eq!(config.admin_password, "fixture-admin");
    assert_eq!(config.library_paths, ["/fixtures/music"]);
    assert_eq!(
        config.downvote_skip_threshold, 5,
        "set through the admin API, so Electron wrote it"
    );
    assert!(config.standby_enabled, "switched on through the admin API");
    assert!(config.extra.is_empty());

    ConfigStore::open(data.config_path())
        .unwrap()
        .update(|_| {})
        .unwrap();
    assert_eq!(fs::read(data.config_path()).unwrap(), written);
}

#[test]
fn every_table_reads_completely() {
    let (_dir, data) = copy_of_fixture();
    let conn = db::open_read_only(&data.database_path()).unwrap();
    assert_eq!(
        db::current_migration(&conn).unwrap(),
        db::latest_migration()
    );
    assert_eq!(
        schema_tsv(&conn),
        ELECTRON_SCHEMA,
        "a database Electron created in one go has the same schema"
    );

    assert_eq!(
        read_every_table(&conn).unwrap(),
        vec![
            ("_migrations", 10),
            ("meta", 0),
            ("tracks", 4),
            ("art", 1),
            ("queue", 2),
            ("standby", 1),
            ("play_history", 2),
            ("request_log", 3),
            ("downvote_log", 1),
            ("bans", 2),
        ]
    );

    let mut tracks = Vec::new();
    scan::<TrackRow>(&conn, |t| tracks.push(t)).unwrap();
    let by_title = |title: &str| tracks.iter().find(|t| t.title == title).unwrap();
    let untagged = by_title("Löyly");
    assert_eq!(
        (untagged.artist.as_deref(), untagged.art_hash.as_deref()),
        (None, None)
    );
    let tagged = by_title("Second Wind");
    assert_eq!(tagged.album.as_deref(), Some("Test Album"));
    assert_eq!(tagged.track_no, Some(2));
    assert!(tagged.art_hash.is_some());
    assert_eq!(
        by_title("First Light").art_hash,
        tagged.art_hash,
        "shared cover art is stored once"
    );
    assert_eq!(
        by_title("Loose Single").artist.as_deref(),
        Some("Äänitys Band")
    );
    assert!(tracks
        .iter()
        .all(|t| t.duration.unwrap() > 1.0 && t.path.starts_with("/fixtures/music")));

    let mut queue = Vec::new();
    scan::<QueueRow>(&conn, |q| queue.push(q)).unwrap();
    assert!(queue.iter().any(|q| q.status == "playing"));

    let mut history = Vec::new();
    scan::<PlayHistoryRow>(&conn, |h| history.push(h)).unwrap();
    assert!(history.iter().all(|h| h.source == "guest"));

    let mut bans = Vec::new();
    scan::<BanRow>(&conn, |b| bans.push(b)).unwrap();
    assert_eq!(
        bans.iter().filter(|b| b.expires_at.is_none()).count(),
        1,
        "one permanent, one timed"
    );
}

#[test]
fn opens_for_writing_with_nothing_to_migrate() {
    let (_dir, data) = copy_of_fixture();
    let (conn, report) = db::open(&data.database_path()).unwrap();
    assert!(report.applied.is_empty());
    assert_eq!(report.recorded, db::latest_migration());
    let tracks: i64 = conn
        .query_row(
            "SELECT count(*) FROM tracks_fts WHERE tracks_fts MATCH '\"aanitys\"*'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        tracks, 1,
        "Electron's full-text index works, accent folding included"
    );
}
