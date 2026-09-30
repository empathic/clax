//! The `clax_viewer` cookie: a host-only ULID naming a browser viewer.

use crate::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use clax_core::feedback::display_name;
use clax_core::{Store, is_ulid};
use std::convert::Infallible;

pub const COOKIE: &str = "clax_viewer";

/// The viewer ID from the request's cookie; `None` when absent or not a ULID.
pub struct ViewerCookie(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for ViewerCookie {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        Ok(ViewerCookie(read(&parts.headers)))
    }
}

/// The first `clax_viewer` cookie whose value is a ULID; malformed ones
/// (including earlier duplicates) are skipped.
pub(crate) fn read(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v)
        .find(|v| is_ulid(v))
        .map(str::to_string)
}

/// Admits a viewer request only when it carries no `Origin` header (scripts,
/// curl) or its `Origin` is the daemon's own: `http://` plus the request's
/// `Host`. Artifact origins (`<aid>.localhost:<port>`), `null`, and foreign
/// origins are refused with 403 `forbidden_origin`, so a published page cannot
/// comment, send, resolve, or rename the viewer on a person's behalf.
pub struct SameOrigin;

impl<S: Send + Sync> FromRequestParts<S> for SameOrigin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, ApiError> {
        let Some(origin) = parts.headers.get(header::ORIGIN) else {
            return Ok(SameOrigin);
        };
        let host = parts
            .headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok());
        let same = match (origin.to_str().ok(), host) {
            (Some(o), Some(h)) => o.strip_prefix("http://").is_some_and(|rest| {
                !h.is_empty()
                    && rest.eq_ignore_ascii_case(h)
                    && crate::host::artifact_host(h).is_none()
            }),
            _ => false,
        };
        if same {
            Ok(SameOrigin)
        } else {
            Err(ApiError::forbidden(
                "forbidden_origin",
                "viewer routes accept requests only from the Clax shell's own origin",
            ))
        }
    }
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
pub fn author_name(st: &Store, cookie: Option<&str>) -> clax_core::Result<String> {
    let name = match cookie {
        Some(id) => st.get_viewer(id)?.and_then(|v| v.display_name),
        None => None,
    };
    Ok(display_name(name.as_deref().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_clax_viewer_cookie_names_a_viewer() {
        // Cookies on localhost ignore the port, so the previous daemon's
        // cookie reaches this one; it never names a viewer here.
        const OLD: &str = concat!("arti", "fax");
        let id = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{OLD}_viewer={id}")).unwrap(),
        );
        assert_eq!(read(&h), None);
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{OLD}_viewer={id}; clax_viewer={id}")).unwrap(),
        );
        assert_eq!(read(&h).as_deref(), Some(id));
    }
}
