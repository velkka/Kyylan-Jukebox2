import { WebSocketServer, WebSocket } from 'ws'
import type { IncomingMessage } from 'node:http'
import { normalizeIp } from './net'
import { PlaybackState, QueueState, RealtimeMessage } from '@shared/types'

interface Client {
  ws: WebSocket
  ip: string
}

let clients: Client[] = []
// Builds the per-client queue view (the `mine` flags depend on the client IP).
let buildQueueState: ((ip: string) => QueueState) | null = null

export function initRealtime(
  wss: WebSocketServer,
  build: (ip: string) => QueueState
): void {
  buildQueueState = build
  wss.on('connection', (ws: WebSocket, req: IncomingMessage) => {
    const ip = normalizeIp(req.socket.remoteAddress ?? undefined)
    const client: Client = { ws, ip }
    clients.push(client)
    send(ws, { type: 'queue', payload: build(ip) })
    ws.on('close', () => {
      clients = clients.filter((c) => c !== client)
    })
    ws.on('error', () => {
      /* ignore per-socket errors */
    })
  })
}

function send(ws: WebSocket, msg: RealtimeMessage): void {
  if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg))
}

/** Pushes the (personalized) full queue state to every connected client. */
export function broadcastQueue(): void {
  if (!buildQueueState) return
  for (const c of clients) {
    send(c.ws, { type: 'queue', payload: buildQueueState(c.ip) })
  }
}

// Progress is identical for everyone, so it's broadcast verbatim — but throttled
// to ~1/s for position, and sent immediately when play/pause or the track flips.
let lastProgressAt = 0
let lastPlaying = false
let lastTrackId: number | null = null

export function pushProgress(state: PlaybackState): void {
  const now = Date.now()
  const changed = state.playing !== lastPlaying || state.trackId !== lastTrackId
  if (!changed && now - lastProgressAt < 900) return
  lastProgressAt = now
  lastPlaying = state.playing
  lastTrackId = state.trackId
  const msg: RealtimeMessage = {
    type: 'progress',
    payload: {
      position: state.position,
      duration: state.duration,
      playing: state.playing,
      trackId: state.trackId
    }
  }
  for (const c of clients) send(c.ws, msg)
}
