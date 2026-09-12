import { createHash } from 'node:crypto'
import { getDb } from './db'
import { getTrackById } from './library'
import { listBans } from './bans'
import type { PlayHistoryItem, PlaySource, StatsResponse, TrackStat, UserStat } from '@shared/types'

/**
 * Play / request / downvote logging, and the aggregates the admin panel shows.
 *
 * Titles and artists are copied into the logs rather than joined at read time,
 * so history survives a rescan pruning the track. The library is still joined
 * opportunistically for cover art, which degrades to null once a track is gone.
 */

const now = (): string => new Date().toISOString()

/** Logs a successful guest add. Counts even if the song is removed before it plays. */
export function recordRequest(trackId: number, ip: string, name?: string | null): void {
  const track = getTrackById(trackId)
  getDb()
    .prepare(
      `INSERT INTO request_log (track_id, title, artist, requested_by_ip, requested_by_name, requested_at)
       VALUES (?, ?, ?, ?, ?, ?)`
    )
    .run(trackId, track?.title ?? null, track?.artist ?? null, ip, name?.trim() || null, now())
}

/**
 * Logs a song going on air. Drives both the repeat cooldowns and the play stats.
 * `is_standby` is written alongside `source` purely to keep the older column
 * truthful; every read goes through `source`.
 */
export function recordPlay(
  trackId: number,
  requestedByIp: string | null,
  requestedByName: string | null,
  source: PlaySource
): void {
  const track = getTrackById(trackId)
  getDb()
    .prepare(
      `INSERT INTO play_history
         (track_id, title, artist, requested_by_ip, requested_by_name, is_standby, source, played_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`
    )
    .run(
      trackId,
      track?.title ?? null,
      track?.artist ?? null,
      requestedByIp,
      requestedByName,
      source === 'standby' ? 1 : 0,
      source,
      now()
    )
}

/** Logs one guest's downvote of one song. Callers dedupe repeat votes. */
export function recordDownvote(trackId: number, ip: string, name?: string | null): void {
  const track = getTrackById(trackId)
  getDb()
    .prepare(
      `INSERT INTO downvote_log (track_id, title, artist, voter_ip, voter_name, voted_at)
       VALUES (?, ?, ?, ?, ?, ?)`
    )
    .run(trackId, track?.title ?? null, track?.artist ?? null, ip, name?.trim() || null, now())
}

interface HistoryRow {
  id: number
  trackId: number
  title: string
  artist: string | null
  artHash: string | null
  requestedByName: string | null
  source: PlaySource
  playedAt: string
}

/** Most recently played first. */
export function playHistory(limit = 100, offset = 0): PlayHistoryItem[] {
  const rows = getDb()
    .prepare(
      `SELECT h.id                                      AS id,
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
        LIMIT ? OFFSET ?`
    )
    .all(limit, offset) as HistoryRow[]
  return rows
}

function topFrom(
  table: 'play_history' | 'downvote_log',
  limit: number,
  where = ''
): TrackStat[] {
  return getDb()
    .prepare(
      `SELECT x.track_id                                AS trackId,
              COALESCE(MAX(x.title), MAX(t.title), 'Unknown') AS title,
              COALESCE(MAX(x.artist), MAX(t.artist))    AS artist,
              COUNT(*)                                  AS count
         FROM ${table} x
         LEFT JOIN tracks t ON t.id = x.track_id
         ${where}
        GROUP BY x.track_id
        ORDER BY count DESC, title ASC
        LIMIT ?`
    )
    .all(limit) as TrackStat[]
}

interface CountRow {
  ip: string
  name: string | null
  c: number
}

/** Per-guest request and downvote counts, keyed by IP and merged by it. */
function userStats(): UserStat[] {
  const db = getDb()
  // The correlated subquery takes the latest non-null hostname seen for that IP,
  // since a device may have been logged as a bare IP before its name resolved.
  const requests = db
    .prepare(
      `SELECT r.requested_by_ip AS ip, COUNT(*) AS c,
              (SELECT r2.requested_by_name FROM request_log r2
                WHERE r2.requested_by_ip = r.requested_by_ip
                  AND r2.requested_by_name IS NOT NULL
                ORDER BY r2.id DESC LIMIT 1) AS name
         FROM request_log r GROUP BY r.requested_by_ip`
    )
    .all() as CountRow[]
  const downvotes = db
    .prepare(
      `SELECT d.voter_ip AS ip, COUNT(*) AS c,
              (SELECT d2.voter_name FROM downvote_log d2
                WHERE d2.voter_ip = d.voter_ip AND d2.voter_name IS NOT NULL
                ORDER BY d2.id DESC LIMIT 1) AS name
         FROM downvote_log d GROUP BY d.voter_ip`
    )
    .all() as CountRow[]

  const users = new Map<string, UserStat>()
  const upsert = (ip: string, name: string | null): UserStat => {
    const existing = users.get(ip)
    if (existing) {
      existing.name ??= name
      return existing
    }
    const created: UserStat = {
      id: ip,
      ip,
      name,
      requests: 0,
      downvotes: 0,
      banned: false,
      bannedUntil: null
    }
    users.set(ip, created)
    return created
  }
  for (const r of requests) upsert(r.ip, r.name).requests = r.c
  for (const d of downvotes) upsert(d.ip, d.name).downvotes = d.c
  // A guest can be banned before they ever add anything, so bans contribute
  // rows of their own rather than only annotating existing ones.
  for (const b of listBans()) {
    const u = upsert(b.ip, b.name)
    u.banned = true
    u.bannedUntil = b.expiresAt
  }

  return [...users.values()].sort(
    (a, b) => b.requests - a.requests || b.downvotes - a.downvotes
  )
}

const countOf = (table: string, where = ''): number =>
  (getDb().prepare(`SELECT COUNT(*) AS c FROM ${table} ${where}`).get() as { c: number }).c

/**
 * Strips everything a guest has no business seeing: other guests' addresses and
 * who is currently blocked. The list key becomes an opaque digest so the UI
 * still has something stable to key rows on.
 */
function redact(users: UserStat[]): UserStat[] {
  return users.map((u) => ({
    ...u,
    id: createHash('sha1').update(u.id).digest('hex').slice(0, 8),
    ip: null,
    banned: false,
    bannedUntil: null
  }))
}

/**
 * Standby playlist filler is excluded throughout — it is the admin's playlist,
 * not a record of what the party asked for. Those plays are still logged,
 * because the repeat cooldowns read the same table. Random library fills do
 * count: they are ordinary songs that anyone can downvote off.
 */
export function buildStats(historyLimit = 100, topLimit = 10, forAdmin = false): StatsResponse {
  const users = userStats()
  return {
    history: playHistory(historyLimit),
    topPlayed: topFrom('play_history', topLimit, "WHERE x.source != 'standby'"),
    topDownvoted: topFrom('downvote_log', topLimit),
    users: forAdmin ? users : redact(users),
    totals: {
      plays: countOf('play_history', "WHERE source != 'standby'"),
      requests: countOf('request_log'),
      downvotes: countOf('downvote_log')
    }
  }
}

/**
 * Wipes every log. Note this also clears the repeat cooldowns, since they read
 * the same play history — which is what you want between parties.
 */
export function clearStats(): void {
  const db = getDb()
  db.transaction(() => {
    db.prepare('DELETE FROM play_history').run()
    db.prepare('DELETE FROM request_log').run()
    db.prepare('DELETE FROM downvote_log').run()
  })()
}
