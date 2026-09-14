//! The standby playlist: filler for when the guest queue is empty. Ports src/main/standby.ts.

use rusqlite::Connection;

use super::{EngineError, Result};
use crate::db::iso_now;
use crate::library::query::track_by_id;
use crate::types::StandbyEntry;

/// The playlist's track ids, in order.
pub(super) fn track_ids(db: &Connection) -> rusqlite::Result<Vec<i64>> {
    db.prepare("SELECT track_id FROM standby ORDER BY position")?
        .query_map([], |r| r.get(0))?
        .collect()
}

pub(super) fn list(db: &Connection) -> Result<Vec<StandbyEntry>> {
    let rows: Vec<(i64, i64)> = db
        .prepare("SELECT id, track_id FROM standby ORDER BY position")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut entries = Vec::new();
    for (id, track_id) in rows {
        if let Some(track) = track_by_id(db, track_id)? {
            entries.push(StandbyEntry { id, track });
        }
    }
    Ok(entries)
}

pub(super) fn add(db: &Connection, track_id: i64) -> Result<()> {
    if track_by_id(db, track_id)?.is_none() {
        return Err(EngineError::rejected(400, "Track not found"));
    }
    let position: i64 = db.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 AS p FROM standby",
        [],
        |r| r.get(0),
    )?;
    db.execute(
        "INSERT INTO standby (track_id, position, added_at) VALUES (?, ?, ?)",
        (track_id, position, iso_now()),
    )?;
    Ok(())
}

pub(super) fn remove(db: &Connection, id: f64) -> Result<()> {
    db.execute("DELETE FROM standby WHERE id = ?", [id])?;
    Ok(())
}

pub(super) fn clear(db: &Connection) -> Result<()> {
    db.execute("DELETE FROM standby", [])?;
    Ok(())
}
