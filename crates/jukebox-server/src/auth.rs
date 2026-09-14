//! Admin sessions. Mirrors src/main/auth.ts: a login issues a random token held in memory
//! for 12 hours and sent back as the `kj_session` cookie. A restart logs admins out.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::http::cookie;

pub const SESSION_COOKIE: &str = "kj_session";
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Default)]
pub struct Sessions {
    tokens: Mutex<HashMap<String, Instant>>,
}

impl Sessions {
    /// A new session when `password` is the configured one. No password configured means
    /// nobody can log in.
    pub fn login(&self, password: &str, expected: &str) -> Option<String> {
        if expected.is_empty() || password != expected {
            return None;
        }
        let token: String = (0..32)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        self.lock()
            .insert(token.clone(), Instant::now() + SESSION_TTL);
        Some(token)
    }

    pub fn logout(&self, token: Option<&str>) {
        if let Some(token) = token {
            self.lock().remove(token);
        }
    }

    pub fn is_valid(&self, token: Option<&str>) -> bool {
        let Some(token) = token else { return false };
        let mut tokens = self.lock();
        let now = Instant::now();
        tokens.retain(|_, expires| *expires > now);
        tokens.contains_key(token)
    }

    /// Whether the request carries a live admin session cookie.
    pub fn is_admin(&self, headers: &HeaderMap) -> bool {
        self.is_valid(cookie(headers, SESSION_COOKIE).as_deref())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
        self.tokens.lock().expect("session lock poisoned")
    }
}

/// `res.cookie(SESSION_COOKIE, token, { httpOnly, sameSite: 'lax', maxAge })`.
pub fn session_cookie(token: &str) -> String {
    let expires = chrono::Utc::now() + SESSION_TTL;
    format!(
        "{SESSION_COOKIE}={token}; Max-Age={}; Path=/; Expires={}; HttpOnly; SameSite=Lax",
        SESSION_TTL.as_secs(),
        expires.format("%a, %d %b %Y %H:%M:%S GMT")
    )
}

/// `res.clearCookie(SESSION_COOKIE)`.
pub fn cleared_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT")
}
