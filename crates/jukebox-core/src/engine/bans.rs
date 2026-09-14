//! Guests barred from adding songs, by address. Ports src/main/bans.ts.
//!
//! A ban blocks new adds only: songs the guest already queued keep their place, and they can
//! still downvote.

use rusqlite::{Connection, OptionalExtension, Row};

use super::Result;
use crate::db::iso_now;
use crate::js;
use crate::types::BanEntry;

/// Drops bans whose time is up, so every read sees only active ones.
fn prune_expired(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM bans WHERE expires_at IS NOT NULL AND expires_at <= ?",
        [iso_now()],
    )?;
    Ok(())
}

fn ban_row(row: &Row<'_>) -> rusqlite::Result<BanEntry> {
    Ok(BanEntry {
        ip: row.get(0)?,
        name: row.get(1)?,
        banned_at: row.get(2)?,
        expires_at: row.get(3)?,
    })
}

pub(super) fn list(db: &Connection) -> Result<Vec<BanEntry>> {
    prune_expired(db)?;
    Ok(db
        .prepare(
            "SELECT ip, name, banned_at AS bannedAt, expires_at AS expiresAt
         FROM bans ORDER BY (expires_at IS NOT NULL), expires_at ASC, banned_at DESC",
        )?
        .query_map([], ban_row)?
        .collect::<rusqlite::Result<_>>()?)
}

fn active_ban(db: &Connection, ip: &str) -> rusqlite::Result<Option<BanEntry>> {
    prune_expired(db)?;
    db.query_row(
        "SELECT ip, name, banned_at AS bannedAt, expires_at AS expiresAt
           FROM bans WHERE ip = ?",
        [ip],
        ban_row,
    )
    .optional()
}

/// `minutes` of 0 bans permanently. Banning again replaces the old ban.
pub(super) fn ban(
    db: &Connection,
    ip: &str,
    name: Option<&str>,
    minutes: i64,
) -> Result<Vec<BanEntry>> {
    let expires_at = (minutes > 0).then(|| js::iso_from_ms(js::now_ms() + minutes * 60_000));
    let name = name.map(js::trim).filter(|n| !n.is_empty());
    db.execute(
        "INSERT INTO bans (ip, name, banned_at, expires_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(ip) DO UPDATE SET
         name = COALESCE(excluded.name, bans.name),
         banned_at = excluded.banned_at,
         expires_at = excluded.expires_at",
        (ip, name, iso_now(), expires_at),
    )?;
    list(db)
}

pub(super) fn unban(db: &Connection, ip: &str) -> Result<Vec<BanEntry>> {
    db.execute("DELETE FROM bans WHERE ip = ?", [ip])?;
    list(db)
}

/// "x min", "x hours" or "x days" left on a ban.
fn remaining(expires_at: &str) -> String {
    let left_ms = js::ms_from_iso(expires_at).map_or(f64::NAN, |t| (t - js::now_ms()) as f64);
    let minutes = (left_ms / 60_000.0).ceil().max(1.0);
    if minutes < 60.0 {
        return format!("{minutes} min");
    }
    let hours = (minutes / 60.0).round();
    if hours < 48.0 {
        return format!("{hours} hour{}", if hours == 1.0 { "" } else { "s" });
    }
    format!("{} days", (hours / 24.0).round())
}

/// Why this guest may not add songs, or `None` when they may.
pub(super) fn ban_error(db: &Connection, ip: &str) -> rusqlite::Result<Option<String>> {
    Ok(active_ban(db, ip)?.map(|ban| match ban.expires_at {
        None => "You have been blocked from adding songs.".to_string(),
        Some(expires_at) => format!(
            "You have been blocked from adding songs for another {}.",
            remaining(&expires_at)
        ),
    }))
}
