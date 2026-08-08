import express from 'express'
import { createServer, Server } from 'node:http'
import { join } from 'node:path'
import { existsSync } from 'node:fs'
import { loadConfig } from './config'
import { lanAddresses } from './net'
import { createApiRouter } from './api'

export interface RunningServer {
  http: Server
  port: number
}

/** Absolute path to the built renderer root (web/ + player/ + assets/). */
function rendererDir(): string {
  // out/main/index.js  ->  out/renderer
  return join(__dirname, '../renderer')
}

export function startServer(): Promise<RunningServer> {
  const config = loadConfig()
  const expressApp = express()

  // The router reads the *actual* bound port so setup can tell the admin whether
  // a port change needs a restart.
  let boundPort = config.port
  expressApp.use('/api', createApiRouter(() => boundPort))

  // In production the built UI is served to every LAN client from here. The
  // renderer root holds web/ (guest UI), player/ (hidden audio window) and the
  // shared assets/ dir referenced by absolute /assets/ URLs.
  // In dev, the electron-vite renderer dev server handles the UI, so there may
  // be nothing on disk yet — that's expected.
  const dir = rendererDir()
  if (existsSync(join(dir, 'web', 'index.html'))) {
    expressApp.use(express.static(dir))
    expressApp.get('/', (_req, res) => res.sendFile(join(dir, 'web', 'index.html')))
  } else {
    expressApp.get('/', (_req, res) =>
      res.status(200).send('Kyylan Jukebox server is running (dev mode: open the app window).')
    )
  }

  const http = createServer(expressApp)

  return new Promise((resolve, reject) => {
    http.once('error', (err) => reject(err))
    http.listen(config.port, '0.0.0.0', () => {
      const addr = http.address()
      boundPort = typeof addr === 'object' && addr ? addr.port : config.port
      console.log(`[server] listening on 0.0.0.0:${boundPort}`)
      for (const ip of lanAddresses()) console.log(`[server] guests: http://${ip}:${boundPort}`)
      resolve({ http, port: boundPort })
    })
  })
}
