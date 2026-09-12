import { getDb } from './db'
import { loadConfig } from './config'
import { getTrackById } from './library'
import { getState, loadTrack, pause } from './player'
import { broadcastQueue } from './realtime'
import { standbyTrackIds } from './standby'
import { recordDownvote, recordPlay, recordRequest } from './stats'
import { banError } from './bans'
import { NowPlaying, QueueEntry, QueueState, Track } from '@shared/types'

// Sentinel "added by" for standby (filler) tracks, so they never count against a
// guest's limit and are visually distinguished from guest songs.
const STANDBY_IP = '__standby__'

// Cursor for sequential standby playback; last id to avoid immediate repeats when shuffling.
let standbyCursor = -1
let lastStandbyTrack: number | null = null

// Downvotes for the current song: the playing entry id being voted on, and the
// set of guest IPs that have downvoted it. Reset whenever the song changes.
let downvoteEntryId: number | null = null
let downvoters = new Set<string>()

const minutesAgo = (m: number): string => new Date(Date.now() - m * 60_000).toISOString()

/**
 * Minimum gap between one guest's adds. Measured from their last successful
 * add, so it throttles how often they may queue rather than how many at once
 * (that is `perUserQueueLimit`). 0 is off.
 */
function rateLimitError(ip: string): string | null {
  const minutes = loadConfig().addRateLimitMinutes
  if (minutes <= 0) return null

  const last = getDb()
    .prepare(
      'SELECT requested_at FROM request_log WHERE requested_by_ip = ? ORDER BY id DESC LIMIT 1'
    )
    .get(ip) as { requested_at: string } | undefined
  if (!last) return null

  const leftMs = minutes * 60_000 - (Date.now() - new Date(last.requested_at).getTime())
  if (leftMs <= 0) return null
  const left =
    leftMs < 60_000 ? `${Math.ceil(leftMs / 1000)} s` : `${Math.ceil(leftMs / 60_000)} min`
  return `You're adding songs too quickly — try again in ${left}.`
}

/**
 * Repeat cooldowns. A song/artist is blocked while it is still in rotation:
 * already playing or queued (it would come round again almost immediately), or
 * played within the configured window. Each window is independent and 0 is off,
 * so with both at 0 nothing here applies — including duplicate detection.
 */
function cooldownError(track: Track): string | null {
  const db = getDb()
  const cfg = loadConfig()

  if (cfg.sameSongCooldownMinutes > 0) {
    const queued = db
      .prepare("SELECT 1 FROM queue WHERE track_id = ? AND status IN ('playing', 'pending')")
      .get(track.id)
    if (queued) return `"${track.title}" is already in the queue.`

    const played = db
      .prepare('SELECT 1 FROM play_history WHERE track_id = ? AND played_at >= ?')
      .get(track.id, minutesAgo(cfg.sameSongCooldownMinutes))
    if (played) {
      return `"${track.title}" was played in the last ${cfg.sameSongCooldownMinutes} min — pick something else.`
    }
  }

  if (cfg.sameArtistCooldownMinutes > 0 && track.artist) {
    const queued = db
      .prepare(
        `SELECT 1 FROM queue q JOIN tracks t ON t.id = q.track_id
         WHERE q.status IN ('playing', 'pending') AND t.artist = ? COLLATE NOCASE`
      )
      .get(track.artist)
    if (queued) return `${track.artist} is already in the queue — try another artist.`

    const played = db
      .prepare('SELECT 1 FROM play_history WHERE artist = ? COLLATE NOCASE AND played_at >= ?')
      .get(track.artist, minutesAgo(cfg.sameArtistCooldownMinutes))
    if (played) {
      return `${track.artist} was played in the last ${cfg.sameArtistCooldownMinutes} min — try another artist.`
    }
  }

  return null
}

/** Pending (not-yet-played) songs queued by one guest — what the limit counts. */
function pendingCountFor(ip: string): number {
  return (
    getDb()
      .prepare("SELECT COUNT(*) AS c FROM queue WHERE added_by_ip = ? AND status = 'pending'")
      .get(ip) as { c: number }
  ).c
}

/** The currently playing entry, or null when it's a standby (filler) track. */
function votableEntry(): { id: number; trackId: number } | null {
  const row = getDb()
    .prepare("SELECT id, track_id, added_by_ip FROM queue WHERE status = 'playing' LIMIT 1")
    .get() as { id: number; track_id: number; added_by_ip: string } | undefined
  if (!row || row.added_by_ip === STANDBY_IP) return null
  return { id: row.id, trackId: row.track_id }
}

