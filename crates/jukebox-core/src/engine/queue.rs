//! The play queue. Ports src/main/queue.ts function for function.

use rusqlite::{Connection, OptionalExtension, Row};

use super::{bans, standby, stats, Ctx, EngineError, Playing, Result, MAX_FAILURES_IN_A_ROW};
use crate::db::iso_now;
use crate::js;
use crate::library::query::track_by_id;
use crate::player::PlayerEvent;
use crate::types::{NowPlaying, PlaySource, QueueEntry, QueueState, Track};

/// "Added by" for standby playlist songs: never a guest's, never votable.
const STANDBY_IP: &str = "__standby__";
/// "Added by" for random library fills: nobody's request either, but otherwise an ordinary
/// song — counted in the stats, and guests can downvote it.
const RANDOM_IP: &str = "__random__";

struct QueueRow {
    id: i64,
    track_id: i64,
    added_by_ip: String,
    added_by_name: Option<String>,
    status: String,
}

fn queue_row(row: &Row<'_>) -> rusqlite::Result<QueueRow> {
    Ok(QueueRow {
        id: row.get("id")?,
        track_id: row.get("track_id")?,
        added_by_ip: row.get("added_by_ip")?,
        added_by_name: row.get("added_by_name")?,
        status: row.get("status")?,
    })
}

fn is_filler(ip: &str) -> bool {
    ip == STANDBY_IP || ip == RANDOM_IP
}

/// `name?.trim() || null`.
fn clean_name(name: Option<&str>) -> Option<String> {
    name.map(js::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// `new Date(Date.now() - m * 60_000).toISOString()`.
fn minutes_ago(minutes: u32) -> String {
    js::iso_from_ms(js::now_ms() - i64::from(minutes) * 60_000)
}

pub(super) fn init(ctx: &mut Ctx<'_>) -> Result<()> {
    ctx.db.execute(
        "DELETE FROM queue WHERE added_by_ip IN (?, ?)",
        [STANDBY_IP, RANDOM_IP],
    )?;
    ctx.db.execute(
        "UPDATE queue SET status = 'pending' WHERE status = 'playing'",
        [],
    )?;
    Ok(())
}

fn playing_row(db: &Connection) -> rusqlite::Result<Option<QueueRow>> {
    db.query_row(
        "SELECT * FROM queue WHERE status = 'playing' LIMIT 1",
        [],
        queue_row,
    )
    .optional()
}

fn pending_count_for(db: &Connection, ip: &str) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT COUNT(*) AS c FROM queue WHERE added_by_ip = ? AND status = 'pending'",
        [ip],
        |r| r.get(0),
    )
}

fn to_entry(db: &Connection, row: &QueueRow, for_ip: &str) -> rusqlite::Result<Option<QueueEntry>> {
    Ok(track_by_id(db, row.track_id)?.map(|track| QueueEntry {
        id: row.id,
        track,
        added_by_name: row.added_by_name.clone(),
        mine: row.added_by_ip == for_ip,
    }))
}

pub(super) fn state(ctx: &mut Ctx<'_>, for_ip: &str) -> Result<QueueState> {
    let db = ctx.db;
    let playing = playing_row(db)?;
    let pending: Vec<QueueRow> = db
        .prepare(
            "SELECT * FROM queue WHERE status = 'pending' AND added_by_ip NOT IN (?, ?)
        ORDER BY position ASC",
        )?
        .query_map([STANDBY_IP, RANDOM_IP], queue_row)?
        .collect::<rusqlite::Result<_>>()?;

    let player = ctx.player.state();
    let config = ctx.config.get();
    // Votes count only for the song being voted on. A negative threshold keeps the skip
    // but hides the count.
    let raw_threshold = config.downvote_skip_threshold;
    let votes_active = playing
        .as_ref()
        .is_some_and(|p| ctx.rt.downvote_entry == Some(p.id));
    let now_playing = NowPlaying {
        entry: match &playing {
            Some(row) => to_entry(db, row, for_ip)?,
            None => None,
        },
        position: player.position,
        duration: player.duration,
        playing: player.playing,
        is_standby: playing
            .as_ref()
            .is_some_and(|p| p.added_by_ip == STANDBY_IP),
        downvotes: if votes_active && raw_threshold > 0 {
            ctx.rt.downvoters.len() as i64
        } else {
            0
        },
        downvote_threshold: raw_threshold,
        downvoted_by_me: votes_active && ctx.rt.downvoters.contains(for_ip),
        problem: ctx.rt.problem.clone(),
    };

    let mut queue = Vec::new();
    for row in &pending {
        if let Some(entry) = to_entry(db, row, for_ip)? {
            queue.push(entry);
        }
    }
    Ok(QueueState {
        now_playing,
        queue,
        per_user_limit: config.per_user_queue_limit,
        my_queue_count: pending_count_for(db, for_ip)?,
    })
}

