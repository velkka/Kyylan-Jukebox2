ALTER TABLE play_history ADD COLUMN source TEXT NOT NULL DEFAULT 'guest';
   UPDATE play_history SET source = 'standby' WHERE is_standby = 1;