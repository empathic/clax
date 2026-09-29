//! The `artifax_viewer` cookie: a host-only ULID naming a browser viewer.

use artifax_core::feedback::display_name;
use artifax_core::{Store, is_ulid};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use std::convert::Infallible;

pub const COOKIE: &str = "artifax_viewer";

/// The viewer ID from the request's cookie; `None` when absent or not a ULID.
pub struct ViewerCookie(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for ViewerCookie {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        Ok(ViewerCookie(read(&parts.headers)))
    }
}

fn read(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
        .filter(|v| is_ulid(v))
}

/// `Set-Cookie` for viewer `id`: host-only (no `Domain`), `HttpOnly`, `SameSite=Lax`, five years.
pub fn set_cookie(id: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={id}; Path=/; Max-Age=157680000; HttpOnly; SameSite=Lax"
    ))
    .expect("ULIDs are header-safe")
}

/// The name a viewer comment is attributed to: the viewer's display name,
/// sanitised, else `Viewer`.
pub fn author_name(st: &Store, cookie: Option<&str>) -> artifax_core::Result<String> {
    let name = match cookie {
        Some(id) => st.get_viewer(id)?.and_then(|v| v.display_name),
        None => None,
    };
    Ok(display_name(name.as_deref().unwrap_or("")))
}
