import { getDb } from './db'
import { loadConfig } from './config'
import { getTrackById } from './library'
import { getState, loadTrack, pause } from './player'
import { broadcastQueue } from './realtime'
import { standbyTrackIds } from './standby'
import { NowPlaying, QueueEntry, QueueState } from '@shared/types'

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

/** Pending (not-yet-played) songs queued by one guest — what the limit counts. */
function pendingCountFor(ip: string): number {
  return (
    getDb()
      .prepare("SELECT COUNT(*) AS c FROM queue WHERE added_by_ip = ? AND status = 'pending'")
      .get(ip) as { c: number }
  ).c
}

function playingRowId(): number | null {
  const row = getDb().prepare("SELECT id FROM queue WHERE status = 'playing' LIMIT 1").get() as
    | { id: number }
    | undefined
  return row?.id ?? null
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
    loadTrack(next.track_id, true)
  } else if (loadConfig().standbyEnabled) {
    const trackId = pickStandbyTrack()
    if (trackId != null) {
      db.prepare(
        `INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
         VALUES (?, ?, NULL, ?, 0, 'playing')`
      ).run(trackId, STANDBY_IP, new Date().toISOString())
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
  if (!getTrackById(trackId)) throw new QueueError('Track not found', 404)

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
export function downvote(ip: string): void {
  const raw = loadConfig().downvoteSkipThreshold
  if (raw === 0) return // feature disabled
  const threshold = Math.abs(raw) // negative = same logic, count hidden in the UI
  const playingId = playingRowId()
  if (playingId == null) return // nothing playing

  if (downvoteEntryId !== playingId) {
    // First vote on this song.
    downvoteEntryId = playingId
    downvoters = new Set()
  }
  downvoters.add(ip)

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
