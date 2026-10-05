//! Who a request speaks for: the owner of this install, a browser viewer, or
//! no one.
//!
//! The owner is one identity, a viewer row flagged as the owner
//! ([`clax_core::Store::owner_viewer`]). A request is the owner's when it
//! carries an owner credential; [`Identity::of`] is the one place that maps
//! credentials to the owner, so a new kind of credential (a browser
//! extension's, say) is added there and nowhere else. The credentials today:
//!
//! - the bearer token (the CLI, scripts, agents, and the owner's shell);
//! - the events cookie ([`crate::auth::events_cookie_name`]), sent only to the
//!   event streams;
//! - the owner cookie ([`owner_cookie_name`]): a hash of the token, set with
//!   the events cookie when the shell on this machine fetches the token, so
//!   every browser of the owner's is the owner on every route without the
//!   token.
//!
//! Anyone else is the viewer the `clax_viewer` cookie names (a LAN viewer),
//! or no one. An owner credential confers identity; what a request may do
//! still depends on the token (see [`crate::db_caller`]): the owner cookie
//! alone grants no more than a viewer cookie does.

use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use clax_core::Store;
use clax_core::model::Viewer;
use std::convert::Infallible;

/// The owner cookie's name: `clax_owner_<port>`, the port from the request's
/// `Host` (80 when it names none), like the events cookie.
pub fn owner_cookie_name(host: &str) -> String {
    crate::auth::events_cookie_name(host).replacen("clax_events_", "clax_owner_", 1)
}

/// The owner cookie's value for `token`: a SHA-256 of it (lowercase hex),
/// under a label of its own, so it is neither the token nor the events cookie.
pub fn owner_cookie_value(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::new()
        .chain_update(b"clax owner cookie\n")
        .chain_update(token.as_bytes())
        .finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `Set-Cookie` for the owner cookie on `host`: host-only, `Path=/` (the
/// shell's pages read it too), `HttpOnly`, `SameSite=Lax`, five years. A new
/// token makes it worthless; the shell's next token request sets it again.
pub fn set_owner_cookie(host: &str, token: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{}={}; Path=/; Max-Age=157680000; HttpOnly; SameSite=Lax",
        owner_cookie_name(host),
        owner_cookie_value(token)
    ))
    .ok()
}

/// `Set-Cookie` that removes the `clax_viewer` cookie, once the viewer it
/// named has been claimed for the owner.
pub fn clear_viewer_cookie() -> HeaderValue {
    HeaderValue::from_static("clax_viewer=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
}

/// The value of cookie `name` in `headers` equals `want` (constant time).
fn has_cookie(headers: &HeaderMap, name: &str, want: &str) -> bool {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == name)
        .any(|(_, v)| crate::auth::token_matches(v, want))
}

/// Whether `headers` carry this port's owner cookie for `token`. Cookies
/// ignore ports, so it counts only on a request the browser does not mark as
/// made from another origin (`Sec-Fetch-Site`).
pub fn has_owner_cookie(headers: &HeaderMap, token: &str) -> bool {
    if crate::viewer::fetched_from_elsewhere(headers) {
        return false;
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    has_cookie(
        headers,
        &owner_cookie_name(host),
        &owner_cookie_value(token),
    )
}

/// The credentials a request carries.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// The bearer token in `Authorization`.
    pub token: bool,
    /// The events cookie (only the event streams receive it).
    pub events_cookie: bool,
    /// The owner cookie.
    pub owner_cookie: bool,
    /// The `clax_viewer` cookie, when it is a ULID.
    pub cookie: Option<String>,
}

impl Identity {
    /// The credentials in `headers`, for a daemon whose token is `token`.
    pub fn of(headers: &HeaderMap, token: &str) -> Identity {
        Identity {
            token: crate::auth::has_token(headers, token),
            events_cookie: !crate::viewer::fetched_from_elsewhere(headers)
                && crate::auth::has_events_cookie(headers, token),
            owner_cookie: has_owner_cookie(headers, token),
            cookie: crate::viewer::read(headers),
        }
    }

