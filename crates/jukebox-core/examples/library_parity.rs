//! Phase 2's exit check on a real library: rescans an install's music folders with the Rust
//! scanner into a scratch database and compares every track with the rows Electron wrote.
//!
//!   cargo run --release -p jukebox-core --example library_parity [-- <data-dir> [--report <file.tsv>]]
//!
//! With no data directory it uses this machine's. Nothing in the data directory or the music
//! folders is changed: the Electron database is opened read-only, files are only read, and
//! the Rust scan goes to a temporary database that's deleted afterwards. The optional report
//! lists every difference, one per line, for review.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Instant, UNIX_EPOCH};

use jukebox_core::config::AppConfig;
use jukebox_core::db;
use jukebox_core::library::{is_audio_file, Scanner};
use jukebox_core::paths::DataDir;
use jukebox_core::rows::{scan, TrackRow};

/// Durations closer than this count as the same. lofty rounds to the millisecond; bigger
/// gaps are listed with their size.
const DURATION_TOLERANCE: f64 = 0.01;
const SAMPLES: usize = 8;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut report = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--report" => report = args.next(),
            _ => dir = Some(DataDir::at(arg)),
        }
    }
    let Some(dir) = dir.or_else(DataDir::resolve) else {
        eprintln!("no data directory on this platform");
        return ExitCode::FAILURE;
    };

    let config = match AppConfig::read(&dir.config_path()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("can't read config.json: {e}");
            return ExitCode::FAILURE;
        }
    };
    let electron = match db::open_read_only(&dir.database_path()) {
        Ok(conn) => {
            let mut rows = HashMap::new();
            scan::<TrackRow>(&conn, |t| {
                rows.insert(t.path.clone(), t);
            })
            .expect("reading Electron's tracks");
            rows
        }
        Err(e) => {
            eprintln!("can't open jukebox.db: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("data directory   {}", dir.root().display());
    println!("library folders  {:?}", config.library_paths);
    println!("electron tracks  {}", electron.len());

    let scratch = tempfile::tempdir().expect("temporary directory");
    let (conn, _) = db::open(&scratch.path().join("jukebox.db")).expect("scratch database");
    let db = Mutex::new(conn);
    let started = Instant::now();
    let status = Scanner::new().scan(&db, &config.library_paths);
    println!(
        "rust scan        {} files, {} tracks in {:.2}s{}",
        status.processed,
        status.total,
        started.elapsed().as_secs_f64(),
        status
            .error
            .map(|e| format!(" — FAILED: {e}"))
            .unwrap_or_default()
    );
    let mut rust = HashMap::new();
    scan::<TrackRow>(&db.lock().unwrap(), |t| {
        rust.insert(t.path.clone(), t);
    })
    .expect("reading the Rust scan");
    let rust_art: HashMap<String, String> = art_mimes(&db.lock().unwrap());
    let electron_art: HashMap<String, String> =
        art_mimes(&db::open_read_only(&dir.database_path()).unwrap());

    let mut diffs: BTreeMap<&'static str, Vec<[String; 3]>> = BTreeMap::new();
    let mut note = |kind: &'static str, path: &str, electron: String, rust: String| {
        diffs
            .entry(kind)
            .or_default()
            .push([path.to_string(), electron, rust]);
    };
    let mut compared = 0;
    let mut changed_since = 0;
    let mut gone_since = 0;
    let mut worst_duration = 0.0f64;

    for (path, e) in &electron {
        let Some(r) = rust.get(path) else {
            match fs::metadata(path) {
                Err(_) => gone_since += 1,
                Ok(_) if !is_audio_file(path) => note(
                    "dropped format (.wma)",
                    path,
                    "indexed".into(),
                    "not indexed".into(),
                ),
                Ok(_) => note(
                    "only in electron",
                    path,
                    "indexed".into(),
                    "not found".into(),
                ),
            }
            continue;
        };
        let mtime = fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        if mtime != Some(e.mtime_ms) {
            changed_since += 1;
            continue;
        }
        compared += 1;
        let n = |v: Option<i64>| v.map(|n| n.to_string());
        let artist = match (&e.artist, &r.artist) {
            (Some(a), Some(b)) if a != b && b.contains('/') && b.starts_with(a.as_str()) => {
                note(
                    "artist: ID3v2.3 '/' no longer split (deliberate)",
                    path,
                    a.clone(),
                    b.clone(),
                );
                None
            }
            _ => Some(("artist", e.artist.clone(), r.artist.clone())),
        };
        let fields = [
            Some(("title", Some(e.title.clone()), Some(r.title.clone()))),
            artist,
            Some(("album", e.album.clone(), r.album.clone())),
            Some((
                "album_artist",
                e.album_artist.clone(),
                r.album_artist.clone(),
            )),
            Some(("genre", e.genre.clone(), r.genre.clone())),
            Some(("track_no", n(e.track_no), n(r.track_no))),
            Some(("disc_no", n(e.disc_no), n(r.disc_no))),
            Some(("year", n(e.year), n(r.year))),
            Some(("art_hash", e.art_hash.clone(), r.art_hash.clone())),
        ];
        for (name, a, b) in fields.into_iter().flatten() {
            if a != b {
                note(name, path, format!("{a:?}"), format!("{b:?}"));
            }
        }
        if let (Some(a), Some(b)) = (&e.art_hash, &r.art_hash) {
            if a == b && electron_art.get(a) != rust_art.get(b) {
                note(
                    "art mime",
                    path,
                    format!("{:?}", electron_art.get(a)),
                    format!("{:?}", rust_art.get(b)),
                );
            }
        }
        match (e.duration, r.duration) {
            (Some(a), Some(b)) => {
                let gap = (a - b).abs();
                worst_duration = worst_duration.max(gap);
                if gap > DURATION_TOLERANCE {
                    note(
                        "duration",
                        path,
                        format!("{a:.3}"),
                        format!("{b:.3} (off by {gap:.3}s)"),
                    );
                }
            }
            (None, None) => {}
            (a, b) => note("duration", path, format!("{a:?}"), format!("{b:?}")),
        }
    }
    let new_since = rust.keys().filter(|p| !electron.contains_key(*p)).count();

    println!("compared         {compared} tracks unchanged since Electron scanned them");
    println!("not compared     {changed_since} modified since, {gone_since} deleted since, {new_since} added since");
    println!("largest duration gap {worst_duration:.3}s");
    if diffs.is_empty() {
        println!("\nno differences");
    }
    for (kind, list) in &diffs {
        println!("\n{kind}: {}", list.len());
        for [path, e, r] in list.iter().take(SAMPLES) {
            println!("  {path}\n    electron {e}\n    rust     {r}");
        }
    }
    if let Some(report) = report {
        let mut out = fs::File::create(&report).expect("report file");
        writeln!(out, "difference\tpath\telectron\trust").unwrap();
        for (kind, list) in &diffs {
            for [path, e, r] in list {
                writeln!(out, "{kind}\t{path}\t{e}\t{r}").unwrap();
            }
        }
        println!("\nfull list in {report}");
    }
    ExitCode::SUCCESS
}

fn art_mimes(conn: &rusqlite::Connection) -> HashMap<String, String> {
    conn.prepare("SELECT hash, mime FROM art")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
