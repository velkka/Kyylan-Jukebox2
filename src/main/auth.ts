import { randomBytes } from 'node:crypto'
import type { Request, Response, NextFunction } from 'express'
import { loadConfig } from './config'

export const SESSION_COOKIE = 'kj_session'
const SESSION_TTL_MS = 12 * 60 * 60 * 1000 // 12 hours

// In-memory session tokens. Cleared on restart (admin simply logs in again).
const sessions = new Map<string, number>() // token -> expiry epoch ms

function prune(): void {
  const now = Date.now()
  for (const [token, exp] of sessions) if (exp <= now) sessions.delete(token)
}

/** Verifies the password and issues a session token, or null on mismatch. */
export function login(password: string): string | null {
  const expected = loadConfig().adminPassword
  // Reject when no password configured or on mismatch. (Plaintext compare — the
  // password is stored in the clear by design; see project notes.)
  if (!expected || password !== expected) return null
  const token = randomBytes(32).toString('hex')
  sessions.set(token, Date.now() + SESSION_TTL_MS)
  return token
}

export function logout(token: string | undefined): void {
  if (token) sessions.delete(token)
}

export function isValidSession(token: string | undefined): boolean {
  if (!token) return false
  prune()
  return sessions.has(token)
}

export function isAdminRequest(req: Request): boolean {
  return isValidSession(req.cookies?.[SESSION_COOKIE])
}

/** Express guard for admin-only routes. */
export function requireAdmin(req: Request, res: Response, next: NextFunction): void {
  if (isAdminRequest(req)) {
    next()
  } else {
    res.status(401).json({ error: 'Admin login required' })
  }
}

export const sessionCookieOptions = {
  httpOnly: true,
  sameSite: 'lax' as const,
  secure: false, // LAN over plain HTTP
  maxAge: SESSION_TTL_MS
}
