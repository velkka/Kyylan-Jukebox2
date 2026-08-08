#!/usr/bin/env node
/*
 * macOS Gatekeeper/XProtect blocks the freshly downloaded, unsigned dev Electron
 * runtime ("... is malware and will damage your computer"). Clearing the quarantine
 * xattrs and applying a deep ad-hoc signature lets the local dev binary run.
 * No-op on Windows/Linux and in CI where the app isn't present.
 */
const { execFileSync } = require('node:child_process')
const { existsSync } = require('node:fs')
const { join } = require('node:path')

if (process.platform !== 'darwin') process.exit(0)

const appPath = join(__dirname, '..', 'node_modules', 'electron', 'dist', 'Electron.app')
if (!existsSync(appPath)) {
  console.log('[fix-electron-macos] Electron.app not found yet, skipping')
  process.exit(0)
}

try {
  execFileSync('xattr', ['-cr', appPath], { stdio: 'ignore' })
  execFileSync('codesign', ['--force', '--deep', '--sign', '-', appPath], { stdio: 'ignore' })
  console.log('[fix-electron-macos] cleared quarantine and ad-hoc signed Electron.app')
} catch (err) {
  console.warn('[fix-electron-macos] could not sign Electron.app:', err.message)
  console.warn('[fix-electron-macos] if the app is blocked, run manually:')
  console.warn(`  xattr -cr "${appPath}"`)
  console.warn(`  codesign --force --deep --sign - "${appPath}"`)
}