/// Moves on to the next song because someone or something asked to: a skip, an add, a song
/// that ended. That's a fresh start, so earlier playback failures no longer count.
pub(super) fn advance(ctx: &mut Ctx<'_>) -> Result<()> {
    ctx.rt.failures = 0;
    ctx.rt.problem = None;
    next_song(ctx)
}

fn next_song(ctx: &mut Ctx<'_>) -> Result<()> {
    // A new song clears the votes.
    ctx.rt.downvoters.clear();
    ctx.rt.downvote_entry = None;
    ctx.db
        .execute("DELETE FROM queue WHERE status = 'playing'", [])?;

    let next = ctx
        .db
        .query_row(
            "SELECT * FROM queue WHERE status = 'pending' AND added_by_ip NOT IN (?, ?)
        ORDER BY position ASC LIMIT 1",
            [STANDBY_IP, RANDOM_IP],
            queue_row,
        )
        .optional()?;

    // Guests first; then the curated playlist, the admin's explicit choice; then random
    // fill. Picking a standby track moves the playlist on, so it's picked only once.
    let mut filler = None;
    if next.is_none() {
        let config = ctx.config.get();
        let standby_id = if config.standby_enabled {
            pick_standby_track(ctx)?
        } else {
            None
        };
        if let Some(id) = standby_id {
            filler = Some((id, STANDBY_IP, PlaySource::Standby));
        } else if config.standby_random_enabled {
            if let Some(id) = pick_random_track(ctx.db)? {
                filler = Some((id, RANDOM_IP, PlaySource::Random));
            }
        }
    }

    if let Some(next) = next {
        ctx.db.execute(
            "UPDATE queue SET status = 'playing' WHERE id = ?",
            [next.id],
        )?;
        let history_id = stats::record_play(
            ctx.db,
            next.track_id,
            Some(&next.added_by_ip),
            next.added_by_name.as_deref(),
            PlaySource::Guest,
        )?;
        send_to_player(ctx, next.track_id, history_id)?;
    } else if let Some((track_id, ip, source)) = filler {
        ctx.db.execute(
            "INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
       VALUES (?, ?, NULL, ?, 0, 'playing')",
            (track_id, ip, iso_now()),
        )?;
        let history_id = stats::record_play(ctx.db, track_id, None, None, source)?;
        send_to_player(ctx, track_id, history_id)?;
    } else {
        ctx.rt.playing = None;
        ctx.player.pause();
    }
    ctx.broadcast();
    Ok(())
}

fn send_to_player(ctx: &mut Ctx<'_>, track_id: i64, history_id: i64) -> Result<()> {
    let title = track_by_id(ctx.db, track_id)?.map_or_else(|| "a song".into(), |t| t.title);
    let load = ctx.player.load(track_id, true);
    ctx.rt.playing = Some(Playing {
        load,
        history_id,
        title,
        started: false,
    });
    Ok(())
}

