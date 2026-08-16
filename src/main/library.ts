import { readdir, stat } from 'node:fs/promises'
import { existsSync, statSync } from 'node:fs'
import { join, extname, basename } from 'node:path'
import { createHash } from 'node:crypto'
import { parseFile } from 'music-metadata'
import { getDb } from './db'
import { loadConfig, updateConfig } from './config'
import { LibraryPath, Track, TracksQuery, TracksResponse, ScanStatus } from '@shared/types'

const AUDIO_EXTS = new Set([
  '.mp3',
  '.m4a',
  '.aac',
  '.flac',
  '.ogg',
  '.oga',
  '.opus',
  '.wav',
  '.wma'
])

interface TrackRow {
  id: number
  path: string
  title: string
  artist: string | null
  album: string | null
  album_artist: string | null
  genre: string | null
  duration: number | null
  track_no: number | null
  disc_no: number | null
  year: number | null
  art_hash: string | null
}

function rowToTrack(r: TrackRow): Track {
  return {
    id: r.id,
    title: r.title,
    artist: r.artist,
    album: r.album,
    albumArtist: r.album_artist,
    genre: r.genre,
    duration: r.duration,
    trackNo: r.track_no,
    discNo: r.disc_no,
    year: r.year,
    artHash: r.art_hash
  }
}

// ---- Library path management -------------------------------------------------

export function listPaths(): string[] {
  return loadConfig().libraryPaths
}

/** Library folders with the number of indexed tracks under each, plus the total. */
export function listPathsWithCounts(): { paths: LibraryPath[]; total: number } {
  const db = getDb()
  // Compare on a normalized "<folder>/" prefix so a stored trailing slash (or the
  // lack of one) doesn't change the result. `length()` is evaluated by SQLite so
  // character counting matches `substr`.
  const countStmt = db.prepare(
    'SELECT COUNT(*) AS c FROM tracks WHERE substr(path, 1, length(@prefix)) = @prefix'
  )
  const paths = loadConfig().libraryPaths.map((path) => {
    const prefix = `${path.replace(/\/+$/, '')}/`
    return { path, trackCount: (countStmt.get({ prefix }) as { c: number }).c }
  })
  const total = (db.prepare('SELECT COUNT(*) AS c FROM tracks').get() as { c: number }).c
  return { paths, total }
}

export function addPath(path: string): string[] {
  const clean = path.trim()
  if (!clean) throw new Error('Path is required')
  if (!existsSync(clean) || !statSync(clean).isDirectory())
    throw new Error('Folder does not exist')
  const current = loadConfig().libraryPaths
  if (current.includes(clean)) return current
  const next = [...current, clean]
  updateConfig({ libraryPaths: next })
  return next
}

export function removePath(path: string): string[] {
  const next = loadConfig().libraryPaths.filter((p) => p !== path)
  updateConfig({ libraryPaths: next })
  return next
}

// ---- Scanning ----------------------------------------------------------------

const scanState: ScanStatus = {
  scanning: false,
  processed: 0,
  added: 0,
  updated: 0,
  removed: 0,
  total: 0,
  startedAt: null,
  finishedAt: null,
  error: null
}

export function scanStatus(): ScanStatus {
  return { ...scanState }
}

async function* walk(dir: string): AsyncGenerator<string> {
  let entries
  try {
    entries = await readdir(dir, { withFileTypes: true })
  } catch {
    return // unreadable dir — skip
  }
  for (const entry of entries) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) {
      yield* walk(full)
    } else if (entry.isFile() && AUDIO_EXTS.has(extname(entry.name).toLowerCase())) {
      yield full
    }
  }
}

function storeArt(data: Buffer, mime: string): string {
  const hash = createHash('sha1').update(data).digest('hex')
  const db = getDb()
  const exists = db.prepare('SELECT 1 FROM art WHERE hash = ?').get(hash)
  if (!exists) {
    db.prepare('INSERT INTO art (hash, mime, data) VALUES (?, ?, ?)').run(hash, mime, data)
  }
  return hash
}

