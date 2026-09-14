//! Play, request and downvote logs, and the history and stats built from them. Ports
//! src/main/stats.ts.
//!
//! Titles and artists are copied into the logs, so history survives a rescan pruning the
//! track; the library is still joined for cover art, which is null once a track is gone.

use rusqlite::Connection;
use sha1::{Digest, Sha1};

use super::{bans, Result};
use crate::db::iso_now;
use crate::js;
use crate::library::query::track_by_id;
use crate::types::{PlayHistoryItem, PlaySource, StatsResponse, StatsTotals, TrackStat, UserStat};

fn clean_name(name: Option<&str>) -> Option<&str> {
    name.map(js::trim).filter(|n| !n.is_empty())
}

/// A guest's add, counted even if the song is removed before it plays.
pub(super) fn record_request(
    db: &Connection,
    track_id: i64,
    ip: &str,
    name: Option<&str>,
) -> rusqlite::Result<()> {
    let track = track_by_id(db, track_id)?;
    db.execute(
        "INSERT INTO request_log (track_id, title, artist, requested_by_ip, requested_by_name, requested_at)
       VALUES (?, ?, ?, ?, ?, ?)",
        (
            track_id,
            track.as_ref().map(|t| &t.title),
            track.as_ref().and_then(|t| t.artist.as_ref()),
            ip,
            clean_name(name),
            iso_now(),
        ),
    )?;
    Ok(())
}

/// A song going on air. `is_standby` is kept truthful for the older column; reads use
/// `source`.
pub(super) fn record_play(
    db: &Connection,
    track_id: i64,
    requested_by_ip: Option<&str>,
    requested_by_name: Option<&str>,
    source: PlaySource,
) -> rusqlite::Result<()> {
    let track = track_by_id(db, track_id)?;
    let source_name = match source {
        PlaySource::Guest => "guest",
        PlaySource::Standby => "standby",
        PlaySource::Random => "random",
    };
    db.execute(
        "INSERT INTO play_history
         (track_id, title, artist, requested_by_ip, requested_by_name, is_standby, source, played_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (
            track_id,
            track.as_ref().map(|t| &t.title),
            track.as_ref().and_then(|t| t.artist.as_ref()),
            requested_by_ip,
            requested_by_name,
            i64::from(source == PlaySource::Standby),
            source_name,
            iso_now(),
        ),
    )?;
    Ok(())
}

/// One guest's downvote of one song; the caller has already deduplicated.
pub(super) fn record_downvote(
    db: &Connection,
    track_id: i64,
    ip: &str,
    name: Option<&str>,
) -> rusqlite::Result<()> {
    let track = track_by_id(db, track_id)?;
    db.execute(
        "INSERT INTO downvote_log (track_id, title, artist, voter_ip, voter_name, voted_at)
       VALUES (?, ?, ?, ?, ?, ?)",
        (
            track_id,
            track.as_ref().map(|t| &t.title),
            track.as_ref().and_then(|t| t.artist.as_ref()),
            ip,
            clean_name(name),
            iso_now(),
        ),
    )?;
    Ok(())
}

fn play_history(db: &Connection, limit: i64) -> rusqlite::Result<Vec<PlayHistoryItem>> {
    db.prepare(
        "SELECT h.id                                      AS id,
              h.track_id                                AS trackId,
              COALESCE(h.title, t.title, 'Unknown')     AS title,
              COALESCE(h.artist, t.artist)              AS artist,
              t.art_hash                                AS artHash,
              h.requested_by_name                       AS requestedByName,
              h.source                                  AS source,
              h.played_at                               AS playedAt
         FROM play_history h
         LEFT JOIN tracks t ON t.id = h.track_id
        WHERE h.source != 'standby'
        ORDER BY h.id DESC
        LIMIT ? OFFSET ?",
    )?
    .query_map((limit, 0), |r| {
        let source: String = r.get(6)?;
        Ok(PlayHistoryItem {
            id: r.get(0)?,
            track_id: r.get(1)?,
            title: r.get(2)?,
            artist: r.get(3)?,
            art_hash: r.get(4)?,
            requested_by_name: r.get(5)?,
            source: serde_json::from_value(serde_json::Value::String(source))
                .unwrap_or(PlaySource::Guest),
            played_at: r.get(7)?,
        })
    })?
    .collect()
}

fn top_from(
    db: &Connection,
    table: &str,
    limit: i64,
    filter: &str,
) -> rusqlite::Result<Vec<TrackStat>> {
    db.prepare(&format!(
        "SELECT x.track_id                                AS trackId,
              COALESCE(MAX(x.title), MAX(t.title), 'Unknown') AS title,
              COALESCE(MAX(x.artist), MAX(t.artist))    AS artist,
              COUNT(*)                                  AS count
         FROM {table} x
         LEFT JOIN tracks t ON t.id = x.track_id
         {filter}
        GROUP BY x.track_id
        ORDER BY count DESC, title ASC
        LIMIT ?"
    ))?
    .query_map([limit], |r| {
        Ok(TrackStat {
            track_id: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            count: r.get(3)?,
        })
    })?
    .collect()
}

/// Per-guest request and downvote counts, merged by address.
fn user_stats(db: &Connection) -> Result<Vec<UserStat>> {
    // The latest hostname seen for the address: a device may have been logged as a bare
    // address before its name resolved.
    let counts = |sql: &str| -> rusqlite::Result<Vec<(String, Option<String>, i64)>> {
        db.prepare(sql)?
            .query_map([], |r| Ok((r.get("ip")?, r.get("name")?, r.get("c")?)))?
            .collect()
    };
    let requests = counts(
        "SELECT r.requested_by_ip AS ip, COUNT(*) AS c,
              (SELECT r2.requested_by_name FROM request_log r2
                WHERE r2.requested_by_ip = r.requested_by_ip
                  AND r2.requested_by_name IS NOT NULL
                ORDER BY r2.id DESC LIMIT 1) AS name
         FROM request_log r GROUP BY r.requested_by_ip",
    )?;
    let downvotes = counts(
        "SELECT d.voter_ip AS ip, COUNT(*) AS c,
              (SELECT d2.voter_name FROM downvote_log d2
                WHERE d2.voter_ip = d.voter_ip AND d2.voter_name IS NOT NULL
                ORDER BY d2.id DESC LIMIT 1) AS name
         FROM downvote_log d GROUP BY d.voter_ip",
    )?;

    let mut users: Vec<UserStat> = Vec::new();
    let upsert = |users: &mut Vec<UserStat>, ip: String, name: Option<String>| -> usize {
        if let Some(i) = users.iter().position(|u| u.id == ip) {
            if users[i].name.is_none() {
                users[i].name = name;
            }
            return i;
        }
        users.push(UserStat {
            id: ip.clone(),
            ip: Some(ip),
            name,
            requests: 0,
            downvotes: 0,
            banned: false,
            banned_until: None,
        });
        users.len() - 1
    };
    for (ip, name, c) in requests {
        let i = upsert(&mut users, ip, name);
        users[i].requests = c;
    }
    for (ip, name, c) in downvotes {
        let i = upsert(&mut users, ip, name);
        users[i].downvotes = c;
    }
    // A guest can be banned before adding anything, so bans add rows of their own.
    for ban in bans::list(db)? {
        let i = upsert(&mut users, ban.ip, ban.name);
        users[i].banned = true;
        users[i].banned_until = ban.expires_at;
    }
    users.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then(b.downvotes.cmp(&a.downvotes))
    });
    Ok(users)
}

