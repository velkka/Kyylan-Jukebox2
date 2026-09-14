//! The Rust library does what Electron's did, checked against Electron's own code.
//!
//! tests/fixtures/library-electron.json was recorded by scripts/library-oracle.mjs, which
//! runs the real src/main/library.ts inside Electron: three scans of tests/fixtures/library,
//! then a batch of queries and the CSV export over the resulting database. These tests
//! repeat each step with the Rust code and compare.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use jukebox_core::db;
use jukebox_core::library::query::{self, AlbumsQuery, ArtistsQuery};
use jukebox_core::library::{export, folders, Scanner};
use jukebox_core::types::TracksQuery;
use rusqlite::types::Value;
use rusqlite::Connection;
use serde_json::{json, Map, Value as Json};
use sha1::{Digest, Sha1};

/// Where the Rust build stores something different on purpose: (path, column, Electron's
/// value, the Rust build's value, why). Each is a music-metadata bug.
const DELIBERATE: &[(&str, &str, &str, &str, &str)] = &[(
    "/music/id3v23-slash-artist.mp3",
    "artist",
    "AC",
    "AC/DC",
    "music-metadata splits an ID3v2.3 artist on every '/', so AC/DC was stored as AC",
)];

fn oracle() -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library-electron.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Library {
    _dir: tempfile::TempDir,
    root: String,
    db: Mutex<Connection>,
}

impl Library {
    /// A scratch copy of the fixtures and an empty database, as the oracle started with.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library"),
            &music,
        );
        let (conn, _) = db::open(&dir.path().join("jukebox.db")).unwrap();
        Library {
            root: music.to_str().unwrap().to_string(),
            db: Mutex::new(conn),
            _dir: dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        Path::new(&self.root).join(name)
    }

    /// The oracle's `/music` spelling of a path under the scratch copy.
    fn hide(&self, path: &str) -> String {
        path.replace(&self.root, "/music").replace('\\', "/")
    }

    fn scan(&self, roots: &[String]) -> Json {
        let status = Scanner::new().scan(&self.db, roots);
        let mut status = serde_json::to_value(status).unwrap();
        for stamp in ["startedAt", "finishedAt"] {
            status[stamp] = json!(!status[stamp].is_null());
        }
        status
    }

    fn tracks(&self) -> Vec<Map<String, Json>> {
        rows(&self.db.lock().unwrap(), "SELECT * FROM tracks ORDER BY id")
    }
}

