//! `/api/player/*`: playback state and the admin's transport controls.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use jukebox_core::js;
use jukebox_core::types::{DevicesResponse, OutputResponse};
use serde_json::Value;

use super::{blocking, internal, require_admin, AppStateRef};
use crate::http::{error, ok, JsonBody};

pub async fn state(State(state): AppStateRef) -> Response {
    ok(&state.player.state())
}

pub async fn devices(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    state.player.request_devices();
    ok(&DevicesResponse {
        devices: state.player.devices(),
        selected: state.config.get().output_device_id,
    })
}

pub async fn output(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let device_id = body.string_or_empty("deviceId");
    let config = state.config.clone();
    let id = device_id.clone();
    let saved =
        blocking(move || config.update(|c| c.output_device_id = (!id.is_empty()).then_some(id)))
            .await;
    if let Err(err) = saved {
        return internal(err);
    }
    state.player.set_output_device(&device_id);
    ok(&OutputResponse {
        ok: true,
        selected: state.config.get().output_device_id,
    })
}

pub async fn load(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let track_id = body.number("trackId");
    if !js::is_integer(track_id) {
        return error(StatusCode::BAD_REQUEST, "trackId required");
    }
    let autoplay = body.get("autoplay") != Some(&Value::Bool(false));
    state.player.load(track_id as i64, autoplay);
    ok(&state.player.state())
}

pub async fn play(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    state.player.play();
    ok(&state.player.state())
}

pub async fn pause(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    state.player.pause();
    ok(&state.player.state())
}

pub async fn seek(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let position = body.number("position");
    if !position.is_finite() {
        return error(StatusCode::BAD_REQUEST, "position required");
    }
    state.player.seek(position);
    ok(&state.player.state())
}

pub async fn volume(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let value = body.number("value");
    if !value.is_finite() {
        return error(StatusCode::BAD_REQUEST, "value required");
    }
    state.player.set_volume(value);
    ok(&state.player.state())
}
