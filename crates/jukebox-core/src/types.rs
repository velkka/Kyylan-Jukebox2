//! Serde mirrors of src/shared/types.ts — the JSON contract between the server and the web
//! UI. Field names, nullability and omission rules follow the TypeScript exactly:
//!
//! - `x: T | null` → `Option<T>`, always present, `null` when empty
//! - `x?: T`       → `Option<T>`, left out of the JSON when empty
//! - `number` that can be fractional → `f64` written the way JavaScript writes it
//!
//! The Electron-only `PlayerCommand` IPC type has no mirror: nothing in the Rust build
//! talks to a player window.

use serde::{Deserialize, Serialize};

/// JavaScript prints a whole-number float without a decimal point (`215`, not `215.0`), and
/// a non-finite one as `null`. Matching that keeps the Rust server's JSON identical to the
/// Electron server's — which the parity tests compare.
pub(crate) mod js_number {
    use serde::{Deserializer, Serialize, Serializer};

    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;

    fn write<S: Serializer>(v: f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_finite() && v.fract() == 0.0 && v.abs() < MAX_SAFE_INTEGER {
            s.serialize_i64(v as i64)
        } else {
            s.serialize_f64(v)
        }
    }

    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        write(*v, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        serde::Deserialize::deserialize(d)
    }

    pub mod option {
        use super::*;

        pub fn serialize<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(v) => write(*v, s),
                None => None::<()>.serialize(s),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
            serde::Deserialize::deserialize(d)
        }
    }
}

// ---- Setup, auth, settings -----------------------------------------------------

/// Non-sensitive settings any guest may see. Never includes the password.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub configured: bool,
    pub port: u16,
    pub per_user_queue_limit: i32,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    pub admin_password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupResponse {
    pub ok: bool,
    /// True when the chosen port differs from the running one.
    pub restart_required: bool,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub is_admin: bool,
    /// The request came from the host itself, which gates the native folder picker.
    pub is_local: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowseFolderResponse {
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSettings {
    pub port: u16,
    pub per_user_queue_limit: i32,
    pub downvote_skip_threshold: i32,
    pub same_song_cooldown_minutes: u32,
    pub same_artist_cooldown_minutes: u32,
    pub add_rate_limit_minutes: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSettingsUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_user_queue_limit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downvote_skip_threshold: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_song_cooldown_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_artist_cooldown_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_rate_limit_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_password: Option<String>,
}

/// `POST /api/admin/settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsResponse {
    pub restart_required: bool,
}

/// Every error response: `{ "error": "…" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub name: String,
    pub version: String,
    /// LAN addresses guests can open.
    pub addresses: Vec<String>,
    pub port: u16,
}

// ---- Library ---------------------------------------------------------------------

/// A library track as clients see it — no filesystem path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    /// Seconds.
    #[serde(with = "js_number::option")]
    pub duration: Option<f64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    /// When set, the art is at `/api/art/<artHash>`.
    pub art_hash: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TracksQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_album: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TracksResponse {
    pub tracks: Vec<Track>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    pub artist: String,
    pub track_count: i64,
    pub album_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    pub album: String,
    pub artist: String,
    pub art_hash: Option<String>,
    pub track_count: i64,
    pub year: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtistsResponse {
    pub artists: Vec<ArtistSummary>,
    pub total: i64,
    /// Initials present in the library: A–Z, plus `#` for everything else.
    pub letters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumsResponse {
    pub albums: Vec<AlbumSummary>,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub scanning: bool,
    pub processed: i64,
    pub added: i64,
    pub updated: i64,
    pub removed: i64,
    pub total: i64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPath {
    pub path: String,
    pub track_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryPathsResponse {
    pub paths: Vec<LibraryPath>,
    pub total: i64,
}

// ---- Player ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub device_id: String,
    pub label: String,
}

/// `GET /api/player/devices`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicesResponse {
    pub devices: Vec<AudioDevice>,
    pub selected: Option<String>,
}

/// `POST /api/player/output`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputResponse {
    pub ok: bool,
    pub selected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub track_id: Option<i64>,
    pub playing: bool,
    /// Seconds.
    #[serde(with = "js_number")]
    pub position: f64,
    /// Seconds; 0 until known.
    #[serde(with = "js_number")]
    pub duration: f64,
    #[serde(with = "js_number")]
    pub volume: f64,
}

// ---- Queue -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub id: i64,
    pub track: Track,
    pub added_by_name: Option<String>,
    /// Added by the requesting client.
    pub mine: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub entry: Option<QueueEntry>,
    #[serde(with = "js_number")]
    pub position: f64,
    #[serde(with = "js_number")]
    pub duration: f64,
    pub playing: bool,
    /// The track comes from the standby playlist rather than a guest.
    pub is_standby: bool,
    /// 0 when the count is hidden.
    pub downvotes: i64,
    /// 0 = disabled; negative = same threshold, count hidden.
    pub downvote_threshold: i32,
    pub downvoted_by_me: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandbyEntry {
    pub id: i64,
    pub track: Track,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandbyState {
    pub enabled: bool,
    pub shuffle: bool,
    pub random: bool,
    pub entries: Vec<StandbyEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub now_playing: NowPlaying,
    pub queue: Vec<QueueEntry>,
    /// 0 = no limit; negative = same limit, counter hidden.
    pub per_user_limit: i32,
    pub my_queue_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueRequest {
    pub track_id: i64,
}

// ---- History & stats ---------------------------------------------------------------

/// Where a play came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaySource {
    Guest,
    Standby,
    Random,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayHistoryItem {
    pub id: i64,
    pub track_id: i64,
    pub title: String,
    pub artist: Option<String>,
    /// Null once the track has been pruned from the library.
    pub art_hash: Option<String>,
    /// Null for filler.
    pub requested_by_name: Option<String>,
    pub source: PlaySource,
    pub played_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackStat {
    pub track_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStat {
    /// The IP for admins, an opaque digest of it for guests.
    pub id: String,
    /// Admins only.
    pub ip: Option<String>,
    pub name: Option<String>,
    pub requests: i64,
    pub downvotes: i64,
    /// Always false for non-admin viewers.
    pub banned: bool,
    pub banned_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsTotals {
    pub plays: i64,
    pub requests: i64,
    pub downvotes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsResponse {
    pub history: Vec<PlayHistoryItem>,
    pub top_played: Vec<TrackStat>,
    pub top_downvoted: Vec<TrackStat>,
    pub users: Vec<UserStat>,
    pub totals: StatsTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BanEntry {
    pub ip: String,
    pub name: Option<String>,
    pub banned_at: String,
    /// Null = permanent.
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BansResponse {
    pub bans: Vec<BanEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BanRequest {
    pub ip: String,
    /// Omitted or 0 = permanent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
}

// ---- Realtime --------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPayload {
    #[serde(with = "js_number")]
    pub position: f64,
    #[serde(with = "js_number")]
    pub duration: f64,
    pub playing: bool,
    pub track_id: Option<i64>,
}

/// Pushed server → client over `/ws`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "lowercase")]
pub enum RealtimeMessage {
    /// Boxed: a full queue state is many times the size of a progress tick.
    Queue(Box<QueueState>),
    Progress(ProgressPayload),
}
