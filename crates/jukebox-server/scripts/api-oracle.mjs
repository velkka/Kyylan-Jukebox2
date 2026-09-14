// Records how Electron's HTTP API answers tests/fixtures/api-script.json, as the expected
// responses for the Rust server's parity harness (tests/api_electron.rs).
//
// This runs Electron's real server — server.ts, api.ts, auth.ts, realtime.ts, queue.ts and
// everything they use — bundled with esbuild and started inside Electron's Node, so Express,
// ws, better-sqlite3 and music-metadata are exactly the Electron build's. Only what needs a
// window or the network is stubbed, identically on the Rust side:
//
//   - `electron`: the data directory, the version, and a folder picker that is cancelled.
//   - net.ts's LAN addresses and hostname lookups, which differ per machine.
//   - the player window: player.ts runs unchanged, but no window reports playback events.
//
// Each request's client address is set from the script, so four clients can be simulated
// from one machine. Run from the repository root, after `npm ci`:
//
//   node crates/jukebox-server/scripts/api-oracle.mjs
//
// Port 18094 must be free.

import { spawnSync } from 'node:child_process'
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const REPO = resolve(fileURLToPath(import.meta.url), '../../../..')
const TESTS = join(REPO, 'crates/jukebox-server/tests')
const scratch = mkdtempSync(join(tmpdir(), 'kyylan-api-oracle-'))

const netStub = `
export { normalizeIp } from ${JSON.stringify(join(REPO, 'src/main/net.ts'))}
export { hostnameFor as resolveHostname, lanAddresses } from ${JSON.stringify(join(TESTS, 'fixtures/api-net-stub.mjs'))}
`

const electronStub = `
const { EventEmitter } = require('node:events')
module.exports = {
  app: {
    getPath: () => process.env.ORACLE_DATA,
    getVersion: () => require(${JSON.stringify(join(REPO, 'package.json'))}).version
  },
  dialog: { showOpenDialog: async () => ({ canceled: true, filePaths: [] }) },
  BrowserWindow: class {},
  ipcMain: new EventEmitter()
}
`

const entry = String.raw`
import http from 'node:http'
import { createHash } from 'node:crypto'
import { readFileSync, writeFileSync } from 'node:fs'
import { WebSocket, WebSocketServer } from 'ws'
import { startServer } from './src/main/server'
import { getDb } from './src/main/db'
import { initQueue, advance, buildQueueState } from './src/main/queue'
import { onStateChange } from './src/main/player'
import { initRealtime, pushProgress } from './src/main/realtime'
import { scanStatus } from './src/main/library'
import { normalize } from ${JSON.stringify(join(TESTS, 'fixtures/api-normalize.mjs'))}

const script = JSON.parse(readFileSync(process.env.ORACLE_SCRIPT, 'utf-8'))
const ROOT = process.env.ORACLE_ROOT
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function main() {
  // index.ts's bootstrap, minus the windows and the tray.
  getDb()
  initQueue()
  onStateChange((state) => pushProgress(state))
  const server = await startServer()
  const wss = new WebSocketServer({ server: server.http, path: '/ws' })
  initRealtime(wss, buildQueueState)

  // Every request says which simulated client sent it.
  const setClient = (req) => {
    const ip = req.headers['x-client-ip']
    Object.defineProperty(req.socket, 'remoteAddress', { value: ip, configurable: true })
  }
  server.http.prependListener('request', setClient)
  server.http.prependListener('upgrade', setClient)

  const fill = (value) =>
    typeof value === 'string'
      ? value.split('{root}').join(ROOT).split('{port}').join(String(script.port))
      : Array.isArray(value)
        ? value.map(fill)
        : value && typeof value === 'object'
          ? Object.fromEntries(Object.entries(value).map(([k, v]) => [k, fill(v)]))
          : value

  const send = (step, cookie) =>
    new Promise((resolve, reject) => {
      const headers = { 'x-client-ip': script.clients[step.ip], connection: 'close', ...(step.headers ?? {}) }
      let body
      if (step.json !== undefined) {
        body = JSON.stringify(fill(step.json))
        headers['content-type'] = 'application/json'
      } else if (step.text !== undefined) {
        body = step.text
        headers['content-type'] = step.contentType
      }
      if (body !== undefined) headers['content-length'] = Buffer.byteLength(body)
      if (step.admin && cookie) headers.cookie = cookie
      const req = http.request(
        { host: '127.0.0.1', port: server.port, method: step.method, path: fill(step.path), headers, agent: false },
        (res) => {
          const chunks = []
          res.on('data', (c) => chunks.push(c))
          res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }))
        }
      )
      req.on('error', reject)
      req.end(body)
    })

  let cookie = null
  const sockets = {}
  const messages = {}
  const responses = []

  for (const step of script.steps) {
    switch (step.do) {
      case 'ws-open': {
        const ws = new WebSocket('ws://127.0.0.1:' + server.port + '/ws', {
          headers: { 'x-client-ip': script.clients[step.client] }
        })
        messages[step.client] = []
        ws.on('message', (data) => messages[step.client].push(JSON.parse(data.toString())))
        await new Promise((r) => ws.once('open', r))
        sockets[step.client] = ws
        await sleep(50)
        continue
      }
      case 'wait-scan':
        while (scanStatus().scanning) await sleep(20)
        continue
      case 'track-ended':
        // What player.ts does when the player window reports the song ended.
        advance()
        continue
      case 'ws-close':
        await sleep(200)
        for (const ws of Object.values(sockets)) ws.close()
        continue
    }

    const res = await send(step, cookie)
    const setCookie = res.headers['set-cookie']
    if (step.id === 'login' && setCookie) cookie = setCookie[0].split(';')[0]
    responses.push({ id: step.id, ...describe(res) })
    await sleep(15)
  }

  writeFileSync(process.env.ORACLE_OUT, JSON.stringify(normalize({
    electron: process.versions.electron,
    responses,
    websocket: messages
  }, ROOT), null, 2) + '\n')
  server.http.close()
  process.exit(0)
}

// The parts of a response the harness compares.
const COMPARED_HEADERS = ['content-type', 'content-length', 'content-range', 'accept-ranges', 'cache-control', 'content-disposition', 'set-cookie', 'content-security-policy', 'x-content-type-options']
function describe(res) {
  const headers = {}
  for (const name of COMPARED_HEADERS) if (res.headers[name] !== undefined) headers[name] = res.headers[name]
  const type = res.headers['content-type'] ?? ''
  let body
  if (type.startsWith('application/json')) body = { json: JSON.parse(res.body.toString('utf-8')) }
  else if (type.startsWith('text/csv')) body = { csvLines: res.body.toString('utf-8').split('\r\n').length }
  else if (type.startsWith('text/')) body = { text: res.body.toString('utf-8') }
  else body = { bytes: res.body.length, sha1: createHash('sha1').update(res.body).digest('hex') }
  // A JSON or CSV body's length depends on how its durations are written, which the body
  // comparison already accounts for.
  if (body.json !== undefined || body.csvLines !== undefined) delete headers['content-length']
  return { status: res.status, headers, body }
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
`