pub(super) fn player_event(ctx: &mut Ctx<'_>, event: PlayerEvent) -> Result<()> {
    let load = match &event {
        PlayerEvent::Started { load }
        | PlayerEvent::Ended { load }
        | PlayerEvent::Failed { load, .. } => *load,
    };
    // About a song that has since been replaced: nothing to do.
    if ctx.player.current_load() != Some(load) {
        return Ok(());
    }
    match event {
        PlayerEvent::Started { .. } => {
            if let Some(playing) = ctx.rt.playing.as_mut().filter(|p| p.load == load) {
                playing.started = true;
            }
            ctx.rt.failures = 0;
            if ctx.rt.problem.take().is_some() {
                ctx.broadcast();
            }
            Ok(())
        }
        // What player.ts's "ended" did, whoever loaded the song.
        PlayerEvent::Ended { .. } => advance(ctx),
        PlayerEvent::Failed { reason, .. } => {
            // A song the admin loaded by hand isn't the queue's to skip.
            let Some(playing) = ctx.rt.playing.take().filter(|p| p.load == load) else {
                tracing::warn!(%reason, "couldn't play a song loaded outside the queue");
                return Ok(());
            };
            tracing::warn!(title = %playing.title, %reason, "couldn't play a song; skipping it");
            // It never played, so it isn't a play.
            if !playing.started {
                ctx.db.execute(
                    "DELETE FROM play_history WHERE id = ?",
                    [playing.history_id],
                )?;
            }
            ctx.rt.failures += 1;
            if ctx.rt.failures < MAX_FAILURES_IN_A_ROW {
                return next_song(ctx);
            }
            ctx.rt.downvoters.clear();
            ctx.rt.downvote_entry = None;
            ctx.db
                .execute("DELETE FROM queue WHERE status = 'playing'", [])?;
            ctx.player.pause();
            ctx.rt.problem = Some(format!(
                "Playback stopped: the last {MAX_FAILURES_IN_A_ROW} songs couldn't be played. \
                 The last was “{}”: {reason}.",
                playing.title
            ));
            ctx.broadcast();
            Ok(())
        }
    }
}

pub(super) fn maybe_start(ctx: &mut Ctx<'_>) -> Result<()> {
    let playing = ctx
        .db
        .query_row("SELECT 1 FROM queue WHERE status = 'playing'", [], |_| {
            Ok(())
        })
        .optional()?;
    if playing.is_none() {
        advance(ctx)?;
    }
    Ok(())
}

