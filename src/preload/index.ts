import { contextBridge, ipcRenderer } from 'electron'
import type { PlayerCommand } from '@shared/types'

// Bridge exposed to the hidden player renderer (context isolation on).
// Main sends PlayerCommands; the renderer reports playback events back.
const api = {
  onCommand: (handler: (cmd: PlayerCommand) => void): void => {
    ipcRenderer.on('player:command', (_e, cmd: PlayerCommand) => handler(cmd))
  },
  report: (event: string, payload: unknown): void => {
    ipcRenderer.send('player:event', event, payload)
  }
}

contextBridge.exposeInMainWorld('jukeboxPlayer', api)

export type JukeboxPlayerApi = typeof api