async function upsertFile(path: string, mtimeMs: number): Promise<'added' | 'updated' | 'unchanged'> {
  const db = getDb()
  const existing = db.prepare('SELECT id, mtime_ms FROM tracks WHERE path = ?').get(path) as
    | { id: number; mtime_ms: number }
    | undefined

  if (existing) {
    if (Math.floor(existing.mtime_ms) === Math.floor(mtimeMs)) {
      db.prepare('UPDATE tracks SET seen = 1 WHERE id = ?').run(existing.id)
      return 'unchanged'
    }
  }

  const meta = await parseFile(path, { duration: true }).catch(() => null)
  const common = meta?.common
  const picture = common?.picture?.[0]
  const artHash = picture ? storeArt(Buffer.from(picture.data), picture.format) : null
  const title = common?.title?.trim() || basename(path, extname(path))

  const fields = {
    path,
    title,
    artist: common?.artist ?? null,
    album: common?.album ?? null,
    album_artist: common?.albumartist ?? null,
    genre: common?.genre?.join(', ') ?? null,
    duration: meta?.format.duration ?? null,
    track_no: common?.track?.no ?? null,
    disc_no: common?.disk?.no ?? null,
    year: common?.year ?? null,
    art_hash: artHash,
    mtime_ms: Math.floor(mtimeMs)
  }

  if (existing) {
    db.prepare(
      `UPDATE tracks SET title=@title, artist=@artist, album=@album, album_artist=@album_artist,
        genre=@genre, duration=@duration, track_no=@track_no, disc_no=@disc_no, year=@year,
        art_hash=@art_hash, mtime_ms=@mtime_ms, seen=1 WHERE path=@path`
    ).run(fields)
    return 'updated'
  }

  db.prepare(
    `INSERT INTO tracks (path, title, artist, album, album_artist, genre, duration,
       track_no, disc_no, year, art_hash, mtime_ms, added_at, seen)
     VALUES (@path, @title, @artist, @album, @album_artist, @genre, @duration,
       @track_no, @disc_no, @year, @art_hash, @mtime_ms, @added_at, 1)`
  ).run({ ...fields, added_at: new Date().toISOString() })
  return 'added'
}

/** Rescans all configured library paths. Concurrent calls are ignored. */
export async function scanAll(): Promise<ScanStatus> {
  if (scanState.scanning) return scanStatus()

  const db = getDb()
  Object.assign(scanState, {
    scanning: true,
    processed: 0,
    added: 0,
    updated: 0,
    removed: 0,
    total: 0,
    startedAt: new Date().toISOString(),
    finishedAt: null,
    error: null
  })

  try {
    db.prepare('UPDATE tracks SET seen = 0').run()

    for (const root of loadConfig().libraryPaths) {
      for await (const file of walk(root)) {
        try {
          const info = await stat(file)
          const result = await upsertFile(file, info.mtimeMs)
          if (result === 'added') scanState.added++
          else if (result === 'updated') scanState.updated++
        } catch (err) {
          console.warn('[library] failed to index', file, err)
        }
        scanState.processed++
      }
    }

    const del = db.prepare('DELETE FROM tracks WHERE seen = 0').run()
    scanState.removed = del.changes
    scanState.total = (db.prepare('SELECT COUNT(*) AS c FROM tracks').get() as { c: number }).c
  } catch (err) {
    scanState.error = err instanceof Error ? err.message : String(err)
    console.error('[library] scan failed:', err)
  } finally {
    scanState.scanning = false
    scanState.finishedAt = new Date().toISOString()
  }
  return scanStatus()
}

// ---- Queries -----------------------------------------------------------------

/** Turns a free-text query into a safe FTS5 prefix-match expression. */
function toFtsQuery(search: string): string {
  const tokens = search.match(/[\p{L}\p{N}]+/gu) ?? []
  return tokens.map((t) => `"${t}"*`).join(' ')
}

export function queryTracks(query: TracksQuery): TracksResponse {
  const db = getDb()
  const limit = Number.isFinite(query.limit) ? Math.min(Math.max(query.limit as number, 1), 500) : 100
  const offset = Number.isFinite(query.offset) ? Math.max(query.offset as number, 0) : 0
  const search = query.search?.trim() ?? ''
  const album = query.album?.trim()
  const artist = query.artist?.trim()

  // Full-text search takes precedence.
  if (search) {
    const fts = toFtsQuery(search)
    if (!fts) return { tracks: [], total: 0, limit, offset }
    const total = (
      db.prepare('SELECT COUNT(*) AS c FROM tracks_fts WHERE tracks_fts MATCH ?').get(fts) as {
        c: number
      }
    ).c
    const rows = db
      .prepare(
        `SELECT t.* FROM tracks t
         JOIN tracks_fts f ON f.rowid = t.id
         WHERE tracks_fts MATCH ?
         ORDER BY rank
         LIMIT ? OFFSET ?`
      )
      .all(fts, limit, offset) as TrackRow[]
    return { tracks: rows.map(rowToTrack), total, limit, offset }
  }

  // Filtered browse: by album (with album-artist) or by artist.
  let where = ''
  let params: Record<string, unknown> = {}
  let order = 'artist COLLATE NOCASE, album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE'
  if (album) {
    where = 'album = @album COLLATE NOCASE'
    params.album = album
    const aa = query.albumArtist?.trim()
    if (aa) {
      where += ' AND COALESCE(album_artist, artist) = @aa COLLATE NOCASE'
      params.aa = aa
    }
    order = 'disc_no, track_no, title COLLATE NOCASE'
  } else if (artist) {
    where = '(artist = @artist COLLATE NOCASE OR album_artist = @artist COLLATE NOCASE)'
    params.artist = artist
    if (query.noAlbum) {
      where += " AND (album IS NULL OR album = '')"
      order = 'title COLLATE NOCASE'
    } else {
      order = 'album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE'
    }
  }

  const whereSql = where ? `WHERE ${where}` : ''
  const total = (
    db.prepare(`SELECT COUNT(*) AS c FROM tracks ${whereSql}`).get(params) as { c: number }
  ).c
  const rows = db
    .prepare(`SELECT * FROM tracks ${whereSql} ORDER BY ${order} LIMIT @limit OFFSET @offset`)
    .all({ ...params, limit, offset }) as TrackRow[]
  return { tracks: rows.map(rowToTrack), total, limit, offset }
}

