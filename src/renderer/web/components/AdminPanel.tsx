import { useCallback, useEffect, useState } from 'react'
import type { AudioDevice, LibraryPath, ScanStatus, StandbyEntry, Track } from '@shared/types'
import * as api from '../api'
import { subtitle } from '../util'
import TrackArt from './TrackArt'

export default function AdminPanel({ onError }: { onError: (msg: string) => void }): JSX.Element {
  return (
    <section className="space-y-4 rounded-2xl border border-jukebox-accent/30 bg-jukebox-panel p-4">
      <h3 className="flex items-center gap-2 font-semibold text-jukebox-accent">
        <span>⚙︎</span> Admin controls
      </h3>
      <OutputDevice onError={onError} />
      <Standby onError={onError} />
      <MusicFolders onError={onError} />
      <Settings onError={onError} />
    </section>
  )
}

function Group({ title, children }: { title: string; children: React.ReactNode }): JSX.Element {
  return (
    <div className="rounded-xl bg-black/25 p-3">
      <p className="mb-2 text-xs font-medium uppercase tracking-wide text-white/40">{title}</p>
      {children}
    </div>
  )
}

function OutputDevice({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const [devices, setDevices] = useState<AudioDevice[]>([])
  const [selected, setSelected] = useState<string>('')

  const load = (): void => {
    api
      .getDevices()
      .then((d) => {
        setDevices(d.devices)
        setSelected(d.selected ?? d.devices[0]?.deviceId ?? '')
      })
      .catch((e) => onError(String(e.message ?? e)))
  }
  useEffect(load, [])

  function change(deviceId: string): void {
    setSelected(deviceId)
    api.setOutput(deviceId).catch((e) => onError(String(e.message ?? e)))
  }

  return (
    <Group title="Audio output">
      <div className="flex gap-2">
        <select
          value={selected}
          onChange={(e) => change(e.target.value)}
          className="min-w-0 flex-1 rounded-lg bg-black/40 px-3 py-2 text-sm outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
        >
          {devices.length === 0 && <option>No devices found</option>}
          {devices.map((d) => (
            <option key={d.deviceId} value={d.deviceId}>
              {d.label}
            </option>
          ))}
        </select>
        <button
          onClick={load}
          className="shrink-0 rounded-lg bg-white/10 px-3 text-sm hover:bg-white/20"
          title="Refresh device list"
        >
          ↻
        </button>
      </div>
    </Group>
  )
}

function Toggle({
  label,
  checked,
  onChange
}: {
  label: string
  checked: boolean
  onChange: (v: boolean) => void
}): JSX.Element {
  return (
    <label className="flex cursor-pointer items-center gap-2 text-sm text-white/70">
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative h-5 w-9 shrink-0 rounded-full transition ${
          checked ? 'bg-jukebox-accent' : 'bg-white/15'
        }`}
      >
        <span
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition-all ${
            checked ? 'left-[18px]' : 'left-0.5'
          }`}
        />
      </button>
      {label}
    </label>
  )
}