/// A random library track for random fill, avoiding the last 50 plays so a small library
/// doesn't loop tightly — or any track at all when that leaves nothing.
fn pick_random_track(db: &Connection) -> rusqlite::Result<Option<i64>> {
    let fresh = db
        .query_row(
            "SELECT id FROM tracks
        WHERE id NOT IN (SELECT track_id FROM play_history ORDER BY id DESC LIMIT 50)
        ORDER BY RANDOM() LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if fresh.is_some() {
        return Ok(fresh);
    }
    db.query_row("SELECT id FROM tracks ORDER BY RANDOM() LIMIT 1", [], |r| {
        r.get(0)
    })
    .optional()
}

/// The next standby track, in order or shuffled.
fn pick_standby_track(ctx: &mut Ctx<'_>) -> rusqlite::Result<Option<i64>> {
    let ids = standby::track_ids(ctx.db)?;
    if ids.is_empty() {
        return Ok(None);
    }
    if ctx.config.get().standby_shuffle {
        let mut pick = ids[rand::random_range(0..ids.len())];
        // Avoid repeating the previous track when there's more than one.
        let mut tries = 0;
        while ids.len() > 1 && Some(pick) == ctx.rt.last_standby && tries < 8 {
            pick = ids[rand::random_range(0..ids.len())];
            tries += 1;
        }
        ctx.rt.last_standby = Some(pick);
        return Ok(Some(pick));
    }
    let cursor = ctx.rt.standby_cursor.map_or(0, |c| c + 1) % ids.len();
    ctx.rt.standby_cursor = Some(cursor);
    ctx.rt.last_standby = Some(ids[cursor]);
    Ok(Some(ids[cursor]))
}

/// The minimum gap between one guest's adds, from their last successful one.
fn rate_limit_error(ctx: &Ctx<'_>, ip: &str) -> rusqlite::Result<Option<String>> {
    let minutes = ctx.config.get().add_rate_limit_minutes;
    if minutes == 0 {
        return Ok(None);
    }
    let last: Option<String> = ctx
        .db
        .query_row(
            "SELECT requested_at FROM request_log WHERE requested_by_ip = ? ORDER BY id DESC LIMIT 1",
            [ip],
            |r| r.get(0),
        )
        .optional()?;
    let Some(last) = last.as_deref().and_then(js::ms_from_iso) else {
        return Ok(None);
    };
    let left_ms = i64::from(minutes) * 60_000 - (js::now_ms() - last);
    if left_ms <= 0 {
        return Ok(None);
    }
    let left = if left_ms < 60_000 {
        format!("{} s", (left_ms as f64 / 1000.0).ceil())
    } else {
        format!("{} min", (left_ms as f64 / 60_000.0).ceil())
    };
    Ok(Some(format!(
        "You're adding songs too quickly — try again in {left}."
    )))
}

/// The repeat cooldowns: a song or artist is blocked while it's playing, queued, or played
/// within its window. Each window is independent, and 0 turns it off.
fn cooldown_error(ctx: &Ctx<'_>, track: &Track) -> rusqlite::Result<Option<String>> {
    let db = ctx.db;
    let config = ctx.config.get();
    let exists = |sql: &str, params: &[&dyn rusqlite::ToSql]| {
        db.query_row(sql, params, |_| Ok(()))
            .optional()
            .map(|r| r.is_some())
    };

    if config.same_song_cooldown_minutes > 0 {
        // Matched on title and artist rather than track id: a library often holds the same
        // song several times over, and they're the same song to the room.
        if exists(
            "SELECT 1 FROM queue q JOIN tracks t ON t.id = q.track_id
          WHERE q.status IN ('playing', 'pending')
            AND t.title COLLATE NOCASE = ?
            AND IFNULL(t.artist, '') COLLATE NOCASE = IFNULL(?, '')",
            &[&track.title, &track.artist],
        )? {
            return Ok(Some(format!(
                "\"{}\" is already in the queue.",
                track.title
            )));
        }
        // History rows from before titles were logged fall back to the track id.
        if exists(
            "SELECT 1 FROM play_history
          WHERE played_at >= ?
            AND (track_id = ?
                 OR (title COLLATE NOCASE = ?
                     AND IFNULL(artist, '') COLLATE NOCASE = IFNULL(?, '')))",
            &[
                &minutes_ago(config.same_song_cooldown_minutes),
                &track.id,
                &track.title,
                &track.artist,
            ],
        )? {
            return Ok(Some(format!(
                "\"{}\" was played in the last {} min — pick something else.",
                track.title, config.same_song_cooldown_minutes
            )));
        }
    }

    if let Some(artist) = track.artist.as_deref().filter(|a| !a.is_empty()) {
        if config.same_artist_cooldown_minutes > 0 {
            if exists(
                "SELECT 1 FROM queue q JOIN tracks t ON t.id = q.track_id
         WHERE q.status IN ('playing', 'pending') AND t.artist = ? COLLATE NOCASE",
                &[&artist],
            )? {
                return Ok(Some(format!(
                    "{artist} is already in the queue — try another artist."
                )));
            }
            if exists(
                "SELECT 1 FROM play_history WHERE artist = ? COLLATE NOCASE AND played_at >= ?",
                &[&artist, &minutes_ago(config.same_artist_cooldown_minutes)],
            )? {
                return Ok(Some(format!(
                    "{artist} was played in the last {} min — try another artist.",
                    config.same_artist_cooldown_minutes
                )));
            }
        }
    }
    Ok(None)
}

pub(super) fn enqueue(
    ctx: &mut Ctx<'_>,
    track_id: i64,
    ip: &str,
    name: Option<&str>,
) -> Result<()> {
    let Some(track) = track_by_id(ctx.db, track_id)? else {
        return Err(EngineError::rejected(404, "Track not found"));
    };

    // The most decisive reason first, so a banned guest is never told merely to wait.
    if let Some(banned) = bans::ban_error(ctx.db, ip)? {
        return Err(EngineError::rejected(403, banned));
    }
    if let Some(too_soon) = rate_limit_error(ctx, ip)? {
        return Err(EngineError::rejected(429, too_soon));
    }
    if let Some(cooldown) = cooldown_error(ctx, &track)? {
        return Err(EngineError::rejected(409, cooldown));
    }
    // 0 is no limit; a negative limit applies but keeps the counter hidden.
    let raw = ctx.config.get().per_user_queue_limit;
    if raw != 0 {
        let limit = i64::from(raw.unsigned_abs());
        if pending_count_for(ctx.db, ip)? >= limit {
            return Err(EngineError::rejected(
                409,
                format!(
                    "You can have at most {limit} song{} in the queue.",
                    if limit == 1 { "" } else { "s" }
                ),
            ));
        }
    }

    let next_position: i64 = ctx.db.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 AS p FROM queue",
        [],
        |r| r.get(0),
    )?;
    ctx.db.execute(
        "INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
     VALUES (?, ?, ?, ?, ?, 'pending')",
        (track_id, ip, clean_name(name), iso_now(), next_position),
    )?;
    stats::record_request(ctx.db, track_id, ip, name)?;

    let playing: Option<String> = ctx
        .db
        .query_row(
            "SELECT added_by_ip AS ip FROM queue WHERE status = 'playing' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match playing {
        // Idle: start. Filler playing: the guest's song takes over now.
        None => advance(ctx),
        Some(ip) if is_filler(&ip) => advance(ctx),
        Some(_) => {
            ctx.broadcast();
            Ok(())
        }
    }
}

