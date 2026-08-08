import Database from 'better-sqlite3'
import { app } from 'electron'
import { join } from 'node:path'

let db: Database.Database | null = null

// Ordered, append-only list of schema migrations. Later milestones add the
// `tracks` (M3) and `queue` (M5) tables by appending here — never edit an
// existing entry, always add a new one.
const MIGRATIONS: string[] = [
  // 1: key/value scratch table (host/library metadata, last-scan timestamps, …)
  `CREATE TABLE meta (
     key   TEXT PRIMARY KEY,
     value TEXT
   );`,

  // 2: the music library. `seen` is a sweep marker used to prune deleted files;
  // `mtime_ms` lets rescans skip unchanged files.
  `CREATE TABLE tracks (
     id           INTEGER PRIMARY KEY AUTOINCREMENT,
     path         TEXT UNIQUE NOT NULL,
     title        TEXT NOT NULL,
     artist       TEXT,
     album        TEXT,
     album_artist TEXT,
     genre        TEXT,
     duration     REAL,
     track_no     INTEGER,
     disc_no      INTEGER,
     year         INTEGER,
     art_hash     TEXT,
     mtime_ms     INTEGER NOT NULL,
     added_at     TEXT NOT NULL,
     seen         INTEGER NOT NULL DEFAULT 1
   );
   CREATE INDEX idx_tracks_artist ON tracks(artist COLLATE NOCASE);
   CREATE INDEX idx_tracks_album  ON tracks(album COLLATE NOCASE);`,

  // 3: full-text search over title/artist/album, kept in sync via triggers.
  `CREATE VIRTUAL TABLE tracks_fts USING fts5(
     title, artist, album,
     content='tracks', content_rowid='id'
   );
   CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
     INSERT INTO tracks_fts(rowid, title, artist, album)
       VALUES (new.id, new.title, new.artist, new.album);
   END;
   CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
     INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
       VALUES ('delete', old.id, old.title, old.artist, old.album);
   END;
   CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album ON tracks BEGIN
     INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
       VALUES ('delete', old.id, old.title, old.artist, old.album);
     INSERT INTO tracks_fts(rowid, title, artist, album)
       VALUES (new.id, new.title, new.artist, new.album);
   END;`,

  // 4: cover art, deduped by content hash and shared across tracks of an album.
  `CREATE TABLE art (
     hash TEXT PRIMARY KEY,
     mime TEXT NOT NULL,
     data BLOB NOT NULL
   );`,

  // 5: the play queue. One row is status='playing' (now playing); the rest are
  // 'pending', ordered by `position`. Entries auto-remove if their track is
  // pruned from the library (FK cascade).
  `CREATE TABLE queue (
     id            INTEGER PRIMARY KEY AUTOINCREMENT,
     track_id      INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
     added_by_ip   TEXT NOT NULL,
     added_by_name TEXT,
     added_at      TEXT NOT NULL,
     position      INTEGER NOT NULL,
     status        TEXT NOT NULL DEFAULT 'pending'
   );
   CREATE INDEX idx_queue_status_pos ON queue(status, position);`
]

function migrate(d: Database.Database): void {
  d.exec(`CREATE TABLE IF NOT EXISTS _migrations (
    id         INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
  )`)
  const applied = new Set(
    (d.prepare('SELECT id FROM _migrations').all() as { id: number }[]).map((r) => r.id)
  )
  const insert = d.prepare('INSERT INTO _migrations (id, applied_at) VALUES (?, ?)')
  const now = new Date().toISOString()
  const run = d.transaction(() => {
    MIGRATIONS.forEach((sql, index) => {
      const id = index + 1
      if (!applied.has(id)) {
        d.exec(sql)
        insert.run(id, now)
      }
    })
  })
  run()
}

/** Opens (once) and returns the shared SQLite handle. */
export function getDb(): Database.Database {
  if (db) return db
  const path = join(app.getPath('userData'), 'jukebox.db')
  const handle = new Database(path)
  handle.pragma('journal_mode = WAL')
  handle.pragma('foreign_keys = ON')
  migrate(handle)
  console.log(`[db] opened ${path} (migrations: ${MIGRATIONS.length})`)
  db = handle
  return handle
}

export function closeDb(): void {
  db?.close()
  db = null
}