/** Chooses the next standby track (sequential or shuffled), or null if none. */
function pickStandbyTrack(): number | null {
  const ids = standbyTrackIds()
  if (ids.length === 0) return null
  if (loadConfig().standbyShuffle) {
    let pick = ids[Math.floor(Math.random() * ids.length)]
    // Avoid repeating the previous track when there's more than one.
    for (let tries = 0; ids.length > 1 && pick === lastStandbyTrack && tries < 8; tries++) {
      pick = ids[Math.floor(Math.random() * ids.length)]
    }
    lastStandbyTrack = pick
    return pick
  }
  standbyCursor = (standbyCursor + 1) % ids.length
  lastStandbyTrack = ids[standbyCursor]
  return lastStandbyTrack
}

/** Error carrying an HTTP status for the API layer. */
export class QueueError extends Error {
  status: number
  constructor(message: string, status = 400) {
    super(message)
    this.status = status
  }
}

interface QueueRow {
  id: number
  track_id: number
  added_by_ip: string
  added_by_name: string | null
  position: number
  status: string
}

function toEntry(row: QueueRow, forIp: string): QueueEntry | null {
  const track = getTrackById(row.track_id)
  if (!track) return null
  return {
    id: row.id,
    track,
    addedByName: row.added_by_name,
    mine: row.added_by_ip === forIp
  }
}

/** Reset transient state on boot: nothing is "playing" until the player starts. */
export function initQueue(): void {
  const db = getDb()
  // Drop any leftover standby track, then demote a guest "playing" row to pending.
  db.prepare('DELETE FROM queue WHERE added_by_ip = ?').run(STANDBY_IP)
  db.prepare("UPDATE queue SET status = 'pending' WHERE status = 'playing'").run()
}

export function buildQueueState(forIp: string): QueueState {
  const db = getDb()
  const playingRow = db
    .prepare("SELECT * FROM queue WHERE status = 'playing' LIMIT 1")
    .get() as QueueRow | undefined
  const pendingRows = db
    .prepare(
      "SELECT * FROM queue WHERE status = 'pending' AND added_by_ip != ? ORDER BY position ASC"
    )
    .all(STANDBY_IP) as QueueRow[]

  const state = getState()
  // Downvotes only apply to the song currently being voted on. A negative
  // threshold keeps the same skip logic but hides the count/threshold from the UI.
  const rawThreshold = loadConfig().downvoteSkipThreshold
  const showCount = rawThreshold > 0
  const votesActive = playingRow != null && downvoteEntryId === playingRow.id
  const nowPlaying: NowPlaying = {
    entry: playingRow ? toEntry(playingRow, forIp) : null,
    position: state.position,
    duration: state.duration,
    playing: state.playing,
    isStandby: playingRow?.added_by_ip === STANDBY_IP,
    downvotes: votesActive && showCount ? downvoters.size : 0,
    downvoteThreshold: rawThreshold,
    downvotedByMe: votesActive ? downvoters.has(forIp) : false
  }

  const queue = pendingRows
    .map((r) => toEntry(r, forIp))
    .filter((e): e is QueueEntry => e !== null)

  return {
    nowPlaying,
    queue,
    perUserLimit: loadConfig().perUserQueueLimit,
    myQueueCount: pendingCountFor(forIp)
  }
}

/**
 * Advances playback. Guest songs come first; when none are pending, the standby
 * playlist fills in (if enabled). Also used for skip / on song-ended.
 */
export function advance(): void {
  const db = getDb()
  // New song → clear any downvotes.
  downvoters = new Set()
  downvoteEntryId = null
  db.prepare("DELETE FROM queue WHERE status = 'playing'").run()

  const next = db
    .prepare(
      "SELECT * FROM queue WHERE status = 'pending' AND added_by_ip != ? ORDER BY position ASC LIMIT 1"
    )
    .get(STANDBY_IP) as QueueRow | undefined

  if (next) {
    db.prepare("UPDATE queue SET status = 'playing' WHERE id = ?").run(next.id)
    recordPlay(next.track_id, next.added_by_ip, next.added_by_name, false)
    loadTrack(next.track_id, true)
  } else if (loadConfig().standbyEnabled) {
    const trackId = pickStandbyTrack()
    if (trackId != null) {
      db.prepare(
        `INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
         VALUES (?, ?, NULL, ?, 0, 'playing')`
      ).run(trackId, STANDBY_IP, new Date().toISOString())
      recordPlay(trackId, null, null, true)
      loadTrack(trackId, true)
    } else {
      pause()
    }
  } else {
    pause()
  }
  broadcastQueue()
}