try {
  const data = join(scratch, 'data')
  const root = join(scratch, 'music')
  mkdirSync(data)
  cpSync(join(REPO, 'crates/jukebox-core/tests/fixtures/library'), root, { recursive: true })
  const script = JSON.parse((await import('node:fs')).readFileSync(join(TESTS, 'fixtures/api-script.json'), 'utf-8'))
  writeFileSync(join(data, 'config.json'), JSON.stringify({ port: script.port }, null, 2))

  writeFileSync(join(scratch, 'electron-stub.cjs'), electronStub)
  writeFileSync(join(scratch, 'net-stub.ts'), netStub)
  const bundle = join(scratch, 'oracle.cjs')
  await build({
    stdin: { contents: entry, resolveDir: REPO, loader: 'ts', sourcefile: 'oracle.ts' },
    bundle: true,
    platform: 'node',
    format: 'cjs',
    outfile: bundle,
    external: ['better-sqlite3', 'music-metadata', 'express', 'cookie-parser', 'ws'],
    alias: {
      electron: join(scratch, 'electron-stub.cjs'),
      '@shared/types': join(REPO, 'src/shared/types.ts')
    },
    plugins: [
      {
        name: 'net-stub',
        setup(b) {
          b.onResolve({ filter: /^\.\/net$/ }, (args) =>
            args.importer.includes(join('src', 'main')) ? { path: join(scratch, 'net-stub.ts') } : undefined
          )
        }
      }
    ],
    logLevel: 'warning'
  })

  const out = join(TESTS, 'fixtures/api-electron.json')
  const run = spawnSync(join(REPO, 'node_modules/.bin/electron'), [bundle], {
    stdio: 'inherit',
    env: {
      ...process.env,
      ELECTRON_RUN_AS_NODE: '1',
      NODE_PATH: join(REPO, 'node_modules'),
      ORACLE_DATA: data,
      ORACLE_ROOT: root,
      ORACLE_SCRIPT: join(TESTS, 'fixtures/api-script.json'),
      ORACLE_OUT: out
    }
  })
  if (run.status !== 0) process.exit(run.status ?? 1)
  console.log('wrote ' + out)
} finally {
  rmSync(scratch, { recursive: true, force: true })
}
