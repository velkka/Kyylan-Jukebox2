//! The database must be interchangeable with Electron's: same schema text, same migration
//! bookkeeping, and every row readable.

use std::path::{Path, PathBuf};

use jukebox_core::db::{self, MIGRATIONS, MIGRATIONS_TABLE};
use jukebox_core::rows::{read_every_table, scan, ArtRow, TrackRow};
use rusqlite::Connection;

mod common;
use common::{schema_tsv, ELECTRON_SCHEMA};

fn temp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jukebox.db");
    (dir, path)
}

#[test]
fn fresh_database_has_electrons_exact_schema() {
    let (_dir, path) = temp_db();
    let (conn, report) = db::open(&path).unwrap();
    assert_eq!(report.applied, (1..=10).collect::<Vec<_>>());
    assert_eq!(report.recorded, 10);
    assert_eq!(schema_tsv(&conn), ELECTRON_SCHEMA);
}

#[test]
fn reopening_applies_nothing() {
    let (_dir, path) = temp_db();
    drop(db::open(&path).unwrap());
    let (conn, report) = db::open(&path).unwrap();
    assert!(report.applied.is_empty());
    let stamps: Vec<String> = conn
        .prepare("SELECT DISTINCT applied_at FROM _migrations")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        stamps.len(),
        1,
        "one run stamps every migration with one timestamp, as db.ts does"
    );
    assert!(
        stamps[0].ends_with('Z') && stamps[0].len() == 24,
        "toISOString format: {}",
        stamps[0]
    );
}

#[test]
fn database_from_an_older_release_catches_up() {
    let (_dir, path) = temp_db();
    {
        // What a v0.2.12-era build left behind: migrations 1–9.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATIONS_TABLE).unwrap();
        for (i, sql) in MIGRATIONS.iter().enumerate().take(9) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO _migrations (id, applied_at) VALUES (?1, '2026-09-01T12:00:00.000Z')",
                [(i + 1) as i64],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO play_history (track_id, artist, played_at, is_standby) VALUES (1, 'A', '2026-09-01T12:00:00.000Z', 1)",
            [],
        )
        .unwrap();
    }
    let (conn, report) = db::open(&path).unwrap();
    assert_eq!(report.applied, vec![10]);
    assert_eq!(schema_tsv(&conn), ELECTRON_SCHEMA);
    // Migration 10's backfill ran against the existing row.
    let source: String = conn
        .query_row("SELECT source FROM play_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(source, "standby");
}

#[test]
fn opens_in_wal_mode_with_foreign_keys() {
    let (_dir, path) = temp_db();
    let (conn, _) = db::open(&path).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!((mode.as_str(), fk), ("wal", 1));
}

#[test]
fn migrations_are_verbatim_copies_of_db_ts() {
    let db_ts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/main/db.ts");
    let Ok(source) = std::fs::read_to_string(&db_ts) else {
        eprintln!(
            "skipped: {} not present (Electron sources removed)",
            db_ts.display()
        );
        return;
    };
    let body = &source[source.find("const MIGRATIONS: string[] = [").unwrap()..];
    let body = &body[..body.find("\n]\n").unwrap()];
    // Walk the array literal: skip // comments (which contain backticks), collect `…` strings.
    let (mut sql, mut rest) = (Vec::new(), body);
    while !rest.is_empty() {
        if rest.starts_with("//") {
            rest = &rest[rest.find('\n').unwrap_or(rest.len())..];
        } else if let Some(stripped) = rest.strip_prefix('`') {
            let end = stripped.find('`').unwrap();
            sql.push(&stripped[..end]);
            rest = &stripped[end + 1..];
        } else {
            rest = &rest[rest.chars().next().unwrap().len_utf8()..];
        }
    }
    assert_eq!(
        sql, MIGRATIONS,
        "crates/jukebox-core/migrations/ has drifted from src/main/db.ts"
    );
    assert!(
        source.contains(&format!("d.exec(`{MIGRATIONS_TABLE}`)")),
        "_migrations table SQL drifted from db.ts"
    );
}

