//! `/api/stream/:id`: a track's file with HTTP range support, for guests' pre-listen.

use std::io::SeekFrom;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use jukebox_core::js;
use jukebox_core::library::query::track_path;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::{blocking, AppStateRef};
use crate::http::empty;

pub async fn stream(
    State(state): AppStateRef,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let id = js::string_to_number(&id);
    let path = if js::is_integer(id) {
        blocking(move || state.reads.with(|conn| track_path(conn, id as i64)))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let Some(path) = path else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok(metadata) = tokio::fs::metadata(&path).await else {
        return empty(StatusCode::NOT_FOUND);
    };
    let size = metadata.len();
    let mime = audio_mime(&path);

    let range = headers
        .get(header::RANGE)
        .map(|r| parse_range(r.to_str().unwrap_or(""), size));
    let (status, start, end) = match range {
        None => (StatusCode::OK, 0, size.saturating_sub(1)),
        Some((start, end)) => {
            if start >= size || end >= size || start > end {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        (header::ACCEPT_RANGES, "bytes".to_string()),
                        (header::CONTENT_TYPE, mime.to_string()),
                        (header::CONTENT_RANGE, format!("bytes */{size}")),
                        (header::CONTENT_LENGTH, "0".to_string()),
                    ],
                )
                    .into_response();
            }
            (StatusCode::PARTIAL_CONTENT, start, end)
        }
    };
    let length = if size == 0 { 0 } else { end - start + 1 };

    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return empty(StatusCode::NOT_FOUND);
    };
    if start > 0 && file.seek(SeekFrom::Start(start)).await.is_err() {
        return empty(StatusCode::NOT_FOUND);
    }
    let body = Body::from_stream(ReaderStream::new(file.take(length)));
    let mut response = (status, body).into_response();
    let h = response.headers_mut();
    h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    h.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
    }
    h.insert(header::CONTENT_LENGTH, length.into());
    response
}

/// api.ts's reading of a `Range` header, kept as it was: the first `bytes=<start>-<end>` in
/// it, either number defaulting to the file's start or end. A header it can't read at all
/// still asks for the whole file as a 206, and a suffix range (`bytes=-500`) reads as the
/// first 501 bytes rather than the last 500.
fn parse_range(header: &str, size: u64) -> (u64, u64) {
    fn digits(s: &str) -> (&str, &str) {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        s.split_at(n)
    }
    let mut rest = header;
    while let Some(at) = rest.find("bytes=") {
        let after = &rest[at + "bytes=".len()..];
        let (first, tail) = digits(after);
        if let Some(tail) = tail.strip_prefix('-') {
            let (second, _) = digits(tail);
            let number = |d: &str| d.parse::<u64>().unwrap_or(u64::MAX);
            let start = if first.is_empty() { 0 } else { number(first) };
            let end = if second.is_empty() {
                size.wrapping_sub(1)
            } else {
                number(second)
            };
            return (start, end);
        }
        rest = after;
    }
    (0, size.wrapping_sub(1))
}

/// A browser-friendly audio MIME type for a file's extension.
fn audio_mime(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    match ext.as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("ogg" | "oga") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("wav") => "audio/wav",
        Some("wma") => "audio/x-ms-wma",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_read_like_api_ts() {
        assert_eq!(parse_range("bytes=0-99", 7013), (0, 99));
        assert_eq!(parse_range("bytes=7000-", 7013), (7000, 7012));
        assert_eq!(parse_range("bytes=-50", 7013), (0, 50));
        assert_eq!(parse_range("items=1-2", 10), (0, 9));
        assert_eq!(parse_range("bytes=x bytes=3-4", 10), (3, 4));
        assert_eq!(parse_range("bytes=0-1,5-6", 10), (0, 1));
    }
}
