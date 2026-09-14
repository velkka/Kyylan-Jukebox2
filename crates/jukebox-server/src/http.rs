//! Reading requests and writing responses the way the Express server did: who sent a
//! request, its JSON body and query string as Express parsed them, and responses with the
//! same status codes, headers and error shapes.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use jukebox_core::js;
use serde::Serialize;
use serde_json::{Map, Value};

/// The client's address as a stable per-guest identity: `::ffff:` stripped from IPv4-mapped
/// addresses, and IPv6 loopback treated as IPv4 loopback. Mirrors net.ts's `normalizeIp`.
pub fn normalize_ip(raw: &str) -> String {
    if raw.is_empty() {
        return "unknown".into();
    }
    let ip = raw.strip_prefix("::ffff:").unwrap_or(raw);
    if ip == "::1" {
        "127.0.0.1".into()
    } else {
        ip.to_string()
    }
}

/// The client that sent a request.
pub struct Client {
    pub ip: String,
}

impl Client {
    /// The request came from the host itself — what gates the folder picker.
    pub fn is_local(&self) -> bool {
        self.ip == "127.0.0.1"
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| normalize_ip(&addr.ip().to_string()))
            .unwrap_or_else(|| "unknown".into());
        Ok(Client { ip })
    }
}

/// A request body as `express.json()` left it: parsed when the request says it's JSON,
/// otherwise an empty object.
pub struct JsonBody(pub Value);

/// body-parser's default size limit.
const BODY_LIMIT: usize = 100 * 1024;

impl JsonBody {
    /// `req.body.key`, or `None` for `undefined`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.as_object().and_then(|o| o.get(key))
    }

    /// `req.body.key ?? ''` passed through `String()`.
    pub fn string_or_empty(&self, key: &str) -> String {
        match self.get(key) {
            None | Some(Value::Null) => String::new(),
            Some(value) => js::to_string(value),
        }
    }

    /// `Number(req.body.key)`.
    pub fn number(&self, key: &str) -> f64 {
        js::to_number(self.get(key))
    }
}

impl<S: Send + Sync> FromRequest<S> for JsonBody {
    type Rejection = Response;

    async fn from_request(req: Request, _: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"));
        if !is_json {
            return Ok(JsonBody(Value::Object(Map::new())));
        }
        let bytes = axum::body::to_bytes(req.into_body(), BODY_LIMIT)
            .await
            .map_err(|_| error(StatusCode::PAYLOAD_TOO_LARGE, "request entity too large"))?;
        // Strict mode: an empty body is {}, and only an object or array is accepted.
        let text = String::from_utf8_lossy(&bytes);
        let trimmed = text.trim_start_matches([' ', '\t', '\n', '\r']);
        if trimmed.is_empty() {
            return Ok(JsonBody(Value::Object(Map::new())));
        }
        if !trimmed.starts_with(['{', '[']) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "Request body must be a JSON object",
            ));
        }
        // Express answered a malformed body with its HTML error page; nothing the UI sends
        // hits that, so a JSON error in the usual shape stands in for it.
        serde_json::from_str(trimmed)
            .map(JsonBody)
            .map_err(|e| error(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}")))
    }
}

/// A query string as Express's default parser (`qs`) read it, as far as the routes look:
/// a parameter given once is a string, and one given twice or with brackets is an array or
/// object, which no route treats as a string.
pub struct Query(HashMap<String, Option<String>>);