    /// Whether the request speaks for the owner: any owner credential.
    pub fn is_owner(&self) -> bool {
        self.token || self.events_cookie || self.owner_cookie
    }

    /// Whether the request comes from a browser of the owner's: only the
    /// shell's token request hands out the owner and events cookies.
    pub fn owner_browser(&self) -> bool {
        self.owner_cookie || self.events_cookie
    }

    /// The viewer the request speaks for: the owner's row for the owner,
    /// else the cookie's viewer when it has a row.
    pub fn viewer(&self, st: &Store) -> clax_core::Result<Option<Viewer>> {
        if self.is_owner() {
            return st.owner_viewer().map(Some);
        }
        match &self.cookie {
            Some(c) => st.get_viewer(c),
            None => Ok(None),
        }
    }

    /// [`Identity::viewer`] for a browser's request (the owner or events
    /// cookie, or a viewer cookie); `None` for the token alone (an agent, the
    /// CLI), which reads data as no viewer.
    pub fn browser_viewer(&self, st: &Store) -> clax_core::Result<Option<Viewer>> {
        if self.owner_browser() || self.cookie.is_some() {
            self.viewer(st)
        } else {
            Ok(None)
        }
    }

    /// Like [`Identity::viewer`], creating the cookie's viewer row when it has
    /// none yet; `None` only without any credential or cookie.
    pub fn ensure_viewer(&self, st: &Store) -> clax_core::Result<Option<Viewer>> {
        if self.is_owner() {
            return st.owner_viewer().map(Some);
        }
        match &self.cookie {
            Some(c) => st.upsert_viewer(c, None).map(Some),
            None => Ok(None),
        }
    }
}

impl FromRequestParts<AppState> for Identity {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        Ok(Identity::of(&parts.headers, &state.token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn the_owner_cookie_is_named_for_the_port_and_never_holds_the_token() {
        assert_eq!(owner_cookie_name("localhost:7480"), "clax_owner_7480");
        assert_eq!(owner_cookie_name("[::1]"), "clax_owner_80");
        let v = owner_cookie_value("tok");
        assert_eq!(v.len(), 64);
        assert_ne!(v, crate::auth::events_cookie_value("tok"));
        let ok = format!("clax_owner_7480={v}");
        let id = Identity::of(
            &headers(&[("host", "localhost:7480"), ("cookie", &ok)]),
            "tok",
        );
        assert!(id.is_owner() && id.owner_browser() && !id.token);
        // Another token, another port, or a request from another origin is no owner.
        assert!(
            !Identity::of(
                &headers(&[("host", "localhost:7480"), ("cookie", &ok)]),
                "x"
            )
            .is_owner()
        );
        assert!(
            !Identity::of(
                &headers(&[("host", "localhost:7481"), ("cookie", &ok)]),
                "tok"
            )
            .is_owner()
        );
        assert!(
            !Identity::of(
                &headers(&[
                    ("host", "localhost:7480"),
                    ("cookie", &ok),
                    ("sec-fetch-site", "same-site")
                ]),
                "tok"
            )
            .is_owner()
        );
    }

    #[test]
    fn the_token_is_the_owner_and_a_viewer_cookie_alone_is_not() {
        let id = Identity::of(&headers(&[("authorization", "Bearer tok")]), "tok");
        assert!(id.is_owner() && !id.owner_browser());
        let id = Identity::of(
            &headers(&[("cookie", "clax_viewer=01J9Z3K4M5N6P7Q8R9S0T1V2W3")]),
            "tok",
        );
        assert!(!id.is_owner());
        assert_eq!(id.cookie.as_deref(), Some("01J9Z3K4M5N6P7Q8R9S0T1V2W3"));
    }
}
