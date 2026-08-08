import { app, BrowserWindow, dialog, session } from 'electron'
import { join } from 'node:path'
import { WebSocketServer } from 'ws'
import { startServer, RunningServer } from './server'
import { loadConfig } from './config'
import { getDb, closeDb } from './db'
import { initPlayerIpc, onReady, onStateChange, onTrackEnded, setPlayerWindow } from './player'
import { advance, buildQueueState, initQueue, maybeStart } from './queue'
import { initRealtime, pushProgress } from './realtime'
import { destroyTray, initTray } from './tray'

// Let the hidden player window start audio without a user gesture.
app.commandLine.appendSwitch('autoplay-policy', 'no-user-gesture-required')

/** App/window icon (packaged mac builds use the bundle .icns instead). */
function appIconPath(): string {
  return app.isPackaged
    ? join(process.resourcesPath, 'icon.png')
    : join(app.getAppPath(), 'build', 'icon.png')
}

let hostWindow: BrowserWindow | null = null
let playerWindow: BrowserWindow | null = null
let server: RunningServer | null = null

const isDev = !!process.env['ELECTRON_RENDERER_URL']
const rendererUrl = process.env['ELECTRON_RENDERER_URL'] ?? ''

/** The admin/host console window (loads the same web UI guests see). */
function createHostWindow(port: number): void {
  hostWindow = new BrowserWindow({
    width: 1180,
    height: 820,
    minWidth: 900,
    minHeight: 600,
    backgroundColor: '#0e0b14',
    title: 'Kyylan Jukebox',
    icon: appIconPath(), // used on Windows/Linux; macOS uses the bundle icon
    autoHideMenuBar: true,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false
    }
  })

  if (isDev) {
    hostWindow.loadURL(`${rendererUrl}/web/index.html`)
  } else {
    // Load over HTTP so the host console behaves exactly like a LAN guest
    // (same origin for API, cookies and websockets).
    hostWindow.loadURL(`http://localhost:${port}/`)
  }

  hostWindow.on('closed', () => {
    hostWindow = null
  })
}

/** Shows the console window, recreating it if it was closed (called from tray). */
function openConsole(): void {
  if (hostWindow) {
    if (hostWindow.isMinimized()) hostWindow.restore()
    hostWindow.show()
    hostWindow.focus()
  } else if (server) {
    createHostWindow(server.port)
  }
}

/** Hidden window that actually decodes audio and drives the output device. */
function createPlayerWindow(): void {
  playerWindow = new BrowserWindow({
    show: false,
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      // Keep timers/audio running full-speed while the window is hidden.
      backgroundThrottling: false
    }
  })

  setPlayerWindow(playerWindow)

  if (isDev) {
    playerWindow.loadURL(`${rendererUrl}/player/index.html`)
  } else {
    // Load over HTTP (same origin as the API it streams from), so the player's
    // absolute /assets/ URLs resolve.
    playerWindow.loadURL(`http://localhost:${server?.port ?? loadConfig().port}/player/index.html`)
  }

  playerWindow.on('closed', () => {
    playerWindow = null
  })
}

async function bootstrap(): Promise<void> {
  const config = loadConfig()
  getDb() // open + migrate the database before serving
  initQueue() // clear stale "playing" state from a previous run

  // Show the eyes on the macOS dock during development (packaged apps use .icns).
  if (!app.isPackaged && process.platform === 'darwin' && app.dock) {
    app.dock.setIcon(appIconPath())
  }

  // Grant media permission so the player renderer can read audio-output labels.
  session.defaultSession.setPermissionRequestHandler((_wc, _permission, cb) => cb(true))
  initPlayerIpc()

  // Queue drives the player; the player's events drive the queue + live updates.
  onTrackEnded(() => advance())
  onReady(() => maybeStart())
  onStateChange((state) => pushProgress(state))

  try {
    server = await startServer()
  } catch (err: unknown) {
    const message =
      err instanceof Error && 'code' in err && (err as NodeJS.ErrnoException).code === 'EADDRINUSE'
        ? `Port ${config.port} is already in use. Change the port in the app settings and restart.`
        : `Failed to start the jukebox server: ${String(err)}`
    dialog.showErrorBox('Kyylan Jukebox', message)
    app.quit()
    return
  }

  // WebSocket for live queue/progress updates to every connected browser.
  const wss = new WebSocketServer({ server: server.http, path: '/ws' })
  initRealtime(wss, buildQueueState)

  createPlayerWindow()
  createHostWindow(server.port)
  initTray(openConsole) // keeps the jukebox running when the console is closed
}

app.whenReady().then(bootstrap)

app.on('activate', () => {
  // On macOS, re-open the console from the dock/tray.
  openConsole()
})

// The hidden player window keeps the app alive while the console is closed, so
// the jukebox keeps playing. Quit explicitly from the tray.
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})

app.on('before-quit', () => {
  destroyTray()
  server?.http.close()
  closeDb()
})
