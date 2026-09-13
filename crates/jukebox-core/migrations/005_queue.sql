CREATE TABLE queue (
     id            INTEGER PRIMARY KEY AUTOINCREMENT,
     track_id      INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
     added_by_ip   TEXT NOT NULL,
     added_by_name TEXT,
     added_at      TEXT NOT NULL,
     position      INTEGER NOT NULL,
     status        TEXT NOT NULL DEFAULT 'pending'
   );
   CREATE INDEX idx_queue_status_pos ON queue(status, position);