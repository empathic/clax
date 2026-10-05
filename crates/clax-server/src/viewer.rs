//! The `clax_viewer` cookie: a host-only ULID naming a browser viewer that
//! holds no owner credential (see [`crate::identity`]).

use crate::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use clax_core::feedback::display_name;
use clax_core::{Store, is_ulid};

pub const COOKIE: &str = "clax_viewer";

/// Every distinct ULID value of the `clax_viewer` cookies in `headers`, in
/// the order sent; malformed values are skipped. More than one means another
/// page set a cookie of that name (cookies ignore ports, and one scoped to a
/// longer path is sent first).
pub(crate) fn read_all(headers: &HeaderMap) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v)
        .filter(|v| is_ulid(v))
    {
        if !out.iter().any(|o| o == v) {
            out.push(v.to_string());
        }
    }
    out
}

/// The viewer the `clax_viewer` cookies name: their one ULID value; `None`
/// when there is none, or when they disagree (a shadowing cookie never
/// wins).
pub(crate) fn read(headers: &HeaderMap) -> Option<String> {
    let mut all = read_all(headers);
    (all.len() == 1).then(|| all.remove(0))
}

/// Admits a viewer request only when it carries no `Origin` header (scripts,
/// curl) or its `Origin` is the daemon's own: `http://` plus the request's
/// `Host`. Artifact origins (`<aid>.localhost:<port>`), `null`, and foreign
/// origins are refused with 403 `forbidden_origin`, so a published page cannot
/// comment, send, resolve, or rename the viewer on a person's behalf. A
/// request without `Origin` whose `Sec-Fetch-Site` names another origin
/// (`same-site` or `cross-site`: a page's `<img>` or other no-cors GET, which
/// browsers send without `Origin`) is refused the same way, so a page cannot
/// reach a viewer route's side effects, such as minting a viewer, either.
pub struct SameOrigin;

/// Whether the browser says another origin made the request (`Sec-Fetch-Site`
/// other than `same-origin` or `none`; scripts send no such header).
pub(crate) fn fetched_from_elsewhere(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_some_and(|v| !matches!(v.to_str(), Ok("same-origin" | "none")))
}

impl<S: Send + Sync> FromRequestParts<S> for SameOrigin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, ApiError> {
        let Some(origin) = parts.headers.get(header::ORIGIN) else {
            if fetched_from_elsewhere(&parts.headers) {
                return Err(ApiError::forbidden(
                    "forbidden_origin",
                    "viewer routes accept requests only from the Clax shell's own origin",
                ));
            }
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

/// The comment author for the request: its viewer's display name
/// ([`crate::identity::Identity::viewer`]: the owner's for an owner
/// credential), sanitised, else `Viewer`; and that viewer's public ID.
pub fn author(
    st: &Store,
    who: &crate::identity::Identity,
) -> clax_core::Result<(String, Option<String>)> {
    // The owner always has a row to author with (made for the CLI acting
    // before any browser); a cookie with no row stays anonymous.
    let v = if who.is_owner() {
        who.ensure_viewer(st)?
    } else {
        who.viewer(st)?
    };
    let name = display_name(
        v.as_ref()
            .and_then(|v| v.display_name.as_deref())
            .unwrap_or(""),
    );
    Ok((name, v.map(|v| v.public_id)))
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

    #[test]
    fn a_shadowing_viewer_cookie_never_wins() {
        let (a, b) = ("01J9Z3K4M5N6P7Q8R9S0T1V2W3", "01J9Z3K4M5N6P7Q8R9S0T1V2W4");
        let mut h = HeaderMap::new();
        // A planted cookie with a longer path is sent first.
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "clax_viewer={b}; clax_viewer={a}; clax_viewer=junk"
            ))
            .unwrap(),
        );
        assert_eq!(read(&h), None);
        assert_eq!(read_all(&h), [b, a]);
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("clax_viewer={a}; clax_viewer={a}")).unwrap(),
        );
        assert_eq!(read(&h).as_deref(), Some(a));
    }
}
