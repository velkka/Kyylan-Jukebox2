import express, { Router } from 'express'
import cookieParser from 'cookie-parser'
import { app, dialog } from 'electron'
import { createReadStream, existsSync, statSync } from 'node:fs'
import { extname } from 'node:path'
import { loadConfig, updateConfig } from './config'
import { lanAddresses, normalizeIp } from './net'
import {
  isAdminRequest,
  login,
  logout,
  requireAdmin,
  SESSION_COOKIE,
  sessionCookieOptions
} from './auth'
import {
  buildQueueState,
  clearPending,
  downvote,
  enqueue,
  maybeStart,
  QueueError,
  removeEntry,
  reorder,
  skip
} from './queue'
import { addStandby, clearStandby, listStandby, removeStandby } from './standby'
import { broadcastQueue } from './realtime'
import {
  addPath,
  getArt,
  getTrackPath,
  listPathsWithCounts,
  queryAlbums,
  queryArtists,
  queryTracks,
  removePath,
  scanAll,
  scanStatus
} from './library'
import {
  getDevices,
  getState,
  loadTrack,
  pause,
  play,
  requestDevices,
  seek,
  setOutputDevice,
  setVolume
} from './player'
import { getHostWindow } from './hostWindow'
import {
  BrowseFolderResponse,
  HealthResponse,
  LibraryPathsResponse,
  PublicConfig,
  SetupRequest,
  SetupResponse
} from '@shared/types'

/**
 * Public API router. Everything here is reachable by any LAN guest.
 * Admin-only routes (settings writes, queue management) are added under an auth
 * guard in later milestones. This never exposes the plaintext admin password.
 */
