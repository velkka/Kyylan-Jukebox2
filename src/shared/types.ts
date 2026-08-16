// Types shared between the Electron main process and the renderers.

export interface AppConfig {
  /** Whether first-run setup has been completed (admin password chosen). */
  configured: boolean
  /** TCP port the embedded HTTP/WebSocket server listens on (0.0.0.0). */
  port: number
  /** Plaintext admin password (LAN party convenience — see project notes). */
  adminPassword: string
  /** Folders scanned to build the music library. */
  libraryPaths: string[]
  /**
   * Max queued (not-yet-played) songs one guest IP may hold. 0 = no limit;
   * negative applies |value| as the limit but hides the counter in the UI.
   */
  perUserQueueLimit: number
  /** Output device id (MediaDeviceInfo.deviceId) the player renderer should use. */
  outputDeviceId: string | null
  /** Play the standby (filler) playlist when the guest queue is empty. */
  standbyEnabled: boolean
  /** Pick standby tracks at random instead of in order. */
  standbyShuffle: boolean
  /** Downvotes needed to auto-skip the current song. 0 disables downvoting. */
  downvoteSkipThreshold: number
}

export const DEFAULT_CONFIG: AppConfig = {
  configured: false,
  port: 8080,
  adminPassword: '',
  libraryPaths: [],
  perUserQueueLimit: 3,
  outputDeviceId: null,
  standbyEnabled: false,
  standbyShuffle: false,
  downvoteSkipThreshold: 0
}

/** Non-sensitive settings safe to expose to any guest. Never includes the password. */
export interface PublicConfig {
  configured: boolean
  port: number
  perUserQueueLimit: number
  name: string
  version: string
}

export interface SetupRequest {
  adminPassword: string
  port?: number
}

export interface LoginRequest {
  password: string
}

export interface AuthStatus {
  isAdmin: boolean
  /**
   * True when the request came from the host machine itself (loopback), which
   * gates host-only affordances like the native folder picker.
   */
  isLocal: boolean
}

export interface BrowseFolderResponse {
  canceled: boolean
  path?: string
}

export interface AdminSettings {
  port: number
  perUserQueueLimit: number
  downvoteSkipThreshold: number
}

export interface AdminSettingsUpdate {
  port?: number
  perUserQueueLimit?: number
  downvoteSkipThreshold?: number
  adminPassword?: string
}

export interface SetupResponse {
  ok: boolean
  /** True when the chosen port differs from the running one (restart needed). */
  restartRequired: boolean
  port: number
}

/** A library track as exposed to clients (no filesystem path). */
export interface Track {
  id: number
  title: string
  artist: string | null
  album: string | null
  albumArtist: string | null
  genre: string | null
  /** Duration in seconds. */
  duration: number | null
  trackNo: number | null
  discNo: number | null
  year: number | null
  /** Cover-art hash; when set, art is at /api/art/<artHash>. */
  artHash: string | null
}

export interface TracksQuery {
  search?: string
  /** Filter to a single artist (matches artist or album artist). */
  artist?: string
  /** Filter to a single album (use with albumArtist to disambiguate). */
  album?: string
  albumArtist?: string
  /** With `artist`, return only that artist's tracks that have no album. */
  noAlbum?: boolean
  limit?: number
  offset?: number
}

export interface TracksResponse {
  tracks: Track[]
  total: number
  limit: number
  offset: number
}

export interface ArtistSummary {
  artist: string
  trackCount: number
  albumCount: number
}

export interface AlbumSummary {
  album: string
  artist: string
  artHash: string | null
  trackCount: number
  year: number | null
}

export interface ArtistsResponse {
  artists: ArtistSummary[]
  total: number
}

export interface AlbumsResponse {
  albums: AlbumSummary[]
  total: number
}

export interface ScanStatus {
  scanning: boolean
  /** Files processed so far in the current/last scan. */
  processed: number
  added: number
  updated: number
  removed: number
  total: number
  startedAt: string | null
  finishedAt: string | null
  error: string | null
}

export interface LibraryPath {
  path: string
  /** Number of indexed tracks located under this folder. */
  trackCount: number
}

export interface LibraryPathsResponse {
  paths: LibraryPath[]
  /** Total tracks in the library (across all folders). */
  total: number
}

// ---- Player ------------------------------------------------------------------

export interface AudioDevice {
  deviceId: string
  label: string
}

export interface PlaybackState {
  trackId: number | null
  playing: boolean
  /** Current position in seconds. */
  position: number
  /** Track duration in seconds (0 until known). */
  duration: number
  volume: number
}

/** Command sent from main → hidden player renderer. */
export type PlayerCommand =
  | { type: 'load'; trackId: number; autoplay: boolean }
  | { type: 'play' }
  | { type: 'pause' }
  | { type: 'seek'; position: number }
  | { type: 'volume'; value: number }
  | { type: 'setSinkId'; deviceId: string }
  | { type: 'enumerate' }

// ---- Queue -------------------------------------------------------------------

export interface QueueEntry {
  id: number
  track: Track
  addedByName: string | null
  /** True when this entry was added by the requesting client (IP match). */
  mine: boolean
}

export interface NowPlaying {
  entry: QueueEntry | null
  position: number
  duration: number
  playing: boolean
  /** True when the current track comes from the standby playlist, not a guest. */
  isStandby: boolean
  /** Downvotes for the current song (0 when the count is hidden). */
  downvotes: number
  /**
   * Downvote setting: 0 = disabled, positive = votes-to-skip with the count
   * shown, negative = same magnitude threshold but the count is hidden in the UI.
   */
  downvoteThreshold: number
  /** Whether the requesting client has already downvoted this song. */
  downvotedByMe: boolean
}

export interface StandbyEntry {
  id: number
  track: Track
}

export interface StandbyState {
  enabled: boolean
  shuffle: boolean
  entries: StandbyEntry[]
}

export interface QueueState {
  nowPlaying: NowPlaying
  queue: QueueEntry[]
  /**
   * Per-guest queue setting: 0 = no limit, positive = max queued songs with the
   * counter shown, negative = same |N| limit but the counter is hidden in the UI.
   */
  perUserLimit: number
  /** How many pending songs the requesting client currently has queued. */
  myQueueCount: number
}

export interface EnqueueRequest {
  trackId: number
  name?: string
}

/** Real-time messages pushed server → client over the WebSocket. */
export type RealtimeMessage =
  | { type: 'queue'; payload: QueueState }
  | { type: 'progress'; payload: { position: number; duration: number; playing: boolean; trackId: number | null } }

export interface HealthResponse {
  name: string
  version: string
  /** LAN URLs guests can open to reach the jukebox. */
  addresses: string[]
  port: number
}
