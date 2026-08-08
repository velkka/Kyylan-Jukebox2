import { getDb } from './db'
import { getTrackById } from './library'
import { StandbyEntry } from '@shared/types'

/** Ordered track ids in the standby playlist. */
export function standbyTrackIds(): number[] {
  return (
    getDb().prepare('SELECT track_id FROM standby ORDER BY position').all() as {
      track_id: number
    }[]
  ).map((r) => r.track_id)
}

export function listStandby(): StandbyEntry[] {
  const rows = getDb().prepare('SELECT id, track_id FROM standby ORDER BY position').all() as {
    id: number
    track_id: number
  }[]
  return rows
    .map((r) => {
      const track = getTrackById(r.track_id)
      return track ? { id: r.id, track } : null
    })
    .filter((e): e is StandbyEntry => e !== null)
}

export function addStandby(trackId: number): void {
  if (!getTrackById(trackId)) throw new Error('Track not found')
  const db = getDb()
  const pos = (db.prepare('SELECT COALESCE(MAX(position), 0) + 1 AS p FROM standby').get() as {
    p: number
  }).p
  db.prepare('INSERT INTO standby (track_id, position, added_at) VALUES (?, ?, ?)').run(
    trackId,
    pos,
    new Date().toISOString()
  )
}

export function removeStandby(id: number): void {
  getDb().prepare('DELETE FROM standby WHERE id = ?').run(id)
}

export function clearStandby(): void {
  getDb().prepare('DELETE FROM standby').run()
}
