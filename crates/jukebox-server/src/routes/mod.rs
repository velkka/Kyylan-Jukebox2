//! The API routes, grouped as api.ts grouped them. Each handler reproduces its Express
//! counterpart: the same validation in the same order, the same messages, the same statuses.

mod auth;
mod library;
mod player;
mod queue;
mod settings;
mod standby;
mod stats;
mod stream;

use std::sync::Arc;

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use jukebox_core::engine::EngineError;

use crate::http::{error, not_found, Client};
use crate::{realtime, ui, AppState};

pub(crate) type AppStateRef = State<Arc<AppState>>;

pub(crate) const APP_NAME: &str = "Kyylan Jukebox";

pub(crate) fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/auth", get(auth::status))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/admin/settings", get(settings::get).post(settings::save))
        .route("/health", get(settings::health))
        .route("/config", get(settings::public_config))
        .route("/setup", post(settings::setup))
        .route("/tracks", get(library::tracks))
        .route("/artists", get(library::artists))
        .route("/albums", get(library::albums))
        .route("/art/{hash}", get(library::art))
        .route(
            "/library/paths",
            get(library::paths)
                .post(library::add_path)
                .delete(library::remove_path),
        )
        .route("/library/browse", post(library::browse))
        .route("/library/export.csv", get(library::export))
        .route("/library/scan", post(library::scan))
        .route("/library/scan/status", get(library::scan_status))
        .route("/stream/{id}", get(stream::stream))
        .route("/player/state", get(player::state))
        .route("/player/devices", get(player::devices))
        .route("/player/output", post(player::output))
        .route("/player/load", post(player::load))
        .route("/player/play", post(player::play))
        .route("/player/pause", post(player::pause))
        .route("/player/seek", post(player::seek))
        .route("/player/volume", post(player::volume))
        .route("/queue", get(queue::state).post(queue::add))
        .route("/queue/{id}", delete(queue::remove))
        .route("/queue/downvote", post(queue::downvote))
        .route("/queue/{id}/move", post(queue::move_entry))
        .route("/queue/skip", post(queue::skip))
        .route("/queue/clear", post(queue::clear))
        .route("/stats", get(stats::get))
        .route("/stats/reset", post(stats::reset))
        .route("/bans", get(stats::bans).post(stats::ban))
        .route("/bans/{ip}", delete(stats::unban))
        .route("/standby", get(standby::get).post(standby::add))
        .route("/standby/{id}", delete(standby::remove))
        .route("/standby/clear", post(standby::clear))
        .route("/standby/settings", post(standby::settings))
        .fallback(express_not_found)
        .method_not_allowed_fallback(express_not_found);

    Router::new()
        .nest("/api", api)
        .route("/ws", get(websocket))
        .fallback(ui::serve)
        .with_state(state)
}

/// Express had no "method not allowed": an unmatched method was simply an unmatched route.
async fn express_not_found(method: Method, OriginalUri(uri): OriginalUri) -> Response {
    not_found(&method, uri.path())
}

async fn websocket(
    State(state): AppStateRef,
    client: Client,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    match upgrade {
        Ok(upgrade) => {
            let hub = state.hub.clone();
            upgrade.on_upgrade(move |socket| realtime::serve_socket(hub, client.ip, socket))
        }
        // Not an upgrade: /ws is an ordinary path to the web UI, as it was.
        Err(_) => ui::serve(method, uri, headers).await,
    }
}

/// Runs database and engine work off the async runtime's threads.
pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work)
        .await
        .expect("request work panicked")
}

/// The 401 every admin route sends without a session.
pub(crate) fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    if state.sessions.is_admin(headers) {
        Ok(())
    } else {
        Err(error(StatusCode::UNAUTHORIZED, "Admin login required"))
    }
}

/// An engine rejection as the status and message api.ts sent; anything else as a 500.
pub(crate) fn engine_error(err: EngineError) -> Response {
    match err {
        EngineError::Rejected { status, message } => error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            &message,
        ),
        EngineError::Database(err) => internal(err),
    }
}

pub(crate) fn internal(err: impl std::fmt::Display) -> Response {
    tracing::error!(%err, "request failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string())
}

/// `?` for handlers: an error that is already a response.
pub(crate) type Handled = Result<Response, Response>;

pub(crate) fn respond(result: Handled) -> Response {
    result.unwrap_or_else(|e| e)
}
