//! `/api/queue*`: the queue as each guest sees it, adding and removing, downvotes, and the
//! admin's reordering, skip and clear.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use jukebox_core::engine::{Engine, EngineError};
use jukebox_core::js;

use super::{blocking, engine_error, require_admin, AppStateRef};
use crate::http::{error, ok, Client, JsonBody};
use crate::AppState;

/// Runs an engine operation, then answers with the queue as this client now sees it.
async fn then_state(
    state: &Arc<AppState>,
    ip: String,
    op: impl FnOnce(&Engine, &str) -> Result<(), EngineError> + Send + 'static,
) -> Response {
    let engine = state.engine.clone();
    let result = blocking(move || {
        op(&engine, &ip)?;
        engine.queue_state(&ip)
    })
    .await;
    match result {
        Ok(queue) => ok(&queue),
        Err(err) => engine_error(err),
    }
}

/// A guest's display name: their device's hostname.
async fn hostname(state: &Arc<AppState>, ip: &str) -> String {
    let (network, ip) = (state.network.clone(), ip.to_string());
    blocking(move || network.hostname(&ip)).await
}

pub async fn state(State(state): AppStateRef, client: Client) -> Response {
    then_state(&state, client.ip, |_, _| Ok(())).await
}

pub async fn add(State(state): AppStateRef, client: Client, body: JsonBody) -> Response {
    let track_id = body.number("trackId");
    if !js::is_integer(track_id) {
        return error(StatusCode::BAD_REQUEST, "trackId required");
    }
    let name = hostname(&state, &client.ip).await;
    then_state(&state, client.ip, move |engine, ip| {
        engine.enqueue(track_id as i64, ip, Some(&name))
    })
    .await
}

pub async fn remove(
    State(state): AppStateRef,
    client: Client,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    // Admins can remove any entry; guests only their own.
    let is_admin = state.sessions.is_admin(&headers);
    let id = js::string_to_number(&id);
    then_state(&state, client.ip, move |engine, ip| {
        engine.remove_entry(id, ip, is_admin)
    })
    .await
}

pub async fn downvote(State(state): AppStateRef, client: Client) -> Response {
    let name = hostname(&state, &client.ip).await;
    then_state(&state, client.ip, move |engine, ip| {
        engine.downvote(ip, Some(&name))
    })
    .await
}

pub async fn move_entry(
    State(state): AppStateRef,
    client: Client,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: JsonBody,
) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    let to_index = body.number("toIndex");
    if !js::is_integer(to_index) {
        return error(StatusCode::BAD_REQUEST, "toIndex required");
    }
    let id = js::string_to_number(&id);
    then_state(&state, client.ip, move |engine, _| {
        engine.reorder(id, to_index)
    })
    .await
}

pub async fn skip(State(state): AppStateRef, client: Client, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    then_state(&state, client.ip, |engine, _| engine.skip()).await
}

pub async fn clear(State(state): AppStateRef, client: Client, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    then_state(&state, client.ip, |engine, _| engine.clear_pending()).await
}
