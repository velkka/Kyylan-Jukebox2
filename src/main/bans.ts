import { getDb } from './db'
import type { BanEntry } from '@shared/types'

/**
 * Guests barred from adding songs, by IP. A ban blocks new adds only — songs
 * the guest already has queued keep their place, and downvoting still works;
 * an admin can remove their entries from the queue separately.
 */

const now = (): string => new Date().toISOString()

/** Drops bans whose time is up, so every read only ever sees active ones. */
function pruneExpired(): void {
  getDb().prepare('DELETE FROM bans WHERE expires_at IS NOT NULL AND expires_at <= ?').run(now())
}

interface BanRow {
  ip: string
  name: string | null
  bannedAt: string
  expiresAt: string | null
}

export function listBans(): BanEntry[] {
  pruneExpired()
  return getDb()
    .prepare(
      `SELECT ip, name, banned_at AS bannedAt, expires_at AS expiresAt
         FROM bans ORDER BY (expires_at IS NOT NULL), expires_at ASC, banned_at DESC`
    )
    .all() as BanRow[]
}

/** The guest's active ban, or null. Permanent bans have a null `expiresAt`. */
export function activeBan(ip: string): BanEntry | null {
  pruneExpired()
  return (
    (getDb()
      .prepare(
        `SELECT ip, name, banned_at AS bannedAt, expires_at AS expiresAt
           FROM bans WHERE ip = ?`
      )
      .get(ip) as BanRow | undefined) ?? null
  )
}

/** Bans an IP. `minutes` of 0 or undefined means permanently. */
export function banIp(ip: string, name?: string | null, minutes?: number): BanEntry[] {
  const expiresAt =
    minutes && minutes > 0 ? new Date(Date.now() + minutes * 60_000).toISOString() : null
  // Re-banning replaces the old ban rather than stacking.
  getDb()
    .prepare(
      `INSERT INTO bans (ip, name, banned_at, expires_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(ip) DO UPDATE SET
         name = COALESCE(excluded.name, bans.name),
         banned_at = excluded.banned_at,
         expires_at = excluded.expires_at`
    )
    .run(ip, name?.trim() || null, now(), expiresAt)
  return listBans()
}

export function unbanIp(ip: string): BanEntry[] {
  getDb().prepare('DELETE FROM bans WHERE ip = ?').run(ip)
  return listBans()
}

/** Human-readable "x min"/"x h"/"x days" for a ban's remaining time. */
function remaining(expiresAt: string): string {
  const mins = Math.max(1, Math.ceil((new Date(expiresAt).getTime() - Date.now()) / 60_000))
  if (mins < 60) return `${mins} min`
  const hours = Math.round(mins / 60)
  if (hours < 48) return `${hours} hour${hours === 1 ? '' : 's'}`
  return `${Math.round(hours / 24)} days`
}

/** Why this guest may not add, or null when they may. */
export function banError(ip: string): string | null {
  const ban = activeBan(ip)
  if (!ban) return null
  if (!ban.expiresAt) return 'You have been blocked from adding songs.'
  return `You have been blocked from adding songs for another ${remaining(ban.expiresAt)}.`
}
