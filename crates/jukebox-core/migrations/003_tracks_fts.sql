CREATE VIRTUAL TABLE tracks_fts USING fts5(
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
   END;