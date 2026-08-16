import { BrowserWindow } from 'electron'

// The host console window, tracked so native dialogs (e.g. the folder picker)
// can be shown as a sheet attached to it.
let win: BrowserWindow | null = null

export function setHostWindow(w: BrowserWindow | null): void {
  win = w
}

export function getHostWindow(): BrowserWindow | null {
  return win && !win.isDestroyed() ? win : null
}
