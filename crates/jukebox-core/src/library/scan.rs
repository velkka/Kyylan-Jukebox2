//! Rescanning the library folders into the `tracks` table.
//!
//! The outcome matches Electron's `scanAll` row for row: the same files, paths, change
//! detection, counters, and pruning of files that are gone. What differs is the work behind
//! it. Electron parsed and wrote one file at a time; this walks in batches, parses each
//! batch's new and changed files in parallel, and writes the batch in one transaction,
//! holding the database only for that write so queue changes aren't kept waiting.

use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use rusqlite::Connection;

use super::folders::folder_prefix;
use super::metadata::{self, TrackMeta};
use super::walk::Walker;
use crate::db::iso_now;
use crate::types::ScanStatus;

/// Files per batch. Big enough that parsing runs in parallel, small enough that progress
/// moves steadily and each write transaction is short.
const BATCH: usize = 256;

/// Runs scans and reports their progress. One scan runs at a time.
pub struct Scanner {
    status: Mutex<ScanStatus>,
}

/// Proof that [`Scanner::start`] claimed the scan, to be spent on [`Scanner::run`].
#[must_use = "a started scan reports `scanning: true` until it's run"]
pub struct ScanTicket(());

impl Default for Scanner {
    fn default() -> Self {
        Scanner {
            status: Mutex::new(ScanStatus {
                scanning: false,
                processed: 0,
                added: 0,
                updated: 0,
                removed: 0,
                total: 0,
                started_at: None,
                finished_at: None,
                error: None,
            }),
        }
    }
}

impl Scanner {
    pub fn new() -> Self {
        Scanner::default()
    }

    pub fn status(&self) -> ScanStatus {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ScanStatus> {
        self.status.lock().expect("scan status lock poisoned")
    }

    /// Claims the scan and resets the counters, or returns `None` if a scan is already
    /// running. Split from [`run`](Self::run) so a request handler can start a scan, reply
    /// with the fresh status at once — as Electron's fire-and-forget route did — and run it
    /// in the background.
    pub fn start(&self) -> Option<ScanTicket> {
        let mut status = self.lock();
        if status.scanning {
            return None;
        }
        *status = ScanStatus {
            scanning: true,
            processed: 0,
            added: 0,
            updated: 0,
            removed: 0,
            total: 0,
            started_at: Some(iso_now()),
            finished_at: None,
            error: None,
        };
        Some(ScanTicket(()))
    }

    /// Rescans `roots` in order and returns the final status. A failure partway — the
    /// database, never a single unreadable file — ends the scan and is reported in `error`.
    pub fn run(&self, _ticket: ScanTicket, db: &Mutex<Connection>, roots: &[String]) -> ScanStatus {
        match self.sweep(db, roots) {
            Ok((removed, total)) => {
                let mut status = self.lock();
                status.removed = removed;
                status.total = total;
            }
            Err(err) => {
                tracing::error!(%err, "library scan failed");
                self.lock().error = Some(err.to_string());
            }
        }
        let mut status = self.lock();
        status.scanning = false;
        status.finished_at = Some(iso_now());
        status.clone()
    }

    /// Starts and runs a scan, or returns the running one's status.
    pub fn scan(&self, db: &Mutex<Connection>, roots: &[String]) -> ScanStatus {
        match self.start() {
            Some(ticket) => self.run(ticket, db, roots),
            None => self.status(),
        }
    }

    fn sweep(&self, db: &Mutex<Connection>, roots: &[String]) -> rusqlite::Result<(i64, i64)> {
        let mut known = {
            let conn = lock_db(db);
            conn.execute("UPDATE tracks SET seen = 0", [])?;
            // A folder that isn't there right now — a drive not plugged in, a share not yet
            // mounted when the jukebox starts at boot — keeps its tracks, where Electron's
            // scan removed them and, with them, their queue and standby entries. A folder
            // that's gone for good is removed from the library in the admin panel.
            let mut keep =
                conn.prepare("UPDATE tracks SET seen = 1 WHERE substr(path, 1, length(?1)) = ?1")?;
            for root in roots {
                if let Err(err) = fs::read_dir(root) {
                    tracing::warn!(folder = root, %err, "library folder unavailable; keeping its tracks");
                    keep.execute([folder_prefix(root)])?;
                }
            }
            drop(keep);
            known_tracks(&conn)?
        };

        let mut files = roots.iter().flat_map(|root| Walker::new(root));
        loop {
            let batch: Vec<String> = files.by_ref().take(BATCH).collect();
            if batch.is_empty() {
                break;
            }
            let inspected: Vec<Inspected> =
                batch.par_iter().map(|path| inspect(path, &known)).collect();

            let mut counts = Counts::default();
            {
                let mut conn = lock_db(db);
                let tx = conn.transaction()?;
                for (path, file) in batch.iter().zip(inspected) {
                    if let Err(err) = apply(&tx, path, file, &mut known, &mut counts) {
                        tracing::warn!(path, %err, "failed to index");
                    }
                }
                tx.commit()?;
            }
            let mut status = self.lock();
            status.processed += batch.len() as i64;
            status.added += counts.added;
            status.updated += counts.updated;
        }

        let conn = lock_db(db);
        let removed = conn.execute("DELETE FROM tracks WHERE seen = 0", [])? as i64;
        let total = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
        Ok((removed, total))
    }
}

fn lock_db(db: &Mutex<Connection>) -> std::sync::MutexGuard<'_, Connection> {
    db.lock().expect("database lock poisoned")
}

