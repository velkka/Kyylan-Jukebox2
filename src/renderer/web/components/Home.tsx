import { useEffect, useState } from 'react'
import { useJukebox } from '../JukeboxContext'
import NowPlaying from './NowPlaying'
import QueueList from './QueueList'
import Library from './Library'
import AdminLogin from './AdminLogin'
import AdminPanel from './AdminPanel'

export default function Home(): JSX.Element {
  const { connected, isAdmin, name, setName, logout } = useJukebox()
  const [loginOpen, setLoginOpen] = useState(false)
  const [showAdmin, setShowAdmin] = useState(false)
  const [toast, setToast] = useState<string | null>(null)

  useEffect(() => {
    if (!toast) return
    const t = setTimeout(() => setToast(null), 3500)
    return () => clearTimeout(t)
  }, [toast])

  return (
    <div className="mx-auto min-h-full max-w-2xl px-4 pb-24 pt-5">
      <header className="mb-5 flex items-center gap-3">
        <div className="flex items-center gap-2">
          <span className="text-2xl">🎛️</span>
          <span className="text-lg font-bold tracking-tight">Kyylan Jukebox</span>
        </div>
        <span
          className={`h-2 w-2 rounded-full ${connected ? 'bg-green-400' : 'bg-yellow-400'}`}
          title={connected ? 'Live' : 'Reconnecting…'}
        />
        <div className="ml-auto flex items-center gap-2">
          {isAdmin ? (
            <>
              <button
                onClick={() => setShowAdmin((v) => !v)}
                className={`rounded-lg px-3 py-1.5 text-xs font-medium ${
                  showAdmin
                    ? 'bg-jukebox-accent text-white'
                    : 'bg-jukebox-accent/20 text-jukebox-accent hover:bg-jukebox-accent/30'
                }`}
              >
                ⚙︎ Manage
              </button>
              <button
                onClick={logout}
                className="rounded-lg bg-white/5 px-3 py-1.5 text-xs text-white/60 hover:bg-white/10"
              >
                Log out
              </button>
            </>
          ) : (
            <button
              onClick={() => setLoginOpen(true)}
              className="rounded-lg bg-white/5 px-3 py-1.5 text-xs text-white/60 hover:bg-white/10"
            >
              Admin
            </button>
          )}
        </div>
      </header>

      {isAdmin && showAdmin && (
        <div className="mb-4">
          <AdminPanel onError={setToast} />
        </div>
      )}

      <div className="mb-4">
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Your name (shown on songs you add)"
          maxLength={24}
          className="w-full rounded-lg bg-black/30 px-4 py-2 text-sm outline-none ring-1 ring-white/10 focus:ring-jukebox-accent2"
        />
      </div>

      <div className="space-y-4">
        <NowPlaying />
        <QueueList onError={setToast} />
        <Library onError={setToast} />
      </div>

      {toast && (
        <div className="fixed bottom-4 left-1/2 z-40 -translate-x-1/2 rounded-lg bg-red-500/90 px-4 py-2 text-sm font-medium text-white shadow-lg">
          {toast}
        </div>
      )}

      <AdminLogin open={loginOpen} onClose={() => setLoginOpen(false)} />
    </div>
  )
}