pub(super) fn remove_entry(
    ctx: &mut Ctx<'_>,
    entry_id: f64,
    ip: &str,
    is_admin: bool,
) -> Result<()> {
    let row = ctx
        .db
        .query_row("SELECT * FROM queue WHERE id = ?", [entry_id], queue_row)
        .optional()?;
    let Some(row) = row else {
        return Err(EngineError::rejected(404, "Entry not found"));
    };
    if !is_admin && row.added_by_ip != ip {
        return Err(EngineError::rejected(
            403,
            "You can only remove songs you added.",
        ));
    }
    if row.status == "playing" {
        // Removing the current song skips it.
        advance(ctx)
    } else {
        ctx.db
            .execute("DELETE FROM queue WHERE id = ?", [entry_id])?;
        ctx.broadcast();
        Ok(())
    }
}

pub(super) fn reorder(ctx: &mut Ctx<'_>, entry_id: f64, to_index: f64) -> Result<()> {
    let mut ids: Vec<i64> = ctx
        .db
        .prepare("SELECT id FROM queue WHERE status = 'pending' ORDER BY position ASC")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let Some(from) = ids.iter().position(|&id| id as f64 == entry_id) else {
        return Ok(());
    };
    let id = ids.remove(from);
    let dest = to_index.max(0.0).min(ids.len() as f64) as usize;
    ids.insert(dest, id);

    let tx = ctx.db.unchecked_transaction()?;
    for (i, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE queue SET position = ? WHERE id = ?",
            (i as i64 + 1, id),
        )?;
    }
    tx.commit()?;
    ctx.broadcast();
    Ok(())
}

pub(super) fn downvote(ctx: &mut Ctx<'_>, ip: &str, name: Option<&str>) -> Result<()> {
    let raw = ctx.config.get().downvote_skip_threshold;
    if raw == 0 {
        return Ok(());
    }
    let threshold = raw.unsigned_abs() as usize;
    // Nothing to vote on when nothing is playing, or when it's standby filler — filler
    // isn't anyone's pick.
    let Some(playing) = playing_row(ctx.db)?.filter(|p| p.added_by_ip != STANDBY_IP) else {
        return Ok(());
    };

    if ctx.rt.downvote_entry != Some(playing.id) {
        ctx.rt.downvote_entry = Some(playing.id);
        ctx.rt.downvoters.clear();
    }
    if !ctx.rt.downvoters.insert(ip.to_string()) {
        return Ok(());
    }
    stats::record_downvote(ctx.db, playing.track_id, ip, name)?;

    if ctx.rt.downvoters.len() >= threshold {
        advance(ctx)
    } else {
        ctx.broadcast();
        Ok(())
    }
}

pub(super) fn clear_pending(ctx: &mut Ctx<'_>) -> Result<()> {
    ctx.db
        .execute("DELETE FROM queue WHERE status = 'pending'", [])?;
    ctx.broadcast();
    Ok(())
}
