//! Library behaviour beyond what the Electron comparison covers: running one scan at a
//! time, failures, and managing the library folders.

use std::fs;
use std::sync::Mutex;

use jukebox_core::config::ConfigStore;
use jukebox_core::db;
use jukebox_core::library::folders::{self, FolderError};
use jukebox_core::library::Scanner;
use rusqlite::Connection;

fn database() -> (tempfile::TempDir, Mutex<Connection>) {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = db::open(&dir.path().join("jukebox.db")).unwrap();
    (dir, Mutex::new(conn))
}

#[test]
fn one_scan_runs_at_a_time() {
    let (_dir, db) = database();
    let scanner = Scanner::new();
    let ticket = scanner.start().expect("nothing is running yet");

    let running = scanner.status();
    assert!(running.scanning);
    assert!(running.started_at.is_some() && running.finished_at.is_none());
    assert!(scanner.start().is_none(), "a second scan is refused");
    assert_eq!(
        scanner.scan(&db, &[]),
        running,
        "asking to scan while one runs reports the running scan"
    );

    let done = scanner.run(ticket, &db, &[]);
    assert!(!done.scanning && done.finished_at.is_some() && done.error.is_none());
    assert!(
        scanner.start().is_some(),
        "and once it's done, another can start"
    );
}

#[test]
fn a_database_failure_ends_the_scan_with_an_error() {
    let db = Mutex::new(Connection::open_in_memory().unwrap());
    let status = Scanner::new().scan(&db, &["/nowhere".to_string()]);
    assert!(!status.scanning);
    assert!(status.error.unwrap().contains("no such table"));
    assert!(status.finished_at.is_some());
}

#[test]
fn a_missing_library_folder_keeps_its_tracks_until_it_leaves_the_library() {
    // Unlike Electron, which removed them: a drive that isn't plugged in when the jukebox
    // starts and rescans must not take the library, the queue and the standby playlist with
    // it.
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = db::open(&dir.path().join("jukebox.db")).unwrap();
    let drive = dir.path().join("drive");
    let here = dir.path().join("here");
    fs::create_dir(&here).unwrap();
    for folder in [&drive, &here] {
        conn.execute(
            "INSERT INTO tracks (path, title, mtime_ms, added_at) VALUES (?, 'a', 0, 'x')",
            [folder.join("a.mp3").to_str().unwrap()],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO standby (track_id, position, added_at) VALUES (1, 0, 'x')",
        [],
    )
    .unwrap();
    let db = Mutex::new(conn);
    let roots = [
        drive.to_str().unwrap().to_string(),
        here.to_str().unwrap().to_string(),
    ];

    let status = Scanner::new().scan(&db, &roots);
    assert_eq!(
        (status.removed, status.total, status.error),
        (1, 1, None),
        "the missing drive's track stays; the file gone from a folder that's there doesn't"
    );
    let standby: i64 = db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM standby", [], |r| r.get(0))
        .unwrap();
    assert_eq!(standby, 1);

    // Taken out of the library, its tracks go at the next scan.
    let status = Scanner::new().scan(&db, &roots[1..]);
    assert_eq!((status.removed, status.total), (1, 0));
}

#[cfg(unix)]
#[test]
fn an_unreadable_subfolder_is_skipped() {
    use jukebox_core::library::Walker;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("music");
    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(root.join("a.mp3"), b"").unwrap();
    fs::write(locked.join("b.mp3"), b"").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&locked).is_ok(); // running as root
    let found: Vec<String> = Walker::new(root.to_str().unwrap()).collect();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return;
    }
    assert_eq!(found, [root.join("a.mp3").to_str().unwrap()]);
}

fn config() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::open(dir.path().join("config.json")).unwrap();
    (dir, store)
}

#[test]
fn adding_a_folder_checks_it_and_trims_it() {
    let (dir, store) = config();
    let music = dir.path().join("music");
    fs::create_dir(&music).unwrap();
    let music = music.to_str().unwrap().to_string();

    assert_eq!(
        folders::add(&store, "  ").unwrap_err().to_string(),
        "Path is required"
    );
    let missing = dir.path().join("missing");
    assert_eq!(
        folders::add(&store, missing.to_str().unwrap())
            .unwrap_err()
            .to_string(),
        "Folder does not exist"
    );
    let file = dir.path().join("config.json");
    assert!(matches!(
        folders::add(&store, file.to_str().unwrap()),
        Err(FolderError::NotFound)
    ));

    assert_eq!(
        folders::add(&store, &format!(" {music}\t")).unwrap(),
        [music.as_str()]
    );
    assert_eq!(
        folders::add(&store, &music).unwrap(),
        [music.as_str()],
        "no duplicates"
    );
    assert_eq!(store.get().library_paths, [music.as_str()]);

    assert_eq!(
        folders::remove(&store, "elsewhere").unwrap(),
        [music.as_str()]
    );
    assert!(folders::remove(&store, &music).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn adding_an_unreadable_folder_says_why() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, store) = config();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let result = folders::add(&store, locked.to_str().unwrap());
    let readable_anyway = fs::read_dir(&locked).is_ok(); // running as root
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return;
    }
    assert_eq!(
        result.unwrap_err().to_string(),
        "Folder can't be read: permission denied"
    );
    assert!(store.get().library_paths.is_empty());
}

#[test]
fn folder_counts_match_the_scanners_spelling() {
    let (_dir, db) = database();
    let conn = db.lock().unwrap();
    let (base, doubled, other) = if cfg!(windows) {
        ("C:\\Music", "C:/Music//", "C:\\Musical")
    } else {
        ("/srv/music", "/srv//music/", "/srv/musical")
    };
    let sep = std::path::MAIN_SEPARATOR;
    for (i, path) in [
        format!("{base}{sep}a.mp3"),
        format!("{base}{sep}x{sep}b.mp3"),
        format!("{other}{sep}c.mp3"),
    ]
    .iter()
    .enumerate()
    {
        conn.execute(
            "INSERT INTO tracks (path, title, mtime_ms, added_at) VALUES (?1, ?2, 0, 'x')",
            (path, i.to_string()),
        )
        .unwrap();
    }
    let counts = folders::with_counts(&conn, &[base.into(), doubled.into(), other.into()]).unwrap();
    let got: Vec<(String, i64)> = counts
        .paths
        .into_iter()
        .map(|p| (p.path, p.track_count))
        .collect();
    assert_eq!(
        got,
        [(base.into(), 2), (doubled.into(), 2), (other.into(), 1)],
        "a sibling folder sharing the name's start isn't counted"
    );
    assert_eq!(counts.total, 3);
}
