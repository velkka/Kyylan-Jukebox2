import { useEffect, useState } from 'react'
import type { AudioDevice, ScanStatus } from '@shared/types'
import * as api from '../api'

export default function AdminPanel({ onError }: { onError: (msg: string) => void }): JSX.Element {
  return (
    <section className="space-y-4 rounded-2xl border border-jukebox-accent/30 bg-jukebox-panel p-4">
      <h3 className="flex items-center gap-2 font-semibold text-jukebox-accent">
        <span>⚙︎</span> Admin controls
      </h3>
      <OutputDevice onError={onError} />
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

function MusicFolders({ onError }: { onError: (msg: string) => void }): JSX.Element {
  const [paths, setPaths] = useState<string[]>([])
  const [input, setInput] = useState('')
  const [scan, setScan] = useState<ScanStatus | null>(null)

  useEffect(() => {
    api.getPaths().then((r) => setPaths(r.paths)).catch((e) => onError(String(e.message ?? e)))
  }, [])

  async function add(): Promise<void> {
    if (!input.trim()) return
    try {
      const r = await api.addLibraryPath(input.trim())
      setPaths(r.paths)
      setInput('')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    }
  }

  async function remove(p: string): Promise<void> {
    try {
      setPaths((await api.removeLibraryPath(p)).paths)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    }
  }

  async function rescan(): Promise<void> {
    try {
      await api.startScan()
      const poll = setInterval(async () => {
        const s = await api.getScanStatus()
        setScan(s)
        if (!s.scanning) clearInterval(poll)
      }, 500)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <Group title="Music folders">
      {paths.length === 0 ? (
        <p className="mb-2 text-sm text-white/40">No folders yet.</p>
      ) : (
        <ul className="mb-2 space-y-1">
          {paths.map((p) => (
            <li key={p} className="flex items-center gap-2 text-sm">
              <span className="min-w-0 flex-1 truncate font-mono text-white/70">{p}</span>
              <button
                onClick={() => remove(p)}
                className="shrink-0 rounded-md px-2 py-0.5 text-xs text-white/50 hover:bg-red-500/20 hover:text-red-300"
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
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
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setLimit(String(s.perUserQueueLimit))
        setPort(String(s.port))
      })
      .catch((e) => onError(String(e.message ?? e)))
  }, [])

  async function save(): Promise<void> {
    setBusy(true)
    setNote(null)
    try {
      const r = await api.saveSettings({
        perUserQueueLimit: Number(limit),
        port: Number(port)
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
