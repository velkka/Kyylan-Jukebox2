ALTER TABLE play_history ADD COLUMN title TEXT;
   ALTER TABLE play_history ADD COLUMN requested_by_ip TEXT;
   ALTER TABLE play_history ADD COLUMN requested_by_name TEXT;
   ALTER TABLE play_history ADD COLUMN is_standby INTEGER NOT NULL DEFAULT 0;

   CREATE TABLE request_log (
     id                INTEGER PRIMARY KEY AUTOINCREMENT,
     track_id          INTEGER NOT NULL,
     title             TEXT,
     artist            TEXT,
     requested_by_ip   TEXT NOT NULL,
     requested_by_name TEXT,
     requested_at      TEXT NOT NULL
   );
   CREATE INDEX idx_requests_ip ON request_log(requested_by_ip);

   CREATE TABLE downvote_log (
     id         INTEGER PRIMARY KEY AUTOINCREMENT,
     track_id   INTEGER NOT NULL,
     title      TEXT,
     artist     TEXT,
     voter_ip   TEXT NOT NULL,
     voter_name TEXT,
     voted_at   TEXT NOT NULL
   );
   CREATE INDEX idx_downvotes_ip ON downvote_log(voter_ip);