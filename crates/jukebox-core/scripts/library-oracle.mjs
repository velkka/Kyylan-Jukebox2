// Records what Electron's own library code does with the library fixtures, as the expected
// results for the Rust library tests.
//
// This runs the real src/main/library.ts — its walk, upsertFile, scanAll and every query —
// bundled with esbuild and run inside Electron's Node, so the better-sqlite3 and
// music-metadata it uses are exactly the Electron build's. Only `electron` itself is
// stubbed, to point the data directory at a scratch folder. Run from the repository root,
// after `npm ci`:
//
//   node crates/jukebox-core/scripts/library-oracle.mjs
//
// It writes tests/fixtures/library-electron.json. Paths are recorded with the scratch copy
// of the fixtures replaced by /music.

import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const REPO = resolve(fileURLToPath(import.meta.url), '../../../..')
const FIXTURES = join(REPO, 'crates/jukebox-core/tests/fixtures')
const scratch = mkdtempSync(join(tmpdir(), 'kyylan-library-oracle-'))

// ---- The script Electron runs -----------------------------------------------------------

const entry = String.raw`
import { cpSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { join } from 'node:path'
import { getDb } from './src/main/db'
import { updateConfig } from './src/main/config'
import * as lib from './src/main/library'

async function main() {

const SCRATCH = process.env.ORACLE_SCRATCH
const ROOT = join(SCRATCH, 'music')
const OUT = process.env.ORACLE_OUT
const hide = (s) => (typeof s === 'string' ? s.split(ROOT).join('/music') : s)
const hideAll = (v) => JSON.parse(JSON.stringify(v, (_k, x) => hide(x)))

cpSync(process.env.ORACLE_FIXTURES + '/library', ROOT, { recursive: true })
const db = getDb()

const status = (s) => ({ ...s, startedAt: s.startedAt !== null, finishedAt: s.finishedAt !== null })
const tracks = () => db.prepare('SELECT * FROM tracks ORDER BY id').all()
const scan = async (roots, setup) => {
  updateConfig({ libraryPaths: roots })
  const result = await lib.scanAll()
  return hideAll({ setup, roots, status: status(result), tracks: tracks() })
}

const scans = []
scans.push(await scan([ROOT + '/'], 'first scan; the folder has a trailing slash'))
scans.push(await scan([ROOT, join(ROOT, 'Disc 2')], 'nothing changed; the second folder is inside the first'))
utimesSync(join(ROOT, 'untagged.mp3'), new Date('2020-01-01T00:00:00Z'), new Date('2020-01-01T00:00:00Z'))
rmSync(join(ROOT, 'not-audio.mp3'))
scans.push(await scan([ROOT], 'untagged.mp3 modified to 2020-01-01T00:00:00Z; not-audio.mp3 deleted'))

const art = db.prepare('SELECT hash, mime, data FROM art ORDER BY hash').all()
  .map((a) => ({ hash: a.hash, mime: a.mime, data: a.data.toString('base64') }))

// ---- Rows for the queries to find ----

const seed = [
  { title: 'Dancing Queen', artist: 'ABBA', album: 'Greatest', album_artist: 'ABBA', genre: 'Pop', duration: 230.5, track_no: 1, disc_no: 1, year: 1992 },
  { title: 'Waterloo', artist: 'abba', album: 'Greatest', album_artist: null, genre: null, duration: 164, track_no: 2, disc_no: 1, year: 1974 },
  { title: 'Single, With "Quotes"', artist: 'ABBA', album: null, album_artist: null, genre: 'Pop, Disco', duration: 200.25, track_no: null, disc_no: null, year: null },
  { title: 'Empty Album', artist: 'ABBA', album: '', album_artist: null, genre: null, duration: null, track_no: null, disc_no: null, year: null },
  { title: 'Love Will Keep Us', artist: 'Eagles', album: 'Greatest', album_artist: 'Eagles', genre: 'Rock', duration: 301.0625, track_no: 3, disc_no: 2, year: 1994 },
  { title: 'Love Song', artist: 'élan', album: 'Élan', album_artist: null, genre: null, duration: 99.9, track_no: 1, disc_no: null, year: null },
  { title: 'Love Around the World', artist: 'Daft Punk', album: 'Homework', album_artist: null, genre: 'House', duration: 428.1, track_no: 7, disc_no: 1, year: 1997 },
  { title: 'Line\nBreak', artist: 'Daft Punk', album: 'Homework', album_artist: null, genre: null, duration: 60, track_no: 1, disc_no: 1, year: 1997 },
  { title: 'Numbers', artist: '1990s', album: 'Digits', album_artist: null, genre: null, duration: 120, track_no: null, disc_no: null, year: 2007 },
  { title: 'Bridge', artist: 'Øresund', album: null, album_artist: null, genre: null, duration: 180, track_no: null, disc_no: null, year: null },
  { title: 'Irritation', artist: 'Ärsytys', album: 'Äänilevy', album_artist: null, genre: null, duration: 150, track_no: 1, disc_no: null, year: 2011 },
  { title: 'Help!', artist: 'beatles', album: 'Help', album_artist: 'The Beatles', genre: null, duration: 139, track_no: 1, disc_no: null, year: 1965 },
  { title: 'Yesterday', artist: 'Beatles', album: 'Help', album_artist: 'The Beatles', genre: null, duration: 125, track_no: 13, disc_no: null, year: 1965 },
  { title: 'Nameless', artist: '', album: null, album_artist: null, genre: null, duration: 10, track_no: null, disc_no: null, year: null }
]
const insert = db.prepare(
  'INSERT INTO tracks (path, title, artist, album, album_artist, genre, duration, track_no, disc_no, year, art_hash, mtime_ms, added_at, seen) ' +
  "VALUES (@path, @title, @artist, @album, @album_artist, @genre, @duration, @track_no, @disc_no, @year, NULL, 0, '2026-01-01T00:00:00.000Z', 1)"
)
seed.forEach((row, i) => insert.run({ ...row, path: '/seed/' + i + '.mp3' }))

// ---- Queries ----

const firstArt = art[0].hash
const queries = [
  ['tracks', {}],
  ['tracks', { limit: 5, offset: 3 }],
  ['tracks', { limit: 0 }],
  ['tracks', { limit: 9999 }],
  ['tracks', { offset: -5, limit: 2 }],
  ['tracks', { search: 'abba' }],
  ['tracks', { search: '  daft pu  ' }],
  ['tracks', { search: 'aanitys' }],
  ['tracks', { search: 'äänilevy' }],
  ['tracks', { search: '!!!' }],
  ['tracks', { search: 'AC/DC' }],
  ['tracks', { search: 'love', limit: 2, offset: 1 }],
  ['tracks', { search: 'love', artist: 'Eagles' }],
  ['tracks', { album: 'Greatest' }],
  ['tracks', { album: ' greatest ', albumArtist: 'abba' }],
  ['tracks', { album: 'Greatest', albumArtist: '   ' }],
  ['tracks', { artist: 'abba' }],
  ['tracks', { artist: 'ABBA', noAlbum: true }],
  ['tracks', { artist: 'the beatles' }],
  ['tracks', { artist: '   ', limit: 3 }],
  ['artists', {}],
  ['artists', { letter: 'a' }],
  ['artists', { letter: ' e ' }],
  ['artists', { letter: '#' }],
  ['artists', { letter: 'Q' }],
  ['artists', { search: 'ab' }],
  ['artists', { search: 'ab', letter: 'a' }],
  ['artists', { limit: 2, offset: 1 }],
  ['artists', { limit: 0 }],
  ['albums', {}],
  ['albums', { artist: 'abba' }],
  ['albums', { search: 'great' }],
  ['albums', { search: 'beatles' }],
  ['albums', { limit: 1, offset: 1 }],
  ['trackById', { id: 1 }],
  ['trackById', { id: 99999 }],
  ['trackPath', { id: 2 }],
  ['trackPath', { id: 99999 }],
  ['art', { hash: firstArt }],
  ['art', { hash: 'missing' }],
  ['pathsWithCounts', { paths: [ROOT, ROOT + '/', join(ROOT, 'Disc 2'), '/elsewhere'] }]
]
const run = (fn, args) => {
  switch (fn) {
    case 'tracks': return lib.queryTracks(args)
    case 'artists': return lib.queryArtists(args)
    case 'albums': return lib.queryAlbums(args)
    case 'trackById': return lib.getTrackById(args.id)
    case 'trackPath': return lib.getTrackPath(args.id)
    case 'art': {
      const a = lib.getArt(args.hash)
      return a && { mime: a.mime, sha1: createHash('sha1').update(a.data).digest('hex') }
    }
    case 'pathsWithCounts':
      updateConfig({ libraryPaths: args.paths })
      return lib.listPathsWithCounts()
  }
}
const results = queries.map(([fn, args]) => hideAll({ fn, args, result: run(fn, args) }))

// The CSV export route's body, built the way api.ts builds it.
const columns = ['id', 'title', 'artist', 'album', 'albumArtist', 'genre', 'year', 'trackNo', 'discNo', 'duration', 'path', 'addedAt']
const csvCell = (value) => {
  if (value === null || value === undefined) return ''
  const text = String(value)
  return /[",\r\n]/.test(text) ? '"' + text.replace(/"/g, '""') + '"' : text
}
const rows = lib.exportRows()
const csv = '﻿' + [columns.join(','), ...rows.map((r) => columns.map((c) => csvCell(r[c])).join(','))].join('\r\n') + '\r\n'

writeFileSync(OUT, JSON.stringify({
  electron: process.versions.electron,
  musicMetadata: require('music-metadata/package.json').version,
  sqlite: db.prepare('SELECT sqlite_version() AS v').get().v,
  scans,
  art,
  database: hideAll(tracks()),
  queries: results,
  csv: hide(csv)
}, null, 2) + '\n')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
`