fn rows(conn: &Connection, sql: &str) -> Vec<Map<String, Json>> {
    let mut stmt = conn.prepare(sql).unwrap();
    let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    stmt.query_map([], |r| {
        Ok(names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let value = match r.get::<_, Value>(i).unwrap() {
                    Value::Null => Json::Null,
                    Value::Integer(n) => json!(n),
                    Value::Real(n) => json!(n),
                    Value::Text(s) => json!(s),
                    Value::Blob(b) => json!(b),
                };
                (name.clone(), value)
            })
            .collect())
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// Compares one scan's rows with Electron's, column by column. Modification times and
/// `added_at` stamps depend on when each copy was made, so they're checked separately.
fn assert_rows_match(lib: &Library, electron: &[Json], label: &str) {
    let ours = lib.tracks();
    let mut problems = Vec::new();
    if ours.len() != electron.len() {
        problems.push(format!(
            "{} rows, Electron had {}",
            ours.len(),
            electron.len()
        ));
    }
    for (mut row, theirs) in ours.into_iter().zip(electron) {
        let path = lib.hide(row["path"].as_str().unwrap());
        row.insert("path".into(), json!(path));
        for (column, want) in theirs.as_object().unwrap() {
            let got = &row[column];
            if matches!(column.as_str(), "mtime_ms" | "added_at") {
                continue;
            }
            let want = match DELIBERATE.iter().find(|d| d.0 == path && d.1 == column) {
                Some(d) => {
                    assert_eq!(
                        want, d.2,
                        "{path} {column}: Electron's recorded value changed"
                    );
                    json!(d.3)
                }
                None => want.clone(),
            };
            let same = match (column.as_str(), want.as_f64(), got.as_f64()) {
                // lofty reports whole milliseconds, and estimates a raw ADTS stream's length
                // from its bitrate where music-metadata counts every frame.
                ("duration", Some(w), Some(g)) => {
                    (w - g).abs() <= if path.ends_with(".aac") { 0.1 } else { 0.001 }
                }
                _ => want == *got,
            };
            if !same {
                problems.push(format!("{path} {column}: electron {want}, rust {got}"));
            }
        }
        let added_at = row["added_at"].as_str().unwrap();
        assert!(
            added_at.ends_with('Z') && added_at.len() == 24,
            "{added_at}"
        );
    }
    assert!(problems.is_empty(), "{label}:\n{}", problems.join("\n"));
}

#[test]
fn scans_match_electron() {
    let oracle = oracle();
    let scans = oracle["scans"].as_array().unwrap();
    let lib = Library::new();
    let sep = std::path::MAIN_SEPARATOR;

    // 1. A first scan, with the folder spelled with a trailing separator.
    let status = lib.scan(&[format!("{}{sep}", lib.root)]);
    assert_eq!(status, scans[0]["status"], "first scan's status");
    assert_rows_match(&lib, scans[0]["tracks"].as_array().unwrap(), "first scan");

    // 2. Nothing changed, and the second folder lies inside the first.
    let before = lib.tracks();
    let status = lib.scan(&[lib.root.clone(), format!("{}{sep}Disc 2", lib.root)]);
    assert_eq!(status, scans[1]["status"], "second scan's status");
    assert_rows_match(&lib, scans[1]["tracks"].as_array().unwrap(), "second scan");
    assert_eq!(
        lib.tracks(),
        before,
        "an unchanged file isn't touched at all"
    );

    // 3. One file modified, one deleted.
    let modified = UNIX_EPOCH + Duration::from_secs(1_577_836_800);
    fs::File::options()
        .write(true)
        .open(lib.path("untagged.mp3"))
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::remove_file(lib.path("not-audio.mp3")).unwrap();
    let status = lib.scan(std::slice::from_ref(&lib.root));
    assert_eq!(status, scans[2]["status"], "third scan's status");
    let electron = scans[2]["tracks"].as_array().unwrap();
    assert_rows_match(&lib, electron, "third scan");
    let mtime = |rows: &[Map<String, Json>]| {
        rows.iter()
            .find(|r| r["path"].as_str().unwrap().ends_with("untagged.mp3"))
            .map(|r| r["mtime_ms"].clone())
    };
    let electron_rows: Vec<Map<String, Json>> = electron
        .iter()
        .map(|r| r.as_object().unwrap().clone())
        .collect();
    assert_eq!(mtime(&lib.tracks()), mtime(&electron_rows));

    // Cover art, stored once per image.
    let art = rows(
        &lib.db.lock().unwrap(),
        "SELECT hash, mime, data FROM art ORDER BY hash",
    );
    let art: Vec<Json> = art
        .into_iter()
        .map(|a| {
            let bytes: Vec<u8> = serde_json::from_value(a["data"].clone()).unwrap();
            json!({ "hash": a["hash"], "mime": a["mime"], "data": base64(&bytes) })
        })
        .collect();
    assert_eq!(Json::Array(art), oracle["art"]);
}

/// The database the oracle ran its queries against, row for row.
fn query_database(oracle: &Json) -> Connection {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = db::open(&dir.path().join("jukebox.db")).unwrap();
    // The connection keeps the file open; the directory can go.
    std::mem::forget(dir);
    for row in oracle["database"].as_array().unwrap() {
        let row = row.as_object().unwrap();
        let columns: Vec<&String> = row.keys().collect();
        let sql = format!(
            "INSERT INTO tracks ({}) VALUES ({})",
            columns
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            columns
                .iter()
                .map(|c| format!(":{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let values: Vec<(String, Value)> = row
            .iter()
            .map(|(k, v)| {
                let value = match v {
                    Json::Null => Value::Null,
                    Json::String(s) => Value::Text(s.clone()),
                    Json::Number(n) if n.is_i64() => Value::Integer(n.as_i64().unwrap()),
                    Json::Number(n) => Value::Real(n.as_f64().unwrap()),
                    other => panic!("unexpected {other}"),
                };
                (format!(":{k}"), value)
            })
            .collect();
        let params: Vec<(&str, &dyn rusqlite::ToSql)> = values
            .iter()
            .map(|(k, v)| (k.as_str(), v as &dyn rusqlite::ToSql))
            .collect();
        conn.execute(&sql, params.as_slice()).unwrap();
    }
    for art in oracle["art"].as_array().unwrap() {
        conn.execute(
            "INSERT INTO art (hash, mime, data) VALUES (?1, ?2, ?3)",
            (
                art["hash"].as_str().unwrap(),
                art["mime"].as_str().unwrap(),
                unbase64(art["data"].as_str().unwrap()),
            ),
        )
        .unwrap();
    }
    conn
}

#[test]
fn queries_match_electron() {
    let oracle = oracle();
    let conn = query_database(&oracle);
    let mut problems = Vec::new();
    for case in oracle["queries"].as_array().unwrap() {
        let args = &case["args"];
        let id = || args["id"].as_i64().unwrap();
        let got = match case["fn"].as_str().unwrap() {
            "tracks" => json!(query::tracks(
                &conn,
                &serde_json::from_value::<TracksQuery>(args.clone()).unwrap()
            )
            .unwrap()),
            "artists" => json!(query::artists(
                &conn,
                &serde_json::from_value::<ArtistsQuery>(args.clone()).unwrap()
            )
            .unwrap()),
            "albums" => json!(query::albums(
                &conn,
                &serde_json::from_value::<AlbumsQuery>(args.clone()).unwrap()
            )
            .unwrap()),
            "trackById" => json!(query::track_by_id(&conn, id()).unwrap()),
            "trackPath" => json!(query::track_path(&conn, id()).unwrap()),
            "art" => json!(query::art(&conn, args["hash"].as_str().unwrap())
                .unwrap()
                .map(|(mime, data)| json!({ "mime": mime, "sha1": hex(&Sha1::digest(&data)) }))),
            "pathsWithCounts" => {
                if cfg!(windows) {
                    // The recorded paths use '/', which a Windows folder never does; the
                    // Windows prefix has its own test.
                    continue;
                }
                let paths: Vec<String> = serde_json::from_value(args["paths"].clone()).unwrap();
                json!(folders::with_counts(&conn, &paths).unwrap())
            }
            other => panic!("unknown query {other}"),
        };
        if got != case["result"] {
            problems.push(format!(
                "{} {}:\n  electron {}\n  rust     {}",
                case["fn"], args, case["result"], got
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn csv_export_matches_electron() {
    let oracle = oracle();
    let conn = query_database(&oracle);
    assert_eq!(
        export::library_csv(&conn).unwrap(),
        oracle["csv"].as_str().unwrap()
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unbase64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let mut n = 0u32;
        let mut len = 0;
        for (i, c) in chunk.iter().enumerate() {
            if let Some(v) = B64.iter().position(|b| b == c) {
                n |= (v as u32) << (18 - 6 * i);
                len += 1;
            }
        }
        for i in 0..len - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    out
}

#[test]
fn base64_round_trips() {
    for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
        assert_eq!(unbase64(&base64(data)), data);
    }
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64(b"fo"), "Zm8=");
}
