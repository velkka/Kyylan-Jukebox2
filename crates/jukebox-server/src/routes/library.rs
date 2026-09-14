//! Browsing (`/api/tracks`, `/api/artists`, `/api/albums`, `/api/art/:hash`) and library
//! management (`/api/library/*`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use jukebox_core::library::query::{self, AlbumsQuery, ArtistsQuery};
use jukebox_core::library::{export, folders};
use jukebox_core::types::{BrowseFolderResponse, TracksQuery};

use super::{blocking, internal, require_admin, respond, AppStateRef, Handled};
use crate::http::{empty, error, ok, Client, JsonBody, Query};
use crate::AppState;

pub async fn tracks(State(state): AppStateRef, uri: Uri) -> Response {
    let q = Query::parse(uri.query());
    let noalbum = q.string("noAlbum");
    let query = TracksQuery {
        search: q.string("search").map(String::from),
        artist: q.string("artist").map(String::from),
        album: q.string("album").map(String::from),
        album_artist: q.string("albumArtist").map(String::from),
        no_album: Some(noalbum == Some("1") || noalbum == Some("true")),
        limit: q.page_number("limit"),
        offset: q.page_number("offset"),
    };
    read(state, move |conn| query::tracks(conn, &query)).await
}

pub async fn artists(State(state): AppStateRef, uri: Uri) -> Response {
    let q = Query::parse(uri.query());
    let query = ArtistsQuery {
        search: q.string("search").map(String::from),
        letter: q.string("letter").map(String::from),
        limit: q.page_number("limit"),
        offset: q.page_number("offset"),
    };
    read(state, move |conn| query::artists(conn, &query)).await
}

pub async fn albums(State(state): AppStateRef, uri: Uri) -> Response {
    let q = Query::parse(uri.query());
    let query = AlbumsQuery {
        search: q.string("search").map(String::from),
        artist: q.string("artist").map(String::from),
        limit: q.page_number("limit"),
        offset: q.page_number("offset"),
    };
    read(state, move |conn| query::albums(conn, &query)).await
}

/// A read-only query answered as JSON.
async fn read<T: serde::Serialize + Send + 'static>(
    state: Arc<AppState>,
    query: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Response {
    match blocking(move || state.reads.with(query)).await {
        Ok(value) => ok(&value),
        Err(err) => internal(err),
    }
}

pub async fn art(State(state): AppStateRef, Path(hash): Path<String>) -> Response {
    match blocking(move || state.reads.with(|conn| query::art(conn, &hash))).await {
        Ok(Some((mime, data))) => (
            [
                (header::CONTENT_TYPE, mime),
                // Content-addressed, so it never changes for a given hash.
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_string(),
                ),
            ],
            data,
        )
            .into_response(),
        Ok(None) => empty(StatusCode::NOT_FOUND),
        Err(err) => internal(err),
    }
}

async fn paths_response(state: Arc<AppState>) -> Handled {
    let paths = state.config.get().library_paths;
    blocking(move || state.reads.with(|conn| folders::with_counts(conn, &paths)))
        .await
        .map(|counts| ok(&counts))
        .map_err(internal)
}

pub async fn paths(State(state): AppStateRef, headers: HeaderMap) -> Response {
    respond(
        async {
            require_admin(&state, &headers)?;
            paths_response(state).await
        }
        .await,
    )
}

pub async fn add_path(State(state): AppStateRef, headers: HeaderMap, body: JsonBody) -> Response {
    respond(
        async {
            require_admin(&state, &headers)?;
            let path = body.string_or_empty("path");
            let config = state.config.clone();
            blocking(move || folders::add(&config, &path))
                .await
                .map_err(|err| error(StatusCode::BAD_REQUEST, &err.to_string()))?;
            paths_response(state).await
        }
        .await,
    )
}

pub async fn remove_path(
    State(state): AppStateRef,
    headers: HeaderMap,
    body: JsonBody,
) -> Response {
    respond(
        async {
            require_admin(&state, &headers)?;
            let path = body.string_or_empty("path");
            let config = state.config.clone();
            blocking(move || folders::remove(&config, &path))
                .await
                .map_err(internal)?;
            paths_response(state).await
        }
        .await,
    )
}

/// The native folder picker. Host only: an admin elsewhere on the network must not be able to
/// pop a dialog up on the host.
pub async fn browse(State(state): AppStateRef, client: Client, headers: HeaderMap) -> Response {
    respond(
        async {
            require_admin(&state, &headers)?;
            if !client.is_local() {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "The folder picker is only available on the host machine",
                ));
            }
            let picker = state.folder_picker.clone();
            let Some(chosen) = blocking(move || picker.pick_folder()).await else {
                return Ok(ok(&BrowseFolderResponse {
                    canceled: true,
                    path: None,
                }));
            };
            let config = state.config.clone();
            let path = chosen.clone();
            blocking(move || folders::add(&config, &path))
                .await
                .map_err(|err| error(StatusCode::BAD_REQUEST, &err.to_string()))?;
            Ok(ok(&BrowseFolderResponse {
                canceled: false,
                path: Some(chosen),
            }))
        }
        .await,
    )
}

/// Admin only: the export carries file paths, which guests never see.
pub async fn export(State(state): AppStateRef, headers: HeaderMap) -> Response {
    respond(
        async {
            require_admin(&state, &headers)?;
            let csv = blocking(move || state.reads.with(export::library_csv))
                .await
                .map_err(internal)?;
            let stamp = chrono::Utc::now().format("%Y-%m-%d");
            Ok((
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"kyylan-library-{stamp}.csv\""),
                    ),
                ],
                csv,
            )
                .into_response())
        }
        .await,
    )
}

/// Starts a rescan in the background and answers at once; progress is polled.
pub async fn scan(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    match crate::start_scan(&state) {
        Some(started) => ok(&started),
        None => ok(&state.scanner.status()),
    }
}

pub async fn scan_status(State(state): AppStateRef, headers: HeaderMap) -> Response {
    if let Err(denied) = require_admin(&state, &headers) {
        return denied;
    }
    ok(&state.scanner.status())
}
