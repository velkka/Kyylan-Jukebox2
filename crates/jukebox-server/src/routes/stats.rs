//! `/api/stats` and `/api/bans`.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use jukebox_core::js;
use jukebox_core::types::BansResponse;

use super::{blocking, engine_error, require_admin, AppStateRef};
use crate::http::{error, normalize_ip, ok, JsonBody, Query};

/// Public, but guests get the guest list without addresses or who is blocked.
pub async fn get(State(state): AppStateRef, uri: Uri, headers: HeaderMap) -> Response {
    // Math.min(Math.max(Number(limit) || 100, 1), 500)
    let n = Query::parse(uri.query()).number("limit");
    let n = if n == 0.0 || n.is_nan() { 100.0 } else { n };
    let limit = n.clamp(1.0, 500.0).trunc() as i64;
    let for_admin = state.sessions.is_admin(&headers);
    let engine = state.engine.clone();
    match blocking(move || engine.stats(limit, 10, for_admin)).await {
        Ok(stats) => ok(&stats),
        Err(err) => engine_error(err),
    }
}

pub async fn reset(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let engine = state.engine.clone();
    match blocking(move || {
        engine.clear_stats()?;
        engine.stats(100, 10, true)
    })
    .await
    {
        Ok(stats) => ok(&stats),
        Err(err) => engine_error(err),
    }
}

pub async fn bans(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let engine = state.engine.clone();
    match blocking(move || engine.bans()).await {
        Ok(bans) => ok(&BansResponse { bans }),
        Err(err) => engine_error(err),
    }
}

pub async fn ban(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let raw = body.string_or_empty("ip");
    let raw = js::trim(&raw);
    if raw.is_empty() {
        return error(StatusCode::BAD_REQUEST, "ip required");
    }
    let ip = normalize_ip(raw);
    // Omitted or 0 is permanent; otherwise no longer than a month.
    let minutes = match body.get("minutes") {
        None => 0.0,
        Some(_) => body.number("minutes"),
    };
    if !js::is_integer(minutes) || !(0.0..=44640.0).contains(&minutes) {
        return error(
            StatusCode::BAD_REQUEST,
            "minutes must be between 0 and 44640",
        );
    }
    let (network, engine) = (state.network.clone(), state.engine.clone());
    let result = blocking(move || {
        let name = network.hostname(&ip);
        engine.ban(&ip, Some(&name), minutes as i64)
    })
    .await;
    match result {
        Ok(bans) => ok(&BansResponse { bans }),
        Err(err) => engine_error(err),
    }
}

pub async fn unban(
    State(state): AppStateRef,
    headers: HeaderMap,
    Path(ip): Path<String>,
) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let engine = state.engine.clone();
    match blocking(move || engine.unban(&ip)).await {
        Ok(bans) => ok(&BansResponse { bans }),
        Err(err) => engine_error(err),
    }
}
