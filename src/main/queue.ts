import { getDb } from './db'
import { loadConfig } from './config'
import { getTrackById } from './library'
import { getState, loadTrack, pause } from './player'
import { broadcastQueue } from './realtime'
import { NowPlaying, QueueEntry, QueueState } from '@shared/types'

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
  getDb().prepare("UPDATE queue SET status = 'pending' WHERE status = 'playing'").run()
}

export function buildQueueState(forIp: string): QueueState {
  const db = getDb()
  const playingRow = db
    .prepare("SELECT * FROM queue WHERE status = 'playing' LIMIT 1")
    .get() as QueueRow | undefined
  const pendingRows = db
    .prepare("SELECT * FROM queue WHERE status = 'pending' ORDER BY position ASC")
    .all() as QueueRow[]

  const state = getState()
  const nowPlaying: NowPlaying = {
    entry: playingRow ? toEntry(playingRow, forIp) : null,
    position: state.position,
    duration: state.duration,
    playing: state.playing
  }

  const queue = pendingRows
    .map((r) => toEntry(r, forIp))
    .filter((e): e is QueueEntry => e !== null)

  return { nowPlaying, queue, perUserLimit: loadConfig().perUserQueueLimit }
}

/** Advances to the next pending track (also used for skip / on song-ended). */
export function advance(): void {
  const db = getDb()
  db.prepare("DELETE FROM queue WHERE status = 'playing'").run()
  const next = db
    .prepare("SELECT * FROM queue WHERE status = 'pending' ORDER BY position ASC LIMIT 1")
    .get() as QueueRow | undefined
  if (next) {
    db.prepare("UPDATE queue SET status = 'playing' WHERE id = ?").run(next.id)
    loadTrack(next.track_id, true)
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

  const limit = loadConfig().perUserQueueLimit
  const mine = (
    db
      .prepare("SELECT COUNT(*) AS c FROM queue WHERE added_by_ip = ? AND status = 'pending'")
      .get(ip) as { c: number }
  ).c
  if (mine >= limit) {
    throw new QueueError(
      `You can have at most ${limit} song${limit === 1 ? '' : 's'} in the queue.`,
      409
    )
  }

  const nextPos =
    (db.prepare('SELECT COALESCE(MAX(position), 0) + 1 AS p FROM queue').get() as { p: number }).p
  db.prepare(
    `INSERT INTO queue (track_id, added_by_ip, added_by_name, added_at, position, status)
     VALUES (?, ?, ?, ?, ?, 'pending')`
  ).run(trackId, ip, name?.trim() || null, new Date().toISOString(), nextPos)

  maybeStart() // start immediately if idle
  broadcastQueue()
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

/** Admin: clear all pending entries (keeps the current song playing). */
export function clearPending(): void {
  getDb().prepare("DELETE FROM queue WHERE status = 'pending'").run()
  broadcastQueue()
}
