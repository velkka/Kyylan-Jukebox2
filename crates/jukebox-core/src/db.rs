//! The SQLite database, and the migrations that shape it.
//!
//! Mirrors src/main/db.ts to the byte. SQLite stores each `CREATE` statement's text in
//! `sqlite_master`, so running the same SQL makes a database created here structurally
//! identical to one Electron built up release by release — and either build can open
//! the other's.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

/// Every migration's SQL as stored in the repo, oldest first; id = position + 1.
/// Append-only: never edit an entry. The files are extracted verbatim from db.ts, and a
/// test holds them to it. Read them through [`migrations`], never directly.
const RAW_MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_meta.sql"),
    include_str!("../migrations/002_tracks.sql"),
    include_str!("../migrations/003_tracks_fts.sql"),
    include_str!("../migrations/004_art.sql"),
    include_str!("../migrations/005_queue.sql"),
    include_str!("../migrations/006_standby.sql"),
    include_str!("../migrations/007_play_history.sql"),
    include_str!("../migrations/008_history_stats.sql"),
    include_str!("../migrations/009_bans.sql"),
    include_str!("../migrations/010_play_source.sql"),
];

/// Every migration's SQL, oldest first, with line endings normalized to LF.
///
/// JavaScript normalizes CRLF inside template literals, so db.ts's SQL is LF even when Git
/// checks it out with CRLF on Windows. `include_str!` does no such thing, and the text ends
/// up verbatim in `sqlite_master` — so without this, a Windows build would create databases
/// whose schema text differs from Electron's.
pub fn migrations() -> Vec<String> {
    RAW_MIGRATIONS
        .iter()
        .map(|sql| sql.replace("\r\n", "\n"))
        .collect()
}

/// db.ts's bookkeeping table, whitespace included, since the text lands in `sqlite_master`.
pub const MIGRATIONS_TABLE: &str = "CREATE TABLE IF NOT EXISTS _migrations (
    id         INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
  )";

pub fn latest_migration() -> u32 {
    RAW_MIGRATIONS.len() as u32
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Migrations applied by this open, if any.
    pub applied: Vec<u32>,
    /// The newest migration this build knows.
    pub latest: u32,
    /// The newest migration the database records. Above `latest` means a newer version
    /// last opened it; like db.ts, this build carries on — which keeps a rollback working —
    /// and the caller can warn.
    pub recorded: u32,
}

/// Opens the app database the way db.ts does — WAL, foreign keys on — and brings it up to
/// date. Creates the file if needed.
pub fn open(path: &Path) -> Result<(Connection, MigrationReport), DbError> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let report = migrate(&mut conn)?;
    Ok((conn, report))
}

/// Opens a database for reading only: nothing is migrated, created or written.
///
/// A copy of a WAL-mode database made without its `-shm` file can't be opened read-only
/// in the normal way, so that case falls back to SQLite's `immutable` mode — safe for a
/// copy nobody else is writing to, which is the only way that case arises.
pub fn open_read_only(path: &Path) -> Result<Connection, DbError> {
    let read_only = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(conn) = Connection::open_with_flags(path, read_only) {
        if conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_ok()
        {
            return Ok(conn);
        }
    }
    let uri = file_uri(path, "immutable=1");
    Ok(Connection::open_with_flags(
        uri,
        read_only | OpenFlags::SQLITE_OPEN_URI,
    )?)
}

/// A SQLite `file:` URI for a path: forward slashes, a leading slash before a Windows drive
/// letter, and the characters the URI parser treats specially percent-encoded.
fn file_uri(path: &Path, query: &str) -> String {
    let mut encoded = String::new();
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => encoded.push('/'),
            '%' => encoded.push_str("%25"),
            ' ' => encoded.push_str("%20"),
            '?' => encoded.push_str("%3f"),
            '#' => encoded.push_str("%23"),
            c => encoded.push(c),
        }
    }
    if !encoded.starts_with('/') {
        encoded.insert(0, '/');
    }
    format!("file://{encoded}?{query}")
}

/// The highest migration recorded in a database, or 0 for a fresh one.
pub fn current_migration(conn: &Connection) -> Result<u32, DbError> {
    let has_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_migrations')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(0);
    }
    let max: Option<u32> = conn.query_row("SELECT max(id) FROM _migrations", [], |r| r.get(0))?;
    Ok(max.unwrap_or(0))
}

/// Applies every migration the database hasn't seen, in one transaction, stamping them all
/// with a single timestamp — exactly as db.ts's `migrate` does.
pub fn migrate(conn: &mut Connection) -> Result<MigrationReport, DbError> {
    conn.execute_batch(MIGRATIONS_TABLE)?;

    let applied_ids: std::collections::HashSet<u32> = conn
        .prepare("SELECT id FROM _migrations")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let pending: Vec<u32> = (1..=latest_migration())
        .filter(|id| !applied_ids.contains(id))
        .collect();

    if !pending.is_empty() {
        let sql = migrations();
        let now = iso_now();
        let tx = conn.transaction()?;
        for &id in &pending {
            tx.execute_batch(&sql[id as usize - 1])?;
            tx.execute(
                "INSERT INTO _migrations (id, applied_at) VALUES (?1, ?2)",
                (id, &now),
            )?;
        }
        tx.commit()?;
    }
    let recorded = current_migration(conn)?;
    Ok(MigrationReport {
        applied: pending,
        latest: latest_migration(),
        recorded,
    })
}

/// `new Date().toISOString()`: UTC, millisecond precision, `Z` suffix.
pub fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
