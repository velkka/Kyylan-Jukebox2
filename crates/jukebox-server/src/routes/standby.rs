//! `/api/standby*`: the standby playlist and its settings.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use jukebox_core::engine::{Engine, EngineError};
use jukebox_core::js;
use jukebox_core::types::StandbyState;
use serde_json::Value;

use super::{blocking, engine_error, require_admin, AppStateRef};
use crate::http::{error, ok, JsonBody};
use crate::AppState;

/// Runs an operation, then answers with the playlist and its settings.
async fn then_state(
    state: &Arc<AppState>,
    op: impl FnOnce(&Engine) -> Result<(), EngineError> + Send + 'static,
) -> Response {
    let (engine, config) = (state.engine.clone(), state.config.clone());
    let result = blocking(move || {
        op(&engine)?;
        let c = config.get();
        Ok::<_, EngineError>(StandbyState {
            enabled: c.standby_enabled,
            shuffle: c.standby_shuffle,
            random: c.standby_random_enabled,
            entries: engine.standby_entries()?,
        })
    })
    .await;
    match result {
        Ok(standby) => ok(&standby),
        Err(err) => engine_error(err),
    }
}

pub async fn get(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    then_state(&state, |_| Ok(())).await
}

pub async fn add(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let track_id = body.number("trackId");
    if !js::is_integer(track_id) {
        return error(StatusCode::BAD_REQUEST, "trackId required");
    }
    let engine = state.engine.clone();
    if let Err(err) = blocking(move || engine.add_standby(track_id as i64)).await {
        return match err {
            EngineError::Rejected { message, .. } => error(StatusCode::BAD_REQUEST, &message),
            other => engine_error(other),
        };
    }
    // Starts the filler now if it's on and nothing is playing.
    then_state(&state, |engine| engine.maybe_start()).await
}

pub async fn remove(
    State(state): AppStateRef,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let id = js::string_to_number(&id);
    then_state(&state, move |engine| engine.remove_standby(id)).await
}

pub async fn clear(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    then_state(&state, |engine| engine.clear_standby()).await
}

pub async fn settings(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let flag = |key| match body.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    };
    let (enabled, shuffle, random) = (flag("enabled"), flag("shuffle"), flag("random"));
    let config = state.config.clone();
    let saved = blocking(move || {
        config.update(|c| {
            if let Some(v) = enabled {
                c.standby_enabled = v;
            }
            if let Some(v) = shuffle {
                c.standby_shuffle = v;
            }
            if let Some(v) = random {
                c.standby_random_enabled = v;
            }
        })
    })
    .await;
    if let Err(err) = saved {
        return super::internal(err);
    }
    // Turning either filler on while idle starts it straight away.
    let start = enabled == Some(true) || random == Some(true);
    then_state(
        &state,
        move |engine| {
            if start {
                engine.maybe_start()
            } else {
                Ok(())
            }
        },
    )
    .await
}