impl Query {
    pub fn parse(raw: Option<&str>) -> Self {
        let mut params: HashMap<String, Option<String>> = HashMap::new();
        for pair in raw.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = decode(key);
            if key.is_empty() {
                continue;
            }
            let (base, complex) = match key.find('[') {
                Some(i) if i > 0 => (key[..i].to_string(), true),
                _ => (key, false),
            };
            match params.get_mut(&base) {
                Some(existing) => *existing = None,
                None => {
                    params.insert(base, (!complex).then(|| decode(value)));
                }
            }
        }
        Query(params)
    }

    /// The parameter when it's a plain string.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.as_deref())
    }

    /// `Number(req.query.key)`.
    pub fn number(&self, key: &str) -> f64 {
        match self.0.get(key) {
            None => f64::NAN,
            Some(Some(s)) => js::string_to_number(s),
            Some(None) => f64::NAN,
        }
    }

    /// `req.query.key ? Number(req.query.key) : undefined`, as a whole number for SQL.
    ///
    /// A fraction went straight into SQLite's `LIMIT` in Electron and failed the request;
    /// here it's truncated instead.
    pub fn page_number(&self, key: &str) -> Option<i64> {
        match self.0.get(key) {
            Some(Some(s)) if s.is_empty() => None,
            None => None,
            _ => {
                let n = self.number(key);
                n.is_finite()
                    .then(|| n.trunc().clamp(i64::MIN as f64, i64::MAX as f64) as i64)
            }
        }
    }
}

/// `decodeURIComponent` with `+` as a space, as `qs` decodes.
fn decode(s: &str) -> String {
    decode_uri_component(&s.replace('+', " "))
}

/// `decodeURIComponent`, leaving the string as it was when it has a malformed escape — the
/// fallback `qs` and cookie-parser both apply.
fn decode_uri_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let byte = bytes
                .get(i + 1..i + 3)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
            let Some(byte) = byte else {
                return s.to_string();
            };
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// A cookie's value: the first of that name, percent-decoded, as cookie-parser read it.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v);
            decode_uri_component(v)
        })
}

/// `res.json(value)`.
pub fn json(status: StatusCode, value: &impl Serialize) -> Response {
    let body = serde_json::to_vec(value).expect("responses serialize");
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response()
}

pub fn ok(value: &impl Serialize) -> Response {
    json(StatusCode::OK, value)
}

/// `res.status(status).json({ error })`.
pub fn error(status: StatusCode, message: &str) -> Response {
    json(status, &serde_json::json!({ "error": message }))
}

/// `res.status(status).end()`.
pub fn empty(status: StatusCode) -> Response {
    (status, [(header::CONTENT_LENGTH, "0")], Body::empty()).into_response()
}

/// Express's page for a request no route handled: `Cannot GET /path`.
pub fn not_found(method: &Method, path: &str) -> Response {
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>Cannot {method} {}</pre>\n</body>\n</html>\n",
        escape_html(path)
    );
    let mut response = (StatusCode::NOT_FOUND, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_normalize_like_net_ts() {
        assert_eq!(normalize_ip("::ffff:192.168.1.5"), "192.168.1.5");
        assert_eq!(normalize_ip("::1"), "127.0.0.1");
        assert_eq!(normalize_ip("fe80::1"), "fe80::1");
        assert_eq!(normalize_ip(""), "unknown");
    }

    #[test]
    fn query_strings_parse_like_qs() {
        let q = Query::parse(Some(
            "search=ogg+song&limit=2&limit=3&a%20b=%C3%A4&bad=%zz&x[]=1&empty=",
        ));
        assert_eq!(q.string("search"), Some("ogg song"));
        assert_eq!(q.string("limit"), None);
        assert!(q.number("limit").is_nan());
        assert_eq!(q.string("a b"), Some("ä"));
        assert_eq!(q.string("bad"), Some("%zz"));
        assert_eq!(q.string("x"), None);
        assert_eq!(q.page_number("empty"), None);
        assert_eq!(q.page_number("missing"), None);
        assert_eq!(
            Query::parse(Some("limit=0x2")).page_number("limit"),
            Some(2)
        );
        assert_eq!(Query::parse(Some("limit=abc")).page_number("limit"), None);
    }

    #[test]
    fn cookies_parse_like_cookie_parser() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; kj_session=abc%20d; kj_session=second"),
        );
        assert_eq!(cookie(&headers, "kj_session").as_deref(), Some("abc d"));
        assert_eq!(cookie(&headers, "missing"), None);
    }
}
