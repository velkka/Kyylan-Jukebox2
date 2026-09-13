CREATE TABLE tracks (
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
   CREATE INDEX idx_tracks_album  ON tracks(album COLLATE NOCASE);