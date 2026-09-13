CREATE TABLE bans (
     ip         TEXT PRIMARY KEY,
     name       TEXT,
     banned_at  TEXT NOT NULL,
     expires_at TEXT
   );