/// Every indexed track's path, id and stored modification time.
fn known_tracks(conn: &Connection) -> rusqlite::Result<HashMap<String, (i64, i64)>> {
    let mut stmt = conn.prepare("SELECT path, id, mtime_ms FROM tracks")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
    rows.collect()
}

#[derive(Default)]
struct Counts {
    added: i64,
    updated: i64,
}

enum Inspected {
    /// Couldn't even be stat'ed. Left unmarked, so the sweep prunes it — as Electron did.
    Unreadable(std::io::Error),
    Unchanged,
    Parsed {
        mtime_ms: i64,
        meta: Box<TrackMeta>,
    },
}

/// The read-only half of indexing a file, safe to run on many files at once.
fn inspect(path: &str, known: &HashMap<String, (i64, i64)>) -> Inspected {
    let modified = match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(modified) => modified,
        Err(err) => return Inspected::Unreadable(err),
    };
    let mtime_ms = floor_millis(modified);
    if known
        .get(path)
        .is_some_and(|&(_, stored)| stored == mtime_ms)
    {
        return Inspected::Unchanged;
    }
    Inspected::Parsed {
        mtime_ms,
        meta: Box::new(metadata::read(std::path::Path::new(path))),
    }
}

/// `Math.floor(stat.mtimeMs)`, including for times before 1970.
fn floor_millis(time: std::time::SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => {
            let d = before.duration();
            let ms = d.as_millis() as i64;
            if d.as_nanos() % 1_000_000 == 0 {
                -ms
            } else {
                -ms - 1
            }
        }
    }
}

/// The writing half: the same insert, update or mark Electron's `upsertFile` made.
fn apply(
    conn: &Connection,
    path: &str,
    file: Inspected,
    known: &mut HashMap<String, (i64, i64)>,
    counts: &mut Counts,
) -> rusqlite::Result<()> {
    let (mtime_ms, meta) = match file {
        Inspected::Unreadable(err) => {
            tracing::warn!(path, %err, "failed to index");
            return Ok(());
        }
        Inspected::Unchanged => return mark_seen(conn, known[path].0),
        Inspected::Parsed { mtime_ms, meta } => (mtime_ms, meta),
    };
    let existing = known.get(path).copied();
    if let Some((id, stored)) = existing {
        // Overlapping library folders reach the same file twice; the second visit finds
        // the row the first one just wrote.
        if stored == mtime_ms {
            return mark_seen(conn, id);
        }
    }

    let art_hash = match &meta.art {
        Some(art) => {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO art (hash, mime, data) VALUES (?1, ?2, ?3)",
            )?
            .execute((&art.hash, &art.mime, &art.data))?;
            Some(art.hash.as_str())
        }
        None => None,
    };
    let fields = rusqlite::named_params! {
        ":path": path,
        ":title": meta.title,
        ":artist": meta.artist,
        ":album": meta.album,
        ":album_artist": meta.album_artist,
        ":genre": meta.genre,
        ":duration": meta.duration,
        ":track_no": meta.track_no,
        ":disc_no": meta.disc_no,
        ":year": meta.year,
        ":art_hash": art_hash,
        ":mtime_ms": mtime_ms,
    };

    let id = match existing {
        Some((id, _)) => {
            conn.prepare_cached(
                "UPDATE tracks SET title=:title, artist=:artist, album=:album,
                   album_artist=:album_artist, genre=:genre, duration=:duration,
                   track_no=:track_no, disc_no=:disc_no, year=:year, art_hash=:art_hash,
                   mtime_ms=:mtime_ms, seen=1 WHERE path=:path",
            )?
            .execute(fields)?;
            counts.updated += 1;
            id
        }
        None => {
            let added_at = iso_now();
            let mut insert = fields.to_vec();
            insert.push((":added_at", &added_at));
            conn.prepare_cached(
                "INSERT INTO tracks (path, title, artist, album, album_artist, genre, duration,
                   track_no, disc_no, year, art_hash, mtime_ms, added_at, seen)
                 VALUES (:path, :title, :artist, :album, :album_artist, :genre, :duration,
                   :track_no, :disc_no, :year, :art_hash, :mtime_ms, :added_at, 1)",
            )?
            .execute(insert.as_slice())?;
            counts.added += 1;
            conn.last_insert_rowid()
        }
    };
    known.insert(path.to_string(), (id, mtime_ms));
    Ok(())
}

fn mark_seen(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.prepare_cached("UPDATE tracks SET seen = 1 WHERE id = ?1")?
        .execute([id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn modification_times_floor_like_javascript() {
        let at = |ms: i64, extra_ns: u32| {
            if ms >= 0 {
                UNIX_EPOCH
                    + Duration::from_millis(ms as u64)
                    + Duration::from_nanos(extra_ns as u64)
            } else {
                UNIX_EPOCH - Duration::from_millis((-ms) as u64)
                    + Duration::from_nanos(extra_ns as u64)
            }
        };
        assert_eq!(
            floor_millis(at(1_787_512_517_837, 999_999)),
            1_787_512_517_837
        );
        assert_eq!(floor_millis(at(0, 0)), 0);
        assert_eq!(floor_millis(at(-1000, 0)), -1000);
        assert_eq!(
            floor_millis(at(-1000, 1)),
            -1000,
            "−999.999999 ms floors to −1000"
        );
        assert_eq!(floor_millis(at(-1001, 500_000)), -1001);
    }
}