#[test]
fn every_table_reads_back_through_typed_rows() {
    let (_dir, path) = temp_db();
    let (conn, _) = db::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO meta (key, value) VALUES ('lastScan', '2026-09-14T10:00:00.000Z'), ('empty', NULL);
         INSERT INTO tracks (path, title, artist, album, album_artist, genre, duration, track_no, disc_no, year, art_hash, mtime_ms, added_at)
           VALUES ('/music/Löyly.mp3', 'Löyly', NULL, NULL, NULL, NULL, 9.769795918367347, NULL, NULL, NULL, NULL, 1726300000000, '2026-09-14T10:00:00.000Z'),
                  ('/music/Going Under.flac', 'Going Under', 'Evanescence', 'Fallen', 'Evanescence', 'Rock', 215, 1, 1, 2003, 'ef49', 1726300000123, '2026-09-14T10:00:00.000Z');
         INSERT INTO art (hash, mime, data) VALUES ('ef49', 'image/jpeg', x'ffd8ffe0');
         INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
           VALUES (2, '10.40.10.129', 'oh7vm-mbp14', '2026-09-14T10:01:00.000Z', 1, 'playing'),
                  (1, '__random__', NULL, '2026-09-14T10:02:00.000Z', 0, 'pending');
         INSERT INTO standby (track_id, position, added_at) VALUES (1, 1, '2026-09-14T10:00:00.000Z');
         INSERT INTO play_history (track_id, artist, played_at, title, requested_by_ip, requested_by_name, is_standby, source)
           VALUES (2, 'Evanescence', '2026-09-14T10:01:00.000Z', 'Going Under', '10.40.10.129', 'oh7vm-mbp14', 0, 'guest');
         INSERT INTO request_log (track_id, title, artist, requested_by_ip, requested_by_name, requested_at)
           VALUES (2, 'Going Under', 'Evanescence', '10.40.10.129', NULL, '2026-09-14T10:00:59.000Z');
         INSERT INTO downvote_log (track_id, title, artist, voter_ip, voter_name, voted_at)
           VALUES (2, 'Going Under', 'Evanescence', '127.0.0.1', 'host', '2026-09-14T10:01:30.000Z');
         INSERT INTO bans (ip, name, banned_at, expires_at) VALUES ('10.0.0.9', NULL, '2026-09-14T10:00:00.000Z', NULL);",
    )
    .unwrap();

    let counts: Vec<_> = read_every_table(&conn).unwrap();
    assert_eq!(
        counts,
        vec![
            ("_migrations", 10),
            ("meta", 2),
            ("tracks", 2),
            ("art", 1),
            ("queue", 2),
            ("standby", 1),
            ("play_history", 1),
            ("request_log", 1),
            ("downvote_log", 1),
            ("bans", 1),
        ]
    );

    let mut tracks = Vec::new();
    scan::<TrackRow>(&conn, |t| tracks.push(t)).unwrap();
    assert_eq!(tracks[0].title, "Löyly");
    assert_eq!(tracks[0].artist, None);
    assert_eq!(tracks[1].duration, Some(215.0));
    assert_eq!(tracks[1].year, Some(2003));
    let mut art = Vec::new();
    scan::<ArtRow>(&conn, |a| art.push(a)).unwrap();
    assert_eq!(art[0].data, vec![0xff, 0xd8, 0xff, 0xe0]);
}

#[test]
fn read_only_open_changes_nothing() {
    // A space in the path, as in macOS's "Application Support".
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("Application Support");
    std::fs::create_dir(&data).unwrap();
    let path = data.join("jukebox.db");
    drop(db::open(&path).unwrap());
    let before = std::fs::read(&path).unwrap();

    let conn = db::open_read_only(&path).unwrap();
    assert_eq!(db::current_migration(&conn).unwrap(), 10);
    assert!(
        conn.execute("DELETE FROM tracks", []).is_err(),
        "writes must be refused"
    );
    drop(conn);

    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(db::open_read_only(&data.join("missing.db")).is_err());
    assert!(
        !data.join("missing.db").exists(),
        "a read-only open must not create a database"
    );
}
