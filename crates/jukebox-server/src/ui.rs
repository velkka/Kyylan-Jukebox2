//! The web UI, compiled into the binary and served to every client, as server.ts served
//! out/renderer: files by path, `/` as the guest app, and a directory as its index.html.
//!
//! The Electron build also held the hidden player window's page; the Rust build has no
//! player page, so it isn't embedded.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

use crate::http::not_found;

#[derive(Embed)]
#[folder = "$KYYLAN_UI_DIR"]
#[exclude = "player/*"]
struct Assets;

/// Whether a built UI was embedded.
pub fn is_embedded() -> bool {
    Assets::get("web/index.html").is_some()
}

pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return not_found(&method, uri.path());
    }
    let raw = uri.path();
    if raw == "/" {
        return match Assets::get("web/index.html") {
            Some(_) => file("web/index.html", &headers),
            None => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "Kyylan Jukebox server is running (dev mode: open the app window).",
            )
                .into_response(),
        };
    }
    let Some(path) = decoded_path(raw) else {
        return not_found(&method, raw);
    };
    if Assets::get(&path).is_some() {
        return file(&path, &headers);
    }
    // A directory: redirect to the slash form, then serve its index.
    let index = format!("{}/index.html", path.trim_end_matches('/'));
    if Assets::get(&index).is_some() {
        if !raw.ends_with('/') {
            let location = format!("{raw}/");
            return (
                StatusCode::MOVED_PERMANENTLY,
                [(header::LOCATION, location.clone())],
                format!("Redirecting to {location}"),
            )
                .into_response();
        }
        return file(&index, &headers);
    }
    not_found(&method, raw)
}

/// The embedded path for a request path, or `None` for one that tries to leave the UI or
/// reach a hidden file.
fn decoded_path(raw: &str) -> Option<String> {
    let decoded = percent_decode(raw)?;
    let path = decoded.trim_start_matches('/');
    let safe = path
        .split('/')
        .all(|segment| segment != ".." && !segment.starts_with('.') || segment.is_empty());
    safe.then(|| path.to_string())
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn file(path: &str, request: &HeaderMap) -> Response {
    let asset = Assets::get(path).expect("checked by the caller");
    let etag = format!(
        "\"{}\"",
        asset.metadata.sha256_hash()[..10]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = match (mime.type_(), mime.subtype().as_str()) {
        (mime_guess::mime::TEXT, _) | (mime_guess::mime::APPLICATION, "javascript" | "json") => {
            format!("{}; charset=UTF-8", mime.essence_str())
        }
        _ => mime.essence_str().to_string(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).unwrap(),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0"),
    );
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let fresh = request
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|tag| tag.trim() == etag));
    if fresh {
        return (StatusCode::NOT_MODIFIED, headers).into_response();
    }
    (StatusCode::OK, headers, Body::from(asset.data.into_owned())).into_response()
}