fn count(db: &Connection, table: &str, filter: &str) -> rusqlite::Result<i64> {
    db.query_row(
        &format!("SELECT COUNT(*) AS c FROM {table} {filter}"),
        [],
        |r| r.get(0),
    )
}

/// Strips what a guest has no business seeing — other guests' addresses and who is blocked
/// — keying each row by an opaque digest instead.
fn redact(users: Vec<UserStat>) -> Vec<UserStat> {
    users
        .into_iter()
        .map(|u| {
            let digest: String = Sha1::digest(u.id.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            UserStat {
                id: digest[..8].to_string(),
                ip: None,
                banned: false,
                banned_until: None,
                ..u
            }
        })
        .collect()
}

/// Standby filler is left out throughout: it's the admin's playlist, not what the party
/// asked for. Random fills count.
pub(super) fn build(
    db: &Connection,
    history_limit: i64,
    top_limit: i64,
    for_admin: bool,
) -> Result<StatsResponse> {
    let users = user_stats(db)?;
    Ok(StatsResponse {
        history: play_history(db, history_limit)?,
        top_played: top_from(db, "play_history", top_limit, "WHERE x.source != 'standby'")?,
        top_downvoted: top_from(db, "downvote_log", top_limit, "")?,
        users: if for_admin { users } else { redact(users) },
        totals: StatsTotals {
            plays: count(db, "play_history", "WHERE source != 'standby'")?,
            requests: count(db, "request_log", "")?,
            downvotes: count(db, "downvote_log", "")?,
        },
    })
}

pub(super) fn clear(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    tx.execute("DELETE FROM play_history", [])?;
    tx.execute("DELETE FROM request_log", [])?;
    tx.execute("DELETE FROM downvote_log", [])?;
    tx.commit()?;
    Ok(())
}
