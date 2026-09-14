//! `/api/admin/settings`, `/api/health`, `/api/config`, `/api/setup`.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use jukebox_core::js;
use jukebox_core::types::{
    AdminSettings, HealthResponse, PublicConfig, SaveSettingsResponse, SetupResponse,
};
use serde_json::Value;

use super::{blocking, internal, require_admin, respond, AppStateRef, Handled, APP_NAME};
use crate::http::{error, ok, Client, JsonBody};
use crate::SetupAccess;

pub async fn get(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let c = state.config.get();
    ok(&AdminSettings {
        port: c.port,
        per_user_queue_limit: c.per_user_queue_limit,
        downvote_skip_threshold: c.downvote_skip_threshold,
        same_song_cooldown_minutes: c.same_song_cooldown_minutes,
        same_artist_cooldown_minutes: c.same_artist_cooldown_minutes,
        add_rate_limit_minutes: c.add_rate_limit_minutes,
    })
}

#[derive(Default)]
struct Patch {
    same_song: Option<u32>,
    same_artist: Option<u32>,
    rate_limit: Option<u32>,
    per_user_limit: Option<i32>,
    downvote_threshold: Option<i32>,
    port: Option<u16>,
    password: Option<String>,
}

/// `Number(body[key])` when the key is there, if it's an integer within the bounds.
fn bounded(body: &JsonBody, key: &str, min: f64, max: f64) -> Result<Option<f64>, ()> {
    if body.get(key).is_none() {
        return Ok(None);
    }
    let n = body.number(key);
    if js::is_integer(n) && n >= min && n <= max {
        Ok(Some(n))
    } else {
        Err(())
    }
}

pub async fn save(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    respond(
        async {
            require_admin(&state, &headers)?;
            let bad = |message: &str| error(StatusCode::BAD_REQUEST, message);
            let mut patch = Patch::default();

            // Repeat cooldowns and the rate limit, in minutes: 0 is off, a day at most.
            let cooldown = |key| {
                bounded(&body, key, 0.0, 1440.0)
                    .map(|n| n.map(|n| n as u32))
                    .map_err(|_| bad("Cooldowns must be between 0 and 1440 minutes"))
            };
            patch.same_song = cooldown("sameSongCooldownMinutes")?;
            patch.same_artist = cooldown("sameArtistCooldownMinutes")?;
            patch.rate_limit = cooldown("addRateLimitMinutes")?;
            // 0 is no limit; a negative limit applies but hides the counter.
            patch.per_user_limit = bounded(&body, "perUserQueueLimit", -100.0, 100.0)
                .map_err(|_| bad("Per-guest limit must be between -100 and 100"))?
                .map(|n| n as i32);
            // 0 is off; a negative threshold applies but hides the count.
            patch.downvote_threshold = bounded(&body, "downvoteSkipThreshold", -100.0, 100.0)
                .map_err(|_| bad("Downvote threshold must be between -100 and 100"))?
                .map(|n| n as i32);
            patch.port = bounded(&body, "port", 1.0, 65535.0)
                .map_err(|_| bad("Port must be between 1 and 65535"))?
                .map(|n| n as u16);
            if let Some(password) = body.get("adminPassword") {
                match password {
                    Value::String(p) if !p.is_empty() => patch.password = Some(p.clone()),
                    _ => return Err(bad("Password cannot be empty")),
                }
            }

            let rebroadcast = patch.per_user_limit.is_some() || patch.downvote_threshold.is_some();
            let restart_required = patch.port.is_some_and(|p| p != state.running_port);
            let config = state.config.clone();
            blocking(move || {
                config.update(|c| {
                    if let Some(v) = patch.same_song {
                        c.same_song_cooldown_minutes = v;
                    }
                    if let Some(v) = patch.same_artist {
                        c.same_artist_cooldown_minutes = v;
                    }
                    if let Some(v) = patch.rate_limit {
                        c.add_rate_limit_minutes = v;
                    }
                    if let Some(v) = patch.per_user_limit {
                        c.per_user_queue_limit = v;
                    }
                    if let Some(v) = patch.downvote_threshold {
                        c.downvote_skip_threshold = v;
                    }
                    if let Some(v) = patch.port {
                        c.port = v;
                    }
                    if let Some(v) = patch.password {
                        c.admin_password = v;
                    }
                })
            })
            .await
            .map_err(internal)?;
            // Everyone's limit label and downvote button depend on these.
            if rebroadcast {
                let hub = state.hub.clone();
                blocking(move || hub.broadcast_queue()).await;
            }
            Handled::Ok(ok(&SaveSettingsResponse { restart_required }))
        }
        .await,
    )
}

pub async fn health(State(state): AppStateRef) -> Response {
    let network = state.network.clone();
    let addresses = blocking(move || network.lan_addresses()).await;
    ok(&HealthResponse {
        name: APP_NAME.into(),
        version: state.version.clone(),
        addresses,
        port: state.config.get().port,
    })
}

pub async fn public_config(State(state): AppStateRef) -> Response {
    let c = state.config.get();
    ok(&PublicConfig {
        configured: c.configured,
        port: c.port,
        per_user_queue_limit: c.per_user_queue_limit,
        name: APP_NAME.into(),
        version: state.version.clone(),
    })
}

/// First-run setup, allowed only until the app is configured.
pub async fn setup(State(state): AppStateRef, client: Client, body: JsonBody) -> Response {
    let config = state.config.get();
    if config.configured {
        return error(StatusCode::FORBIDDEN, "Already configured");
    }
    match state.setup {
        SetupAccess::Anyone => {}
        SetupAccess::HostOnly if client.is_local() => {}
        SetupAccess::HostOnly => {
            return error(
                StatusCode::FORBIDDEN,
                "Setup can only be done on the computer the jukebox runs on",
            )
        }
        SetupAccess::Disabled => {
            return error(
                StatusCode::FORBIDDEN,
                "Setup is done in the config file on this jukebox",
            )
        }
    }
    let password = match body.get("adminPassword") {
        Some(Value::String(p)) if !js::trim(p).is_empty() => p.clone(),
        _ => return error(StatusCode::BAD_REQUEST, "Admin password is required"),
    };
    let port = match body.get("port") {
        None => config.port,
        Some(Value::Number(n))
            if n.as_f64()
                .is_some_and(|n| js::is_integer(n) && (1.0..=65535.0).contains(&n)) =>
        {
            n.as_f64().unwrap() as u16
        }
        Some(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Port must be an integer between 1 and 65535",
            )
        }
    };
    let store = state.config.clone();
    let saved = blocking(move || {
        store.update(|c| {
            c.configured = true;
            c.admin_password = password;
            c.port = port;
        })
    })
    .await;
    if let Err(err) = saved {
        return internal(err);
    }
    ok(&SetupResponse {
        ok: true,
        restart_required: port != state.running_port,
        port,
    })
}
