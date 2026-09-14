//! `/api/auth`, `/api/login`, `/api/logout`.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use jukebox_core::types::AuthStatus;

use super::AppStateRef;
use crate::auth::{cleared_cookie, session_cookie, SESSION_COOKIE};
use crate::http::{cookie, error, ok, Client, JsonBody};

pub async fn status(State(state): AppStateRef, client: Client, headers: HeaderMap) -> Response {
    ok(&AuthStatus {
        is_admin: state.sessions.is_admin(&headers),
        is_local: client.is_local(),
    })
}

pub async fn login(State(state): AppStateRef, client: Client, body: JsonBody) -> Response {
    let password = body.string_or_empty("password");
    let Some(token) = state
        .sessions
        .login(&password, &state.config.get().admin_password)
    else {
        return error(StatusCode::UNAUTHORIZED, "Incorrect password");
    };
    let status = AuthStatus {
        is_admin: true,
        is_local: client.is_local(),
    };
    ([(header::SET_COOKIE, session_cookie(&token))], ok(&status)).into_response()
}

pub async fn logout(State(state): AppStateRef, client: Client, headers: HeaderMap) -> Response {
    state
        .sessions
        .logout(cookie(&headers, SESSION_COOKIE).as_deref());
    let status = AuthStatus {
        is_admin: false,
        is_local: client.is_local(),
    };
    ([(header::SET_COOKIE, cleared_cookie())], ok(&status)).into_response()
}
