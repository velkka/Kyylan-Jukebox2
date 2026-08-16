import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  ReactNode
} from 'react'
import type { QueueState, RealtimeMessage } from '@shared/types'
import * as api from './api'

interface Progress {
  position: number
  duration: number
  playing: boolean
  trackId: number | null
  at: number
}

interface JukeboxValue {
  connected: boolean
  queue: QueueState | null
  progress: Progress
  isAdmin: boolean
  name: string
  setName: (name: string) => void
  add: (trackId: number) => Promise<void>
  remove: (entryId: number) => Promise<void>
  downvote: () => Promise<void>
  login: (password: string) => Promise<void>
  logout: () => Promise<void>
  refresh: () => void
}

const JukeboxContext = createContext<JukeboxValue | null>(null)

const EMPTY_PROGRESS: Progress = {
  position: 0,
  duration: 0,
  playing: false,
  trackId: null,
  at: Date.now()
}

export function JukeboxProvider({ children }: { children: ReactNode }): JSX.Element {
  const [connected, setConnected] = useState(false)
  const [queue, setQueue] = useState<QueueState | null>(null)
  const [progress, setProgress] = useState<Progress>(EMPTY_PROGRESS)
  const [isAdmin, setIsAdmin] = useState(false)
  const [name, setNameState] = useState<string>(() => localStorage.getItem('kj_name') ?? '')
  const wsRef = useRef<WebSocket | null>(null)

  const setName = useCallback((next: string) => {
    setNameState(next)
    localStorage.setItem('kj_name', next)
  }, [])

  // Auth status on mount.
  useEffect(() => {
    api.getAuth().then((a) => setIsAdmin(a.isAdmin)).catch(() => setIsAdmin(false))
  }, [])

  // WebSocket with auto-reconnect.
  useEffect(() => {
    let closed = false
    let retry: ReturnType<typeof setTimeout>

    const connect = (): void => {
      const proto = location.protocol === 'https:' ? 'wss' : 'ws'
      const ws = new WebSocket(`${proto}://${location.host}/ws`)
      wsRef.current = ws
      ws.onopen = () => setConnected(true)
      ws.onclose = () => {
        setConnected(false)
        if (!closed) retry = setTimeout(connect, 1000)
      }
      ws.onerror = () => ws.close()
      ws.onmessage = (ev) => {
        const msg = JSON.parse(ev.data as string) as RealtimeMessage
        if (msg.type === 'queue') {
          setQueue(msg.payload)
          setProgress((p) => ({
            ...p,
            trackId: msg.payload.nowPlaying.entry?.track.id ?? null,
            duration: msg.payload.nowPlaying.duration || p.duration,
            playing: msg.payload.nowPlaying.playing
          }))
        } else if (msg.type === 'progress') {
          setProgress({ ...msg.payload, at: Date.now() })
        }
      }
    }

    connect()
    return () => {
      closed = true
      clearTimeout(retry)
      wsRef.current?.close()
    }
  }, [])

  const refresh = useCallback(() => {
    api.getQueue().then(setQueue).catch(() => undefined)
  }, [])

  const add = useCallback(
    async (trackId: number) => {
      const next = await api.enqueue(trackId, name || undefined)
      setQueue(next)
    },
    [name]
  )

  const remove = useCallback(async (entryId: number) => {
    const next = await api.removeEntry(entryId)
    setQueue(next)
  }, [])

  const downvote = useCallback(async () => {
    const next = await api.downvote()
    setQueue(next)
  }, [])

  const login = useCallback(async (password: string) => {
    const a = await api.login(password)
    setIsAdmin(a.isAdmin)
  }, [])

  const logout = useCallback(async () => {
    await api.logout()
    setIsAdmin(false)
  }, [])

  const value: JukeboxValue = {
    connected,
    queue,
    progress,
    isAdmin,
    name,
    setName,
    add,
    remove,
    downvote,
    login,
    logout,
    refresh
  }

  return <JukeboxContext.Provider value={value}>{children}</JukeboxContext.Provider>
}

export function useJukebox(): JukeboxValue {
  const ctx = useContext(JukeboxContext)
  if (!ctx) throw new Error('useJukebox must be used within JukeboxProvider')
  return ctx
}

/** Interpolated playback position in seconds (ticks locally between updates). */
export function useLivePosition(progress: Progress): number {
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!progress.playing) return
    const id = setInterval(() => setTick((t) => t + 1), 400)
    return () => clearInterval(id)
  }, [progress.playing])
  if (!progress.playing) return progress.position
  const elapsed = (Date.now() - progress.at) / 1000
  return Math.min(progress.position + elapsed, progress.duration || Infinity)
}
