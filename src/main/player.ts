import { BrowserWindow, ipcMain } from 'electron'
import { loadConfig, updateConfig } from './config'
import { AudioDevice, PlaybackState, PlayerCommand } from '@shared/types'

let win: BrowserWindow | null = null
let devices: AudioDevice[] = []
const state: PlaybackState = {
  trackId: null,
  playing: false,
  position: 0,
  duration: 0,
  volume: 1
}

// Hooks other modules subscribe to (the queue engine wires these in M5).
let endedHandler: (() => void) | null = null
let stateListener: ((s: PlaybackState) => void) | null = null
let readyHandler: (() => void) | null = null

export function setPlayerWindow(w: BrowserWindow): void {
  win = w
}

export function onTrackEnded(fn: () => void): void {
  endedHandler = fn
}

export function onStateChange(fn: (s: PlaybackState) => void): void {
  stateListener = fn
}

export function onReady(fn: () => void): void {
  readyHandler = fn
}

function send(cmd: PlayerCommand): void {
  win?.webContents.send('player:command', cmd)
}

function emitState(): void {
  stateListener?.({ ...state })
}

/** Registers the IPC listener for events reported by the player renderer. */
export function initPlayerIpc(): void {
  ipcMain.on('player:event', (_e, event: string, payload: unknown) => {
    switch (event) {
      case 'ready': {
        // Renderer (re)loaded: refresh devices and apply the saved output.
        send({ type: 'enumerate' })
        const saved = loadConfig().outputDeviceId
        if (saved) send({ type: 'setSinkId', deviceId: saved })
        readyHandler?.()
        break
      }
      case 'devices':
        devices = (payload as AudioDevice[]) ?? []
        break
      case 'timeupdate': {
        const p = payload as Partial<PlaybackState>
        if (typeof p.position === 'number') state.position = p.position
        if (typeof p.duration === 'number') state.duration = p.duration
        if (typeof p.playing === 'boolean') state.playing = p.playing
        if (typeof p.trackId === 'number') state.trackId = p.trackId
        emitState()
        break
      }
      case 'ended':
        state.playing = false
        emitState()
        endedHandler?.()
        break
      case 'error':
        console.error('[player] renderer:', payload)
        break
    }
  })
}

// ---- Transport (used by the queue engine and admin controls) ----------------

export function loadTrack(trackId: number, autoplay = true): void {
  state.trackId = trackId
  state.position = 0
  state.duration = 0
  state.playing = autoplay
  send({ type: 'load', trackId, autoplay })
  emitState()
}

export function play(): void {
  state.playing = true
  send({ type: 'play' })
  emitState()
}

export function pause(): void {
  state.playing = false
  send({ type: 'pause' })
  emitState()
}

export function seek(position: number): void {
  send({ type: 'seek', position })
}

export function setVolume(value: number): void {
  state.volume = Math.min(Math.max(value, 0), 1)
  send({ type: 'volume', value: state.volume })
  emitState()
}

export function setOutputDevice(deviceId: string): void {
  updateConfig({ outputDeviceId: deviceId || null })
  send({ type: 'setSinkId', deviceId })
}

export function requestDevices(): void {
  send({ type: 'enumerate' })
}

export function getDevices(): AudioDevice[] {
  return devices
}

export function getState(): PlaybackState {
  return { ...state }
}
