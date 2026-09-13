CREATE TABLE play_history (
     id        INTEGER PRIMARY KEY AUTOINCREMENT,
     track_id  INTEGER NOT NULL,
     artist    TEXT,
     played_at TEXT NOT NULL
   );
   CREATE INDEX idx_history_played_at ON play_history(played_at);