export function queryArtists(query: { search?: string; limit?: number; offset?: number }): {
  artists: { artist: string; trackCount: number; albumCount: number }[]
  total: number
} {
  const db = getDb()
  const limit = Number.isFinite(query.limit) ? Math.min(Math.max(query.limit as number, 1), 1000) : 200
  const offset = Number.isFinite(query.offset) ? Math.max(query.offset as number, 0) : 0
  const search = query.search?.trim()
  const like = search ? `%${search}%` : null
  const filter = "artist IS NOT NULL AND artist <> ''" + (like ? ' AND artist LIKE @like' : '')

  const total = (
    db
      .prepare(`SELECT COUNT(*) AS c FROM (SELECT 1 FROM tracks WHERE ${filter} GROUP BY artist COLLATE NOCASE)`)
      .get({ like }) as { c: number }
  ).c
  const artists = db
    .prepare(
      `SELECT artist, COUNT(*) AS trackCount, COUNT(DISTINCT album) AS albumCount
       FROM tracks WHERE ${filter}
       GROUP BY artist COLLATE NOCASE
       ORDER BY artist COLLATE NOCASE
       LIMIT @limit OFFSET @offset`
    )
    .all({ like, limit, offset }) as { artist: string; trackCount: number; albumCount: number }[]
  return { artists, total }
}

export function queryAlbums(query: {
  search?: string
  artist?: string
  limit?: number
  offset?: number
}): {
  albums: { album: string; artist: string; artHash: string | null; trackCount: number; year: number | null }[]
  total: number
} {
  const db = getDb()
  const limit = Number.isFinite(query.limit) ? Math.min(Math.max(query.limit as number, 1), 1000) : 200
  const offset = Number.isFinite(query.offset) ? Math.max(query.offset as number, 0) : 0
  const search = query.search?.trim()
  const artist = query.artist?.trim()
  const like = search ? `%${search}%` : null

  let filter = "album IS NOT NULL AND album <> ''"
  const params: Record<string, unknown> = { limit, offset }
  if (like) {
    filter += ' AND (album LIKE @like OR COALESCE(album_artist, artist) LIKE @like)'
    params.like = like
  }
  if (artist) {
    filter += ' AND (artist = @artist COLLATE NOCASE OR album_artist = @artist COLLATE NOCASE)'
    params.artist = artist
  }

  const total = (
    db
      .prepare(
        `SELECT COUNT(*) AS c FROM (
           SELECT 1 FROM tracks WHERE ${filter}
           GROUP BY album COLLATE NOCASE, COALESCE(album_artist, artist) COLLATE NOCASE)`
      )
      .get(params) as { c: number }
  ).c
  const albums = db
    .prepare(
      `SELECT album,
              COALESCE(album_artist, artist) AS artist,
              COUNT(*) AS trackCount,
              MAX(art_hash) AS artHash,
              MAX(year) AS year
       FROM tracks WHERE ${filter}
       GROUP BY album COLLATE NOCASE, COALESCE(album_artist, artist) COLLATE NOCASE
       ORDER BY year, album COLLATE NOCASE
       LIMIT @limit OFFSET @offset`
    )
    .all(params) as {
    album: string
    artist: string
    artHash: string | null
    trackCount: number
    year: number | null
  }[]
  return { albums, total }
}

export function getTrackById(id: number): Track | null {
  const row = getDb().prepare('SELECT * FROM tracks WHERE id = ?').get(id) as TrackRow | undefined
  return row ? rowToTrack(row) : null
}

/** Internal: absolute file path for streaming/playback (M4). */
export function getTrackPath(id: number): string | null {
  const row = getDb().prepare('SELECT path FROM tracks WHERE id = ?').get(id) as
    | { path: string }
    | undefined
  return row?.path ?? null
}

export function getArt(hash: string): { mime: string; data: Buffer } | null {
  const row = getDb().prepare('SELECT mime, data FROM art WHERE hash = ?').get(hash) as
    | { mime: string; data: Buffer }
    | undefined
  return row ?? null
}