/** Starts playback if nothing is currently playing but songs are queued. */
export function maybeStart(): void {
  const playing = getDb().prepare("SELECT 1 FROM queue WHERE status = 'playing'").get()
  if (!playing) advance()
}

export function enqueue(trackId: number, ip: string, name?: string): void {
  const db = getDb()
  const track = getTrackById(trackId)
  if (!track) throw new QueueError('Track not found', 404)

  // Order matters: the most decisive reason first, so a banned guest is never
  // told merely to wait.
  const banned = banError(ip)
  if (banned) throw new QueueError(banned, 403)

  const tooSoon = rateLimitError(ip)
  if (tooSoon) throw new QueueError(tooSoon, 429)

  const cooldown = cooldownError(track)
  if (cooldown) throw new QueueError(cooldown, 409)

  // 0 = no limit; negative applies |value| but keeps the counter hidden.
  const raw = loadConfig().perUserQueueLimit
  if (raw !== 0) {
    const limit = Math.abs(raw)
    if (pendingCountFor(ip) >= limit) {
      throw new QueueError(
        `You can have at most ${limit} song${limit === 1 ? '' : 's'} in the queue.`,
        409
      )
    }
  }

  const nextPos =
    (db.prepare('SELECT COALESCE(MAX(position), 0) + 1 AS p FROM queue').get() as { p: number }).p
  db.prepare(
    `INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
     VALUES (?, ?, ?, ?, ?, 'pending')`
  ).run(trackId, ip, name?.trim() || null, new Date().toISOString(), nextPos)
  recordRequest(trackId, ip, name)

  const playing = db
    .prepare("SELECT added_by_ip AS ip FROM queue WHERE status = 'playing' LIMIT 1")
    .get() as { ip: string } | undefined
  if (!playing || playing.ip === STANDBY_IP) {
    // Idle → start; filler playing → take over from the standby track now.
    advance()
  } else {
    broadcastQueue()
  }
}

export function removeEntry(entryId: number, ip: string, isAdmin: boolean): void {
  const db = getDb()
  const row = db.prepare('SELECT * FROM queue WHERE id = ?').get(entryId) as QueueRow | undefined
  if (!row) throw new QueueError('Entry not found', 404)
  if (!isAdmin && row.added_by_ip !== ip) {
    throw new QueueError('You can only remove songs you added.', 403)
  }
  if (row.status === 'playing') {
    advance() // removing the current song = skip to next
  } else {
    db.prepare('DELETE FROM queue WHERE id = ?').run(entryId)
    broadcastQueue()
  }
}

/** Admin: move a pending entry to a new index within the pending list. */
export function reorder(entryId: number, toIndex: number): void {
  const db = getDb()
  const ids = (
    db.prepare("SELECT id FROM queue WHERE status = 'pending' ORDER BY position ASC").all() as {
      id: number
    }[]
  ).map((r) => r.id)
  const from = ids.indexOf(entryId)
  if (from < 0) return
  ids.splice(from, 1)
  const dest = Math.min(Math.max(toIndex, 0), ids.length)
  ids.splice(dest, 0, entryId)

  const update = db.prepare('UPDATE queue SET position = ? WHERE id = ?')
  db.transaction(() => {
    ids.forEach((id, i) => update.run(i + 1, id))
  })()
  broadcastQueue()
}

/** Admin: skip the current song. */
export function skip(): void {
  advance()
}

/** Guest: downvote the current song; skips it once the threshold is reached. */
export function downvote(ip: string, name?: string): void {
  const raw = loadConfig().downvoteSkipThreshold
  if (raw === 0) return // feature disabled
  const threshold = Math.abs(raw) // negative = same logic, count hidden in the UI
  // Null when nothing is playing, or when the current song is standby filler —
  // filler isn't a guest's pick, so there's nothing to vote off.
  const playing = votableEntry()
  if (playing == null) return

  if (downvoteEntryId !== playing.id) {
    // First vote on this song.
    downvoteEntryId = playing.id
    downvoters = new Set()
  }
  if (downvoters.has(ip)) return // already voted — don't double-count
  downvoters.add(ip)
  recordDownvote(playing.trackId, ip, name)

  if (downvoters.size >= threshold) {
    advance() // enough downvotes → skip (advance clears the vote state)
  } else {
    broadcastQueue()
  }
}

/** Admin: clear all pending entries (keeps the current song playing). */
export function clearPending(): void {
  getDb().prepare("DELETE FROM queue WHERE status = 'pending'").run()
  broadcastQueue()
}
