import { useSyncExternalStore } from 'react'

/**
 * Pre-listen: streams a track to the guest's own device through their browser,
 * never to the party's audio output. It does not touch the queue, so a preview
 * is not a play and is deliberately absent from the history and stats.
 *
 * One shared audio element lives outside React so a preview keeps going as rows
 * unmount (switching artist, searching), and so starting one always stops the
 * previous — nobody wants two songs at once out of one phone.
 */

let audio: HTMLAudioElement | null = null
let currentId: number | null = null
const listeners = new Set<() => void>()

const emit = (): void => listeners.forEach((l) => l())

function element(): HTMLAudioElement {
  if (audio) return audio
  const el = new Audio()
  el.preload = 'none'
  el.addEventListener('ended', () => {
    currentId = null
    emit()
  })
  return (audio = el)
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

const snapshot = (): number | null => currentId

/** The track being pre-listened to, or null. */
export function usePreviewing(): number | null {
  return useSyncExternalStore(subscribe, snapshot, snapshot)
}

export function stopPreview(): void {
  if (currentId == null) return
  const el = element()
  el.pause()
  // Drop the source too, so the browser stops buffering the rest of the file.
  el.removeAttribute('src')
  el.load()
  currentId = null
  emit()
}

/** Starts previewing a track, or stops it if it is already the one playing. */
export function togglePreview(trackId: number, onError?: (msg: string) => void): void {
  if (currentId === trackId) {
    stopPreview()
    return
  }
  const el = element()
  el.src = `/api/stream/${trackId}`
  currentId = trackId
  emit()
  // The click is the user gesture browsers require, so this normally resolves;
  // a rejection means an unsupported codec or the file having gone missing.
  el.play().catch(() => {
    if (currentId !== trackId) return // superseded by another preview
    currentId = null
    emit()
    onError?.('Could not play a preview of that song.')
  })
}