// ---- Bundle and run ---------------------------------------------------------------------

try {
  const bundle = join(scratch, 'oracle.cjs')
  writeFileSync(join(scratch, 'electron-stub.cjs'),
    'module.exports = { app: { getPath: () => process.env.ORACLE_SCRATCH } }\n')
  await build({
    stdin: { contents: entry, resolveDir: REPO, loader: 'ts', sourcefile: 'oracle.ts' },
    bundle: true,
    platform: 'node',
    format: 'cjs',
    outfile: bundle,
    external: ['better-sqlite3', 'music-metadata', 'music-metadata/package.json'],
    alias: {
      electron: join(scratch, 'electron-stub.cjs'),
      '@shared/types': join(REPO, 'src/shared/types.ts')
    },
    logLevel: 'warning'
  })
  const out = join(FIXTURES, 'library-electron.json')
  const run = spawnSync(join(REPO, 'node_modules/.bin/electron'), [bundle], {
    stdio: 'inherit',
    env: {
      ...process.env,
      ELECTRON_RUN_AS_NODE: '1',
      NODE_PATH: join(REPO, 'node_modules'),
      ORACLE_SCRATCH: scratch,
      ORACLE_FIXTURES: FIXTURES,
      ORACLE_OUT: out
    }
  })
  if (run.status !== 0) process.exit(run.status ?? 1)
  console.log(`wrote ${out}`)
} finally {
  rmSync(scratch, { recursive: true, force: true })
}