function Standby({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const [enabled, setEnabled] = useState(false)
  const [shuffle, setShuffle] = useState(false)
  const [entries, setEntries] = useState<StandbyEntry[]>([])
  const [search, setSearch] = useState('')
  const [results, setResults] = useState<Track[]>([])

  const apply = (s: { enabled: boolean; shuffle: boolean; entries: StandbyEntry[] }): void => {
    setEnabled(s.enabled)
    setShuffle(s.shuffle)
    setEntries(s.entries)
  }
  const fail = (e: unknown): void => onError(e instanceof Error ? e.message : String(e))

  useEffect(() => {
    api.getStandby().then(apply).catch(fail)
  }, [])

  useEffect(() => {
    if (!search.trim()) {
      setResults([])
      return
    }
    const t = setTimeout(() => {
      api.getTracks({ search, limit: 8 }).then((r) => setResults(r.tracks)).catch(() => setResults([]))
    }, 250)
    return () => clearTimeout(t)
  }, [search])

  return (
    <Group title="Standby playlist">
      <p className="-mt-1 mb-2 text-xs text-white/40">
        Played automatically when the guest queue is empty.
      </p>
      <div className="flex flex-wrap items-center gap-5">
        <Toggle
          label="Enabled"
          checked={enabled}
          onChange={(v) => api.setStandbySettings({ enabled: v }).then(apply).catch(fail)}
        />
        <Toggle
          label="Shuffle"
          checked={shuffle}
          onChange={(v) => api.setStandbySettings({ shuffle: v }).then(apply).catch(fail)}
        />
      </div>

      {entries.length === 0 ? (
        <p className="mt-3 text-sm text-white/40">No standby songs yet — add some below.</p>
      ) : (
        <>
          <ul className="mt-3 space-y-1">
            {entries.map((e) => (
              <li key={e.id} className="flex items-center gap-2 text-sm">
                <TrackArt hash={e.track.artHash} size={32} />
                <div className="min-w-0 flex-1">
                  <p className="truncate">{e.track.title}</p>
                  <p className="truncate text-xs text-white/45">
                    {subtitle(e.track.artist, e.track.album)}
                  </p>
                </div>
                <button
                  onClick={() => api.removeStandby(e.id).then(apply).catch(fail)}
                  className="shrink-0 rounded-md px-2 py-0.5 text-xs text-white/50 hover:bg-red-500/20 hover:text-red-300"
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
          <button
            onClick={() => api.clearStandby().then(apply).catch(fail)}
            className="mt-2 text-xs text-white/40 hover:text-red-300"
          >
            Clear all
          </button>
        </>
      )}

      <input
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        placeholder="Search to add songs…"
        className="mt-3 w-full rounded-lg bg-black/40 px-3 py-2 text-sm outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
      />
      {results.length > 0 && (
        <ul className="mt-1 space-y-1">
          {results.map((t) => (
            <li
              key={t.id}
              className="flex items-center gap-2 rounded-lg px-2 py-1 text-sm hover:bg-white/5"
            >
              <div className="min-w-0 flex-1">
                <p className="truncate">{t.title}</p>
                <p className="truncate text-xs text-white/45">{subtitle(t.artist, t.album)}</p>
              </div>
              <button
                onClick={() => api.addStandby(t.id).then(apply).catch(fail)}
                className="shrink-0 rounded-md bg-white/10 px-2 py-1 text-xs hover:bg-white/20"
              >
                + Standby
              </button>
            </li>
          ))}
        </ul>
      )}
    </Group>
  )
}

function MusicFolders({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const [paths, setPaths] = useState<LibraryPath[]>([])
  const [total, setTotal] = useState(0)
  const [input, setInput] = useState('')
  const [scan, setScan] = useState<ScanStatus | null>(null)

  const apply = (r: { paths: LibraryPath[]; total: number }): void => {
    setPaths(r.paths)
    setTotal(r.total)
  }
  const fail = (e: unknown): void => onError(e instanceof Error ? e.message : String(e))

  const load = useCallback(() => {
    api.getPaths().then(apply).catch(fail)
  }, [])
  useEffect(load, [load])

  async function add(): Promise<void> {
    if (!input.trim()) return
    try {
      apply(await api.addLibraryPath(input.trim()))
      setInput('')
    } catch (e) {
      fail(e)
    }
  }

  async function remove(p: string): Promise<void> {
    try {
      apply(await api.removeLibraryPath(p))
    } catch (e) {
      fail(e)
    }
  }

  async function rescan(): Promise<void> {
    try {
      await api.startScan()
      const poll = setInterval(async () => {
        const s = await api.getScanStatus()
        setScan(s)
        if (!s.scanning) {
          clearInterval(poll)
          load() // refresh per-folder counts once the scan settles
        }
      }, 500)
    } catch (e) {
      fail(e)
    }
  }

  const n = (v: number): string => v.toLocaleString()

  return (
    <Group title="Music folders">
      {paths.length === 0 ? (
        <p className="mb-2 text-sm text-white/40">No folders yet.</p>
      ) : (
        <>
          <ul className="mb-1 space-y-1">
            {paths.map((p) => (
              <li key={p.path} className="flex items-center gap-2 text-sm">
                <span className="min-w-0 flex-1 truncate font-mono text-white/70">{p.path}</span>
                <span className="shrink-0 tabular-nums text-xs text-white/45">
                  {n(p.trackCount)} song{p.trackCount === 1 ? '' : 's'}
                </span>
                <button
                  onClick={() => remove(p.path)}
                  className="shrink-0 rounded-md px-2 py-0.5 text-xs text-white/50 hover:bg-red-500/20 hover:text-red-300"
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
          <p className="mb-2 border-t border-white/10 pt-1 text-right text-xs text-white/50">
            Total: <span className="tabular-nums font-medium text-white/70">{n(total)}</span> songs
          </p>
        </>
      )}
      <div className="flex gap-2">
        <input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && add()}
          placeholder="/path/to/music"
          className="min-w-0 flex-1 rounded-lg bg-black/40 px-3 py-2 text-sm outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
        />
        <button onClick={add} className="shrink-0 rounded-lg bg-white/10 px-3 text-sm hover:bg-white/20">
          Add
        </button>
      </div>
      <div className="mt-2 flex items-center gap-3">
        <button
          onClick={rescan}
          disabled={scan?.scanning}
          className="rounded-lg bg-jukebox-accent/90 px-3 py-1.5 text-sm font-medium hover:bg-jukebox-accent disabled:opacity-50"
        >
          {scan?.scanning ? 'Scanning…' : 'Rescan library'}
        </button>
        {scan && (
          <span className="text-xs text-white/50">
            {scan.scanning
              ? `${scan.processed} scanned…`
              : `${scan.total} songs · +${scan.added} −${scan.removed}`}
          </span>
        )}
      </div>
    </Group>
  )
}

function Settings({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const [limit, setLimit] = useState('')
  const [port, setPort] = useState('')
  const [downvotes, setDownvotes] = useState('')
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setLimit(String(s.perUserQueueLimit))
        setPort(String(s.port))
        setDownvotes(String(s.downvoteSkipThreshold))
      })
      .catch((e) => onError(String(e.message ?? e)))
  }, [])

  async function save(): Promise<void> {
    setBusy(true)
    setNote(null)
    try {
      const r = await api.saveSettings({
        perUserQueueLimit: Number(limit),
        port: Number(port),
        downvoteSkipThreshold: Number(downvotes)
      })
      setNote(r.restartRequired ? 'Saved. Restart the app for the new port to take effect.' : 'Saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Group title="Settings">
      <div className="grid grid-cols-2 gap-3">
        <label className="text-sm">
          <span className="text-white/60">Songs per guest</span>
          <input
            type="number"
            min={1}
            max={100}
            value={limit}
            onChange={(e) => setLimit(e.target.value)}
            className="mt-1 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
          />
        </label>
        <label className="text-sm">
          <span className="text-white/60">Server port</span>
          <input
            type="number"
            min={1}
            max={65535}
            value={port}
            onChange={(e) => setPort(e.target.value)}
            className="mt-1 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
          />
        </label>
        <label className="text-sm">
          <span className="text-white/60">Downvotes to skip</span>
          <input
            type="number"
            min={-100}
            max={100}
            value={downvotes}
            onChange={(e) => setDownvotes(e.target.value)}
            className="mt-1 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
          />
          <span className="mt-1 block text-xs text-white/40">
            0 = disabled · negative = hide the count
          </span>
        </label>
      </div>
      {note && <p className="mt-2 text-xs text-jukebox-accent2">{note}</p>}
      <button
        onClick={save}
        disabled={busy}
        className="mt-3 rounded-lg bg-jukebox-accent px-4 py-1.5 text-sm font-semibold text-white hover:brightness-110 disabled:opacity-50"
      >
        {busy ? 'Saving…' : 'Save settings'}
      </button>
    </Group>
  )
}
