import { useCallback, useEffect, useState } from 'react'
import type { AlbumSummary, ArtistSummary, Track } from '@shared/types'
import { getAlbums, getArtists, getTracks } from '../api'
import { useJukebox } from '../JukeboxContext'
import { formatTime, subtitle } from '../util'
import TrackArt from './TrackArt'

type Tab = 'search' | 'browse'
const PAGE = 60

export default function Library({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const [tab, setTab] = useState<Tab>('browse')

  return (
    <section className="rounded-2xl bg-jukebox-panel p-4">
      <div className="mb-3 flex items-center gap-1">
        {(['browse', 'search'] as Tab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`rounded-lg px-3 py-1.5 text-sm font-medium capitalize ${
              tab === t ? 'bg-jukebox-accent text-white' : 'text-white/60 hover:bg-white/5'
            }`}
          >
            {t}
          </button>
        ))}
      </div>

      {tab === 'search' ? <SearchTab onError={onError} /> : <BrowseTab onError={onError} />}
    </section>
  )
}

// ---- Search tab --------------------------------------------------------------

function SearchTab({ onError }: { onError: (m: string) => void }): JSX.Element {
  const [search, setSearch] = useState('')
  const [tracks, setTracks] = useState<Track[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(false)

  const load = useCallback(
    async (offset: number) => {
      setLoading(true)
      try {
        const r = await getTracks({ search, limit: PAGE, offset })
        setTotal(r.total)
        setTracks((p) => (offset === 0 ? r.tracks : [...p, ...r.tracks]))
      } catch (e) {
        onError(e instanceof Error ? e.message : String(e))
      } finally {
        setLoading(false)
      }
    },
    [search, onError]
  )

  useEffect(() => {
    if (!search.trim()) {
      setTracks([])
      setTotal(0)
      return
    }
    const t = setTimeout(() => load(0), 250)
    return () => clearTimeout(t)
  }, [search, load])

  return (
    <>
      <input
        value={search}
        autoFocus
        onChange={(e) => setSearch(e.target.value)}
        placeholder="Search songs, artists, albums…"
        className="w-full rounded-lg bg-black/40 px-4 py-2.5 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
      />
      {!search.trim() ? (
        <Empty text="Type to search the library." />
      ) : !loading && tracks.length === 0 ? (
        <Empty text="No matches." />
      ) : (
        <>
          <p className="mb-1 mt-3 text-xs text-white/40">{total} results</p>
          <ul className="space-y-1">
            {tracks.map((t) => (
              <AddRow key={t.id} track={t} sub={subtitle(t.artist, t.album)} onError={onError} />
            ))}
          </ul>
          <LoadMore shown={tracks.length} total={total} loading={loading} onClick={() => load(tracks.length)} />
        </>
      )}
    </>
  )
}

// ---- Browse tab (Artists → Albums → Songs) -----------------------------------

function BrowseTab({ onError }: { onError: (m: string) => void }): JSX.Element {
  const [artist, setArtist] = useState<string | null>(null)
  const [album, setAlbum] = useState<AlbumSummary | null>(null)
  // Kept here so the chosen letter survives drilling into an artist and back.
  const [letter, setLetter] = useState<string | null>(null)

  if (artist && album) {
    return (
      <AlbumSongs
        album={album}
        onBack={() => setAlbum(null)}
        onError={onError}
      />
    )
  }
  if (artist) {
    return (
      <ArtistAlbums
        artist={artist}
        onBack={() => setArtist(null)}
        onOpen={(a) => setAlbum(a)}
        onError={onError}
      />
    )
  }
  return (
    <ArtistsList
      letter={letter}
      onLetter={setLetter}
      onOpen={(name) => setArtist(name)}
      onError={onError}
    />
  )
}

const ALPHABET = [...'ABCDEFGHIJKLMNOPQRSTUVWXYZ', '#']

function ArtistsList({
  letter,
  onLetter,
  onOpen,
  onError
}: {
  letter: string | null
  onLetter: (l: string | null) => void
  onOpen: (name: string) => void
  onError: (m: string) => void
}): JSX.Element {
  const [artists, setArtists] = useState<ArtistSummary[]>([])
  const [total, setTotal] = useState(0)
  const [available, setAvailable] = useState<Set<string>>(new Set())
  const [loading, setLoading] = useState(false)

  const load = useCallback(
    async (offset: number) => {
      setLoading(true)
      try {
        const r = await getArtists({ letter: letter ?? undefined, limit: PAGE, offset })
        setTotal(r.total)
        setAvailable(new Set(r.letters))
        setArtists((p) => (offset === 0 ? r.artists : [...p, ...r.artists]))
      } catch (e) {
        onError(e instanceof Error ? e.message : String(e))
      } finally {
        setLoading(false)
      }
    },
    [letter, onError]
  )
  useEffect(() => {
    load(0)
  }, [load])

  const bar = (
    <div className="mb-2 flex flex-wrap gap-0.5">
      <button
        onClick={() => onLetter(null)}
        className={`rounded px-1.5 py-0.5 text-xs font-medium ${
          letter === null ? 'bg-jukebox-accent text-white' : 'text-white/60 hover:bg-white/10'
        }`}
      >
        All
      </button>
      {ALPHABET.map((l) => {
        const has = available.has(l)
        return (
          <button
            key={l}
            onClick={() => has && onLetter(letter === l ? null : l)}
            disabled={!has}
            aria-label={`Artists starting with ${l}`}
            className={`w-6 rounded py-0.5 text-xs font-medium tabular-nums ${
              letter === l
                ? 'bg-jukebox-accent text-white'
                : has
                  ? 'text-white/60 hover:bg-white/10'
                  : 'cursor-default text-white/15'
            }`}
          >
            {l}
          </button>
        )
      })}
    </div>
  )

  if (!loading && artists.length === 0) {
    return (
      <>
        {bar}
        <Empty text={letter ? `No artists under “${letter}”.` : 'No music yet.'} />
      </>
    )
  }

  return (
    <>
      {bar}
      <p className="mb-1 text-xs text-white/40">
        {total} artist{total === 1 ? '' : 's'}
        {letter ? ` under “${letter}”` : ''}
      </p>
      <ul className="space-y-1">
        {artists.map((a) => (
          <li key={a.artist}>
            <button
              onClick={() => onOpen(a.artist)}
              className="flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left hover:bg-white/5"
            >
              <div className="grid h-11 w-11 shrink-0 place-items-center rounded-md bg-white/5 text-white/40">
                {a.artist.slice(0, 1).toUpperCase()}
              </div>
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{a.artist}</p>
                <p className="truncate text-xs text-white/50">
                  {a.trackCount} song{a.trackCount === 1 ? '' : 's'} · {a.albumCount} album
                  {a.albumCount === 1 ? '' : 's'}
                </p>
              </div>
              <span className="shrink-0 text-white/30">›</span>
            </button>
          </li>
        ))}
      </ul>
      <LoadMore shown={artists.length} total={total} loading={loading} onClick={() => load(artists.length)} />
    </>
  )
}

function ArtistAlbums({
  artist,
  onBack,
  onOpen,
  onError
}: {
  artist: string
  onBack: () => void
  onOpen: (a: AlbumSummary) => void
  onError: (m: string) => void
}): JSX.Element {
  const [albums, setAlbums] = useState<AlbumSummary[]>([])
  const [looseTracks, setLooseTracks] = useState<Track[]>([])
  useEffect(() => {
    getAlbums({ artist, limit: 500 })
      .then((r) => setAlbums(r.albums))
      .catch((e) => onError(String(e.message ?? e)))
    // Songs by this artist that aren't part of any album.
    getTracks({ artist, noAlbum: true, limit: 500 })
      .then((r) => setLooseTracks(r.tracks))
      .catch((e) => onError(String(e.message ?? e)))
  }, [artist, onError])

  return (
    <>
      <DetailHeader title={artist} subtitle={`${albums.length} album${albums.length === 1 ? '' : 's'}`} onBack={onBack} />
      <ul className="mt-3 space-y-1">
        {albums.map((a) => (
          <li key={`${a.album}::${a.artist}`}>
            <button
              onClick={() => onOpen(a)}
              className="flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left hover:bg-white/5"
            >
              <TrackArt hash={a.artHash} size={48} />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{a.album}</p>
                <p className="truncate text-xs text-white/50">
                  {a.year ? `${a.year} · ` : ''}
                  {a.trackCount} song{a.trackCount === 1 ? '' : 's'}
                </p>
              </div>
              <span className="shrink-0 text-white/30">›</span>
            </button>
          </li>
        ))}
      </ul>

      {looseTracks.length > 0 && (
        <>
          <p className="mb-1 mt-4 text-xs font-medium uppercase tracking-wide text-white/40">
            Songs without an album
          </p>
          <ul className="space-y-1">
            {looseTracks.map((t) => (
              <AddRow key={t.id} track={t} sub={t.genre ?? artist} onError={onError} />
            ))}
          </ul>
        </>
      )}
    </>
  )
}

function AlbumSongs({
  album,
  onBack,
  onError
}: {
  album: AlbumSummary
  onBack: () => void
  onError: (m: string) => void
}): JSX.Element {
  const [tracks, setTracks] = useState<Track[]>([])
  useEffect(() => {
    getTracks({ album: album.album, albumArtist: album.artist, limit: 500 })
      .then((r) => setTracks(r.tracks))
      .catch((e) => onError(String(e.message ?? e)))
  }, [album, onError])

  return (
    <>
      <DetailHeader title={album.album} subtitle={album.artist} onBack={onBack} />
      <div className="mt-3 flex items-center gap-3">
        <TrackArt hash={album.artHash} size={72} />
        <p className="text-sm text-white/60">
          {tracks.length} song{tracks.length === 1 ? '' : 's'}
          {album.year ? ` · ${album.year}` : ''}
        </p>
      </div>
      <ul className="mt-3 space-y-1">
        {tracks.map((t) => (
          <AddRow key={t.id} track={t} sub={`${t.trackNo ? `${t.trackNo}. ` : ''}${t.artist ?? album.artist}`} onError={onError} />
        ))}
      </ul>
    </>
  )
}

// ---- Shared bits -------------------------------------------------------------

function AddRow({
  track,
  sub,
  onError
}: {
  track: Track
  sub: string
  onError: (msg: string) => void
}): JSX.Element {
  const { add } = useJukebox()
  const [adding, setAdding] = useState(false)
  async function onAdd(): Promise<void> {
    setAdding(true)
    try {
      await add(track.id)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setAdding(false)
    }
  }
  return (
    <li className="flex items-center gap-3 rounded-lg px-2 py-1.5 hover:bg-white/5">
      <TrackArt hash={track.artHash} size={44} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">{track.title}</p>
        <p className="truncate text-xs text-white/50">{sub}</p>
      </div>
      {track.duration != null && (
        <span className="hidden shrink-0 text-xs tabular-nums text-white/35 sm:inline">
          {formatTime(track.duration)}
        </span>
      )}
      <button
        onClick={onAdd}
        disabled={adding}
        className="shrink-0 rounded-lg bg-jukebox-accent/90 px-3 py-1.5 text-sm font-medium text-white hover:bg-jukebox-accent disabled:opacity-50"
      >
        {adding ? '…' : '+ Add'}
      </button>
    </li>
  )
}

function LoadMore({
  shown,
  total,
  loading,
  onClick
}: {
  shown: number
  total: number
  loading: boolean
  onClick: () => void
}): JSX.Element | null {
  if (shown >= total) return null
  return (
    <button
      onClick={onClick}
      disabled={loading}
      className="mt-3 w-full rounded-lg bg-white/5 py-2 text-sm text-white/70 hover:bg-white/10 disabled:opacity-50"
    >
      {loading ? 'Loading…' : `Load more (${total - shown} left)`}
    </button>
  )
}

function DetailHeader({
  title,
  subtitle: sub,
  onBack
}: {
  title: string
  subtitle: string
  onBack: () => void
}): JSX.Element {
  return (
    <div className="flex items-center gap-2">
      <button
        onClick={onBack}
        className="shrink-0 rounded-lg bg-white/5 px-2.5 py-1.5 text-sm text-white/70 hover:bg-white/10"
      >
        ‹ Back
      </button>
      <div className="min-w-0">
        <p className="truncate font-semibold">{title}</p>
        <p className="truncate text-xs text-white/50">{sub}</p>
      </div>
    </div>
  )
}

function Empty({ text }: { text: string }): JSX.Element {
  return <p className="py-8 text-center text-sm text-white/35">{text}</p>
}
