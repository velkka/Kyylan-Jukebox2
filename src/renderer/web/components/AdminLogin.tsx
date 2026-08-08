import { FormEvent, useState } from 'react'
import { useJukebox } from '../JukeboxContext'

export default function AdminLogin({
  open,
  onClose
}: {
  open: boolean
  onClose: () => void
}): JSX.Element | null {
  const { login } = useJukebox()
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  if (!open) return null

  async function submit(e: FormEvent): Promise<void> {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await login(password)
      setPassword('')
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onClick={(e) => e.stopPropagation()}
        onSubmit={submit}
        className="w-full max-w-sm rounded-2xl bg-jukebox-panel p-6 shadow-2xl"
      >
        <h2 className="text-lg font-semibold">Admin login</h2>
        <p className="mt-1 text-sm text-white/50">Enter the admin password to manage the jukebox.</p>
        <input
          type="password"
          autoFocus
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder="Password"
          className="mt-4 w-full rounded-lg bg-black/40 px-3 py-2 outline-none ring-1 ring-white/10 focus:ring-jukebox-accent"
        />
        {error && <p className="mt-2 text-sm text-red-400">{error}</p>}
        <div className="mt-4 flex gap-2">
          <button
            type="button"
            onClick={onClose}
            className="flex-1 rounded-lg bg-white/5 py-2 text-sm hover:bg-white/10"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={busy}
            className="flex-1 rounded-lg bg-jukebox-accent py-2 text-sm font-semibold text-white hover:brightness-110 disabled:opacity-50"
          >
            {busy ? 'Checking…' : 'Log in'}
          </button>
        </div>
      </form>
    </div>
  )
}
