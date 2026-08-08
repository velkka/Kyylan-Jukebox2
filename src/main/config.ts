import { app } from 'electron'
import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import { AppConfig, DEFAULT_CONFIG } from '@shared/types'

// The config lives as plaintext JSON in the OS-standard userData dir.
// (Admin password is intentionally stored in the clear — LAN party convenience.)
const configPath = (): string => join(app.getPath('userData'), 'config.json')

let cached: AppConfig | null = null

export function loadConfig(): AppConfig {
  if (cached) return cached
  const path = configPath()
  if (existsSync(path)) {
    let next: AppConfig
    try {
      const raw = JSON.parse(readFileSync(path, 'utf-8'))
      // Merge over defaults so new fields get sane values on upgrade.
      next = { ...DEFAULT_CONFIG, ...raw }
    } catch (err) {
      console.error('[config] failed to parse config.json, using defaults:', err)
      next = { ...DEFAULT_CONFIG }
    }
    cached = next
    return next
  }
  return saveConfig({ ...DEFAULT_CONFIG })
}

export function saveConfig(next: AppConfig): AppConfig {
  cached = next
  writeFileSync(configPath(), JSON.stringify(next, null, 2), 'utf-8')
  return cached
}

export function updateConfig(patch: Partial<AppConfig>): AppConfig {
  return saveConfig({ ...loadConfig(), ...patch })
}

export function getConfigPath(): string {
  return configPath()
}
