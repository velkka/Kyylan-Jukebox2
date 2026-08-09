import { FormEvent, useState } from 'react'
import { runSetup } from './api'
import type { PublicConfig } from '@shared/types'
import logo from './logo.png'

export default function FirstRunSetup({
  config,
  onDone
}: {
  config: PublicConfig
  onDone: () => void
}): JSX.Element {
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [port, setPort] = useState(String(config.port))
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  async function submit(e: FormEvent): Promise<void> {
    e.preventDefault()
    setError(null)
    setNote(null)
    if (password.length < 1) return setError('Choose an admin password.')
    if (password !== confirm) return setError('Passwords do not match.')
    const portNum = Number(port)
    if (!Number.isInteger(portNum) || portNum < 1 || portNum > 65535)
      return setError('Port must be a whole number between 1 and 65535.')

    setBusy(true)
    try {
      const result = await runSetup({ adminPassword: password, port: portNum })
      if (result.restartRequired) {
        setNote(
          `Saved. The new port (${result.port}) takes effect after you restart the app.`
        )
        setBusy(false)
        return
      }
      onDone()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setBusy(false)
    }
  }

  return (
    <div className="min-h-full flex items-center justify-center p-8">
      <form
        onSubmit={submit}
        className="w-full max-w-md rounded-2xl bg-jukebox-panel p-8 shadow-2xl"
      >
        <div className="text-center">
          <img src={logo} alt="Kyylan Jukebox" className="mx-auto h-12 w-auto" />
          <h1 className="mt-3 text-2xl font-bold">Welcome to Kyylan Jukebox</h1>
          <p className="mt-1 text-sm text-white/60">
            First-time setup — create the admin password.
          </p>
        </div>

        <label className="mt-6 block text-sm">
          <span className="text-white/70">Admin password</span>
          <input
            type="password"
            value={password}
            autoFocus
            onChange={(e) => setPassword(e.target.value)}
            className="mt-1 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
          />
        </label>

        <label className="mt-4 block text-sm">
          <span className="text-white/70">Confirm password</span>
          <input
            type="password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            className="mt-1 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
          />
        </label>

        <label className="mt-4 block text-sm">
          <span className="text-white/70">Server port</span>
          <input
            type="number"
            value={port}
            min={1}
            max={65535}
            onChange={(e) => setPort(e.target.value)}
            className="mt-1 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
          />
          <span className="mt-1 block text-xs text-white/40">
            Guests connect at http://&lt;this-computer&gt;:{port || '…'}
          </span>
        </label>

        <p className="mt-4 rounded-lg bg-amber-500/10 px-3 py-2 text-xs text-amber-300/90">
          The password is stored in plaintext on this computer (LAN party
          convenience). Don't reuse a sensitive password.
        </p>

        {error && <p className="mt-3 text-sm text-red-400">{error}</p>}
        {note && <p className="mt-3 text-sm text-jukebox-accent2">{note}</p>}

        <button
          type="submit"
          disabled={busy}
          className="mt-5 w-full rounded-lg bg-jukebox-accent px-4 py-2.5 font-semibold text-white transition hover:brightness-110 disabled:opacity-50"
        >
          {busy ? 'Saving…' : 'Finish setup'}
        </button>
      </form>
    </div>
  )
}
