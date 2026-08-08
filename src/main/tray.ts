import { app, Tray, Menu, nativeImage, clipboard } from 'electron'
import { join } from 'node:path'
import { lanAddresses } from './net'
import { loadConfig } from './config'

let tray: Tray | null = null

function iconPath(): string {
  return app.isPackaged
    ? join(process.resourcesPath, 'tray.png')
    : join(app.getAppPath(), 'build', 'tray.png')
}

export function initTray(onOpen: () => void): void {
  const img = nativeImage.createFromPath(iconPath())
  // On macOS the tray art is a template: the OS tints it for light/dark menubars.
  if (process.platform === 'darwin') img.setTemplateImage(true)
  tray = new Tray(img.isEmpty() ? nativeImage.createEmpty() : img)
  tray.setToolTip('Kyylan Jukebox')
  tray.on('click', onOpen)
  rebuildTrayMenu(onOpen)
}

/** Rebuilds the menu (call after the port or network addresses change). */
export function rebuildTrayMenu(onOpen: () => void): void {
  if (!tray) return
  const port = loadConfig().port
  const urls = lanAddresses().map((ip) => `http://${ip}:${port}`)
  const urlItems = urls.length
    ? urls.map((u) => ({ label: `Copy guest link — ${u}`, click: () => clipboard.writeText(u) }))
    : [{ label: 'No network address found', enabled: false }]

  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: 'Kyylan Jukebox', enabled: false },
      { type: 'separator' },
      { label: 'Open jukebox console', click: onOpen },
      { type: 'separator' },
      { label: 'Guests can join at', enabled: false },
      ...urlItems,
      { type: 'separator' },
      { label: 'Quit Kyylan Jukebox', click: () => app.quit() }
    ])
  )
}

export function destroyTray(): void {
  tray?.destroy()
  tray = null
}