export function createApiRouter(getRunningPort: () => number): Router {
  const router = Router()
  router.use(express.json())
  router.use(cookieParser())

  /** True when the request originates from the host machine itself. */
  const isLocal = (req: express.Request): boolean =>
    normalizeIp(req.socket.remoteAddress ?? undefined) === '127.0.0.1'

  // ---- Auth -----------------------------------------------------------------

  router.get('/auth', (req, res) => {
    res.json({ isAdmin: isAdminRequest(req), isLocal: isLocal(req) })
  })

  router.post('/login', (req, res) => {
    const password = String((req.body ?? {}).password ?? '')
    const token = login(password)
    if (!token) {
      res.status(401).json({ error: 'Incorrect password' })
      return
    }
    res.cookie(SESSION_COOKIE, token, sessionCookieOptions)
    res.json({ isAdmin: true, isLocal: isLocal(req) })
  })

  router.post('/logout', (req, res) => {
    logout(req.cookies?.[SESSION_COOKIE])
    res.clearCookie(SESSION_COOKIE)
    res.json({ isAdmin: false, isLocal: isLocal(req) })
  })

  // ---- Admin settings -------------------------------------------------------

  router.get('/admin/settings', requireAdmin, (_req, res) => {
    const c = loadConfig()
    res.json({
      port: c.port,
      perUserQueueLimit: c.perUserQueueLimit,
      downvoteSkipThreshold: c.downvoteSkipThreshold
    })
  })

  router.post('/admin/settings', requireAdmin, (req, res) => {
    const body = (req.body ?? {}) as {
      port?: number
      perUserQueueLimit?: number
      downvoteSkipThreshold?: number
      adminPassword?: string
    }
    const patch: Partial<ReturnType<typeof loadConfig>> = {}

    if (body.perUserQueueLimit !== undefined) {
      const n = Number(body.perUserQueueLimit)
      // 0 = no limit; negative applies |n| as the limit but hides the counter.
      if (!Number.isInteger(n) || n < -100 || n > 100) {
        res.status(400).json({ error: 'Per-guest limit must be between -100 and 100' })
        return
      }
      patch.perUserQueueLimit = n
    }
    if (body.downvoteSkipThreshold !== undefined) {
      const n = Number(body.downvoteSkipThreshold)
      // 0 = disabled; negative uses |n| as the threshold but hides the count.
      if (!Number.isInteger(n) || n < -100 || n > 100) {
        res.status(400).json({ error: 'Downvote threshold must be between -100 and 100' })
        return
      }
      patch.downvoteSkipThreshold = n
    }
    if (body.port !== undefined) {
      const n = Number(body.port)
      if (!Number.isInteger(n) || n < 1 || n > 65535) {
        res.status(400).json({ error: 'Port must be between 1 and 65535' })
        return
      }
      patch.port = n
    }
    if (body.adminPassword !== undefined) {
      if (typeof body.adminPassword !== 'string' || body.adminPassword.length < 1) {
        res.status(400).json({ error: 'Password cannot be empty' })
        return
      }
      patch.adminPassword = body.adminPassword
    }

    updateConfig(patch)
    // These affect everyone's view (limit label / downvote button) — rebroadcast.
    if (patch.perUserQueueLimit !== undefined || patch.downvoteSkipThreshold !== undefined) {
      broadcastQueue()
    }
    res.json({ restartRequired: patch.port !== undefined && patch.port !== getRunningPort() })
  })

  router.get('/health', (_req, res) => {
    const config = loadConfig()
    const body: HealthResponse = {
      name: 'Kyylan Jukebox',
      version: app.getVersion(),
      addresses: lanAddresses(),
      port: config.port
    }
    res.json(body)
  })

  router.get('/config', (_req, res) => {
    const config = loadConfig()
    const body: PublicConfig = {
      configured: config.configured,
      port: config.port,
      perUserQueueLimit: config.perUserQueueLimit,
      name: 'Kyylan Jukebox',
      version: app.getVersion()
    }
    res.json(body)
  })

  // First-run setup. Allowed only until the app is configured; afterwards the
  // admin changes these from the (authenticated) settings screen in M8.
  router.post('/setup', (req, res) => {
    const config = loadConfig()
    if (config.configured) {
      res.status(403).json({ error: 'Already configured' })
      return
    }

    const { adminPassword, port } = (req.body ?? {}) as SetupRequest
    if (typeof adminPassword !== 'string' || adminPassword.trim().length < 1) {
      res.status(400).json({ error: 'Admin password is required' })
      return
    }

    let nextPort = config.port
    if (port !== undefined) {
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        res.status(400).json({ error: 'Port must be an integer between 1 and 65535' })
        return
      }
      nextPort = port
    }

    updateConfig({ configured: true, adminPassword, port: nextPort })
    const body: SetupResponse = {
      ok: true,
      restartRequired: nextPort !== getRunningPort(),
      port: nextPort
    }
    res.json(body)
  })

  // ---- Library (browse / search / art) --------------------------------------

  const str = (v: unknown): string | undefined => (typeof v === 'string' ? v : undefined)

  router.get('/tracks', (req, res) => {
    res.json(
      queryTracks({
        search: str(req.query.search),
        artist: str(req.query.artist),
        album: str(req.query.album),
        albumArtist: str(req.query.albumArtist),
        noAlbum: req.query.noAlbum === '1' || req.query.noAlbum === 'true',
        limit: req.query.limit ? Number(req.query.limit) : undefined,
        offset: req.query.offset ? Number(req.query.offset) : undefined
      })
    )
  })

  router.get('/artists', (req, res) => {
    res.json(
      queryArtists({
        search: str(req.query.search),
        letter: str(req.query.letter),
        limit: req.query.limit ? Number(req.query.limit) : undefined,
        offset: req.query.offset ? Number(req.query.offset) : undefined
      })
    )
  })

  router.get('/albums', (req, res) => {
    res.json(
      queryAlbums({
        search: str(req.query.search),
        artist: str(req.query.artist),
        limit: req.query.limit ? Number(req.query.limit) : undefined,
        offset: req.query.offset ? Number(req.query.offset) : undefined
      })
    )
  })

  router.get('/art/:hash', (req, res) => {
    const art = getArt(req.params.hash)
    if (!art) {
      res.status(404).end()
      return
    }
    // Content-addressed, so it never changes for a given hash.
    res.set('Cache-Control', 'public, max-age=31536000, immutable')
    res.type(art.mime)
    res.send(art.data)
  })

  // ---- Library management (admin) -------------------------------------------

  router.get('/library/paths', requireAdmin, (_req, res) => {
    res.json(listPathsWithCounts() satisfies LibraryPathsResponse)
  })

  router.post('/library/paths', requireAdmin, (req, res) => {
    try {
      addPath(String((req.body ?? {}).path ?? ''))
      res.json(listPathsWithCounts() satisfies LibraryPathsResponse)
    } catch (err) {
      res.status(400).json({ error: err instanceof Error ? err.message : String(err) })
    }
  })

  router.delete('/library/paths', requireAdmin, (req, res) => {
    removePath(String((req.body ?? {}).path ?? ''))
    res.json(listPathsWithCounts() satisfies LibraryPathsResponse)
  })

  // Native folder picker. Host-machine only: a LAN admin must not be able to pop
  // a modal dialog on the host, so remote requests are refused.
  router.post('/library/browse', requireAdmin, async (req, res) => {
    if (!isLocal(req)) {
      res.status(403).json({ error: 'The folder picker is only available on the host machine' })
      return
    }
    const parent = getHostWindow()
    const options: Electron.OpenDialogOptions = {
      title: 'Choose a music folder',
      buttonLabel: 'Add folder',
      properties: ['openDirectory', 'createDirectory']
    }
    const result = parent
      ? await dialog.showOpenDialog(parent, options)
      : await dialog.showOpenDialog(options)
    const chosen = result.filePaths[0]
    if (result.canceled || !chosen) {
      res.json({ canceled: true } satisfies BrowseFolderResponse)
      return
    }
    try {
      addPath(chosen)
    } catch (err) {
      res.status(400).json({ error: err instanceof Error ? err.message : String(err) })
      return
    }
    res.json({ canceled: false, path: chosen } satisfies BrowseFolderResponse)
  })

  router.post('/library/scan', requireAdmin, (_req, res) => {
    // Fire-and-forget; progress is polled via /library/scan/status.
    void scanAll()
    res.json(scanStatus())
  })

  router.get('/library/scan/status', requireAdmin, (_req, res) => {
    res.json(scanStatus())
  })

  // ---- Audio streaming (consumed by the hidden player window) ---------------

  router.get('/stream/:id', (req, res) => {
    const path = getTrackPath(Number(req.params.id))
    if (!path || !existsSync(path)) {
      res.status(404).end()
      return
    }
    const size = statSync(path).size
    const mime = audioMime(extname(path))
    res.set('Accept-Ranges', 'bytes')
    res.type(mime)

    const range = req.headers.range
    if (range) {
      const match = /bytes=(\d*)-(\d*)/.exec(range)
      const start = match && match[1] ? parseInt(match[1], 10) : 0
      const end = match && match[2] ? parseInt(match[2], 10) : size - 1
      if (start >= size || end >= size || start > end) {
        res.status(416).set('Content-Range', `bytes */${size}`).end()
        return
      }
      res.status(206).set({
        'Content-Range': `bytes ${start}-${end}/${size}`,
        'Content-Length': String(end - start + 1)
      })
      createReadStream(path, { start, end }).pipe(res)
    } else {
      res.set('Content-Length', String(size))
      createReadStream(path).pipe(res)
    }
  })

  // ---- Player control (admin — guarded in M6) -------------------------------

  router.get('/player/state', (_req, res) => res.json(getState()))

  router.get('/player/devices', requireAdmin, (_req, res) => {
    requestDevices() // nudge the renderer to refresh
    res.json({ devices: getDevices(), selected: loadConfig().outputDeviceId })
  })

  router.post('/player/output', requireAdmin, (req, res) => {
    const deviceId = String((req.body ?? {}).deviceId ?? '')
    setOutputDevice(deviceId)
    res.json({ ok: true, selected: loadConfig().outputDeviceId })
  })

  router.post('/player/load', requireAdmin, (req, res) => {
    const trackId = Number((req.body ?? {}).trackId)
    if (!Number.isInteger(trackId)) {
      res.status(400).json({ error: 'trackId required' })
      return
    }
    const autoplay = (req.body ?? {}).autoplay !== false
    loadTrack(trackId, autoplay)
    res.json(getState())
  })

  router.post('/player/play', requireAdmin, (_req, res) => {
    play()
    res.json(getState())
  })

  router.post('/player/pause', requireAdmin, (_req, res) => {
    pause()
    res.json(getState())
  })

  router.post('/player/seek', requireAdmin, (req, res) => {
    const position = Number((req.body ?? {}).position)
    if (!Number.isFinite(position)) {
      res.status(400).json({ error: 'position required' })
      return
    }
    seek(position)
    res.json(getState())
  })

  router.post('/player/volume', requireAdmin, (req, res) => {
    const value = Number((req.body ?? {}).value)
    if (!Number.isFinite(value)) {
      res.status(400).json({ error: 'value required' })
      return
    }
    setVolume(value)
    res.json(getState())
  })

  // ---- Queue ----------------------------------------------------------------

  const clientIp = (req: express.Request): string => normalizeIp(req.socket.remoteAddress ?? undefined)

  router.get('/queue', (req, res) => {
    res.json(buildQueueState(clientIp(req)))
  })

  router.post('/queue', (req, res) => {
    const ip = clientIp(req)
    const trackId = Number((req.body ?? {}).trackId)
    const name = typeof (req.body ?? {}).name === 'string' ? (req.body.name as string) : undefined
    if (!Number.isInteger(trackId)) {
      res.status(400).json({ error: 'trackId required' })
      return
    }
    try {
      enqueue(trackId, ip, name)
      res.json(buildQueueState(ip))
    } catch (err) {
      const status = err instanceof QueueError ? err.status : 500
      res.status(status).json({ error: err instanceof Error ? err.message : String(err) })
    }
  })

  router.delete('/queue/:id', (req, res) => {
    const ip = clientIp(req)
    try {
      // Admins can remove any entry; guests only their own.
      removeEntry(Number(req.params.id), ip, isAdminRequest(req))
      res.json(buildQueueState(ip))
    } catch (err) {
      const status = err instanceof QueueError ? err.status : 500
      res.status(status).json({ error: err instanceof Error ? err.message : String(err) })
    }
  })

  router.post('/queue/downvote', (req, res) => {
    const ip = clientIp(req)
    downvote(ip)
    res.json(buildQueueState(ip))
  })

  // ---- Queue admin ----------------------------------------------------------

  router.post('/queue/:id/move', requireAdmin, (req, res) => {
    const toIndex = Number((req.body ?? {}).toIndex)
    if (!Number.isInteger(toIndex)) {
      res.status(400).json({ error: 'toIndex required' })
      return
    }
    reorder(Number(req.params.id), toIndex)
    res.json(buildQueueState(clientIp(req)))
  })

  router.post('/queue/skip', requireAdmin, (req, res) => {
    skip()
    res.json(buildQueueState(clientIp(req)))
  })

  router.post('/queue/clear', requireAdmin, (req, res) => {
    clearPending()
    res.json(buildQueueState(clientIp(req)))
  })

  // ---- Standby (filler) playlist — admin ------------------------------------

  const standbyState = () => {
    const c = loadConfig()
    return { enabled: c.standbyEnabled, shuffle: c.standbyShuffle, entries: listStandby() }
  }

  router.get('/standby', requireAdmin, (_req, res) => res.json(standbyState()))

  router.post('/standby', requireAdmin, (req, res) => {
    const trackId = Number((req.body ?? {}).trackId)
    if (!Number.isInteger(trackId)) {
      res.status(400).json({ error: 'trackId required' })
      return
    }
    try {
      addStandby(trackId)
    } catch (err) {
      res.status(400).json({ error: err instanceof Error ? err.message : String(err) })
      return
    }
    maybeStart() // start filler now if enabled and nothing is playing
    res.json(standbyState())
  })

  router.delete('/standby/:id', requireAdmin, (req, res) => {
    removeStandby(Number(req.params.id))
    res.json(standbyState())
  })

  router.post('/standby/clear', requireAdmin, (_req, res) => {
    clearStandby()
    res.json(standbyState())
  })

  router.post('/standby/settings', requireAdmin, (req, res) => {
    const body = (req.body ?? {}) as { enabled?: boolean; shuffle?: boolean }
    const patch: Partial<ReturnType<typeof loadConfig>> = {}
    if (typeof body.enabled === 'boolean') patch.standbyEnabled = body.enabled
    if (typeof body.shuffle === 'boolean') patch.standbyShuffle = body.shuffle
    updateConfig(patch)
    if (patch.standbyEnabled) maybeStart() // enabling while idle kicks off filler
    res.json(standbyState())
  })

  return router
}

/** Maps a file extension to a browser-friendly audio MIME type. */
function audioMime(ext: string): string {
  switch (ext.toLowerCase()) {
    case '.mp3':
      return 'audio/mpeg'
    case '.m4a':
    case '.aac':
      return 'audio/mp4'
    case '.flac':
      return 'audio/flac'
    case '.ogg':
    case '.oga':
      return 'audio/ogg'
    case '.opus':
      return 'audio/opus'
    case '.wav':
      return 'audio/wav'
    case '.wma':
      return 'audio/x-ms-wma'
    default:
      return 'application/octet-stream'
  }
}
