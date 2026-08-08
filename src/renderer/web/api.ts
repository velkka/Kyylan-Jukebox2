import type {
  AdminSettings,
  AdminSettingsUpdate,
  AlbumsResponse,
  ArtistsResponse,
  AudioDevice,
  AuthStatus,
  HealthResponse,
  LibraryPathsResponse,
  PublicConfig,
  QueueState,
  ScanStatus,
  SetupRequest,
  SetupResponse,
  StandbyState,
  TracksResponse
} from '@shared/types'

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    ...init
  })
  if (!res.ok) {
    let message = `HTTP ${res.status}`
    try {
      const body = (await res.json()) as { error?: string }
      if (body?.error) message = body.error
    } catch {
      /* non-JSON error body */
    }
    throw new Error(message)
  }
  return (await res.json()) as T
}

// ---- Public ------------------------------------------------------------------
export const getConfig = (): Promise<PublicConfig> => jsonFetch('/api/config')
export const getHealth = (): Promise<HealthResponse> => jsonFetch('/api/health')
export const runSetup = (body: SetupRequest): Promise<SetupResponse> =>
  jsonFetch('/api/setup', { method: 'POST', body: JSON.stringify(body) })

// ---- Auth --------------------------------------------------------------------
export const getAuth = (): Promise<AuthStatus> => jsonFetch('/api/auth')
export const login = (password: string): Promise<AuthStatus> =>
  jsonFetch('/api/login', { method: 'POST', body: JSON.stringify({ password }) })
export const logout = (): Promise<AuthStatus> => jsonFetch('/api/logout', { method: 'POST' })

// ---- Library -----------------------------------------------------------------
export const getTracks = (params: {
  search?: string
  artist?: string
  album?: string
  albumArtist?: string
  limit?: number
  offset?: number
}): Promise<TracksResponse> => {
  const q = new URLSearchParams()
  if (params.search) q.set('search', params.search)
  if (params.artist) q.set('artist', params.artist)
  if (params.album) q.set('album', params.album)
  if (params.albumArtist) q.set('albumArtist', params.albumArtist)
  if (params.limit != null) q.set('limit', String(params.limit))
  if (params.offset != null) q.set('offset', String(params.offset))
  return jsonFetch(`/api/tracks?${q.toString()}`)
}
export const getArtists = (params: {
  search?: string
  limit?: number
  offset?: number
}): Promise<ArtistsResponse> => {
  const q = new URLSearchParams()
  if (params.search) q.set('search', params.search)
  if (params.limit != null) q.set('limit', String(params.limit))
  if (params.offset != null) q.set('offset', String(params.offset))
  return jsonFetch(`/api/artists?${q.toString()}`)
}
export const getAlbums = (params: {
  search?: string
  artist?: string
  limit?: number
  offset?: number
}): Promise<AlbumsResponse> => {
  const q = new URLSearchParams()
  if (params.search) q.set('search', params.search)
  if (params.artist) q.set('artist', params.artist)
  if (params.limit != null) q.set('limit', String(params.limit))
  if (params.offset != null) q.set('offset', String(params.offset))
  return jsonFetch(`/api/albums?${q.toString()}`)
}
export const artUrl = (hash: string): string => `/api/art/${hash}`

// ---- Queue -------------------------------------------------------------------
export const getQueue = (): Promise<QueueState> => jsonFetch('/api/queue')
export const enqueue = (trackId: number, name?: string): Promise<QueueState> =>
  jsonFetch('/api/queue', { method: 'POST', body: JSON.stringify({ trackId, name }) })
export const removeEntry = (id: number): Promise<QueueState> =>
  jsonFetch(`/api/queue/${id}`, { method: 'DELETE' })

// ---- Admin: queue ------------------------------------------------------------
export const moveEntry = (id: number, toIndex: number): Promise<QueueState> =>
  jsonFetch(`/api/queue/${id}/move`, { method: 'POST', body: JSON.stringify({ toIndex }) })
export const skipCurrent = (): Promise<QueueState> =>
  jsonFetch('/api/queue/skip', { method: 'POST' })
export const clearQueue = (): Promise<QueueState> =>
  jsonFetch('/api/queue/clear', { method: 'POST' })

// ---- Admin: player -----------------------------------------------------------
export const getDevices = (): Promise<{ devices: AudioDevice[]; selected: string | null }> =>
  jsonFetch('/api/player/devices')
export const setOutput = (deviceId: string): Promise<{ selected: string | null }> =>
  jsonFetch('/api/player/output', { method: 'POST', body: JSON.stringify({ deviceId }) })
export const playerPlay = (): Promise<unknown> => jsonFetch('/api/player/play', { method: 'POST' })
export const playerPause = (): Promise<unknown> =>
  jsonFetch('/api/player/pause', { method: 'POST' })

// ---- Admin: library ----------------------------------------------------------
export const getPaths = (): Promise<LibraryPathsResponse> => jsonFetch('/api/library/paths')
export const addLibraryPath = (path: string): Promise<LibraryPathsResponse> =>
  jsonFetch('/api/library/paths', { method: 'POST', body: JSON.stringify({ path }) })
export const removeLibraryPath = (path: string): Promise<LibraryPathsResponse> =>
  jsonFetch('/api/library/paths', { method: 'DELETE', body: JSON.stringify({ path }) })
export const startScan = (): Promise<ScanStatus> =>
  jsonFetch('/api/library/scan', { method: 'POST' })
export const getScanStatus = (): Promise<ScanStatus> => jsonFetch('/api/library/scan/status')

// ---- Admin: standby playlist -------------------------------------------------
export const getStandby = (): Promise<StandbyState> => jsonFetch('/api/standby')
export const addStandby = (trackId: number): Promise<StandbyState> =>
  jsonFetch('/api/standby', { method: 'POST', body: JSON.stringify({ trackId }) })
export const removeStandby = (id: number): Promise<StandbyState> =>
  jsonFetch(`/api/standby/${id}`, { method: 'DELETE' })
export const clearStandby = (): Promise<StandbyState> =>
  jsonFetch('/api/standby/clear', { method: 'POST' })
export const setStandbySettings = (patch: {
  enabled?: boolean
  shuffle?: boolean
}): Promise<StandbyState> =>
  jsonFetch('/api/standby/settings', { method: 'POST', body: JSON.stringify(patch) })

// ---- Admin: settings ---------------------------------------------------------
export const getSettings = (): Promise<AdminSettings> => jsonFetch('/api/admin/settings')
export const saveSettings = (
  patch: AdminSettingsUpdate
): Promise<{ restartRequired: boolean }> =>
  jsonFetch('/api/admin/settings', { method: 'POST', body: JSON.stringify(patch) })
