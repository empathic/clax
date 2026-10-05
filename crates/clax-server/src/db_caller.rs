//! The caller level of a `db` request (spec §9 "db", §14): the bearer token
//! from no browser (an agent, the CLI, a script; a cookie that names no
//! viewer row is no browser) is `owner`; the bearer token from a browser (the
//! owner shell on localhost) is `admin`; without the token, a viewer with a
//! display name is `interact`; anything else is `view`.
//! `?as_level=view|interact|admin` narrows the level and never raises it. The
//! caller's viewer identity is the public ID of the viewer the request speaks
//! for ([`crate::identity`]: the owner's for an owner's browser), whatever the
//! level.

use crate::auth::has_token;
use crate::error::ApiError;
use crate::identity::Identity;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use clax_core::db::{Caller, Level};
use clax_core::{ArtifactId, CoreError, Store};

/// The value of the first `key=` parameter of `parts`' query string,
/// percent-decoded (`+` is a space). `Some(None)` when the value's encoding
/// is invalid (a malformed `%` escape or bytes that are not UTF-8).
fn query_param(parts: &Parts, key: &str) -> Option<Option<String>> {
    parts
        .uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| percent_decode(v))
}

/// `raw` with `+` as a space and each `%XX` as its byte; `None` when an
/// escape is malformed or the result is not UTF-8.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The level and viewer of a caller that holds the token (`token`: in
/// `Authorization`, or the events cookie on a stream) with the credentials
/// `who`. With the token, a browser (the owner or events cookie, or a viewer
/// cookie that names a viewer row) is `admin` as the owner's viewer; without
/// one (an agent, the CLI, a script) `owner` with no viewer. Without the
/// token, the owner cookie's caller is the owner's viewer and a viewer
/// cookie's its viewer: `interact` when that viewer has a name, else `view`.
fn base_caller(st: &Store, token: bool, who: &Identity) -> clax_core::Result<Caller> {
    let cookie_viewer = match &who.cookie {
        Some(c) => st.get_viewer(c)?,
        None => None,
    };
    if token {
        return Ok(if who.owner_browser() || cookie_viewer.is_some() {
            Caller {
                level: Level::Admin,
                viewer: Some(st.owner_viewer()?.public_id),
            }
        } else {
            Caller {
                level: Level::Owner,
                viewer: None,
            }
        });
    }
    let viewer = if who.is_owner() {
        Some(st.owner_viewer()?)
    } else {
        cookie_viewer
    };
    let level = if viewer.as_ref().is_some_and(|v| v.display_name.is_some()) {
        Level::Interact
    } else {
        Level::View
    };
    Ok(Caller {
        level,
        viewer: viewer.map(|v| v.public_id),
    })
}

/// The level and viewer of a caller that holds the token (`token`) with the
/// credentials `who`, with the rules of the `db` routes (see `base_caller`).
///
/// # Errors
/// When reading the viewer fails.
pub fn caller_of(st: &Store, token: bool, who: &Identity) -> clax_core::Result<Caller> {
    base_caller(st, token, who)
}

/// Who is making a `db` request: whether it carries the bearer token, its
/// viewer cookie, and the `?as_level=` it narrows to (percent-decoded).
/// Rejects an `as_level` other than `view`, `interact`, or `admin`, or one
/// not validly encoded, with 400 `invalid_argument`.
pub struct CallerParts {
    pub token: bool,
    pub who: Identity,
    pub as_level: Option<Level>,
}

impl FromRequestParts<AppState> for CallerParts {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let as_level = match query_param(parts, "as_level") {
            None => None,
            Some(Some(v)) => match v.as_str() {
                "view" => Some(Level::View),
                "interact" => Some(Level::Interact),
                "admin" => Some(Level::Admin),
                _ => {
                    return Err(ApiError::bad_request(
                        "invalid_argument",
                        format!("as_level is view, interact, or admin, not '{v}'"),
                    ));
                }
            },
            Some(None) => {
                return Err(ApiError::bad_request(
                    "invalid_argument",
                    "as_level is not validly percent-encoded",
                ));
            }
        };
        Ok(CallerParts {
            token: has_token(&parts.headers, &state.token),
            who: Identity::of(&parts.headers, &state.token),
            as_level,
        })
    }
}

impl CallerParts {
    /// The caller, looking up the cookie's viewer (a cookie with no viewer row
    /// is no viewer).
    pub fn resolve(&self, st: &Store) -> clax_core::Result<Caller> {
        let base = base_caller(st, self.token, &self.who)?;
        Ok(Caller {
            level: self.as_level.map_or(base.level, |l| l.min(base.level)),
            viewer: base.viewer,
        })
    }

    /// The caller of a request on artifact `id`'s documents, as
    /// [`CallerParts::resolve`]. A caller without the token is refused
    /// [`CoreError::NotDeclared`] when the artifact's current declaration
    /// does not include `db` (a publish or a metadata edit can drop it); the
    /// token keeps access, so agents can seed data before the page that
    /// reads it is published.
    pub fn resolve_for(&self, st: &Store, id: &ArtifactId) -> clax_core::Result<Caller> {
        if !self.token && !st.doc_declared(id)? {
            return Err(CoreError::NotDeclared { capability: "db" });
        }
        self.resolve(st)
    }
}

/// Who is subscribing to `/api/events`, for filtering `doc` events. An
/// `EventSource` cannot send headers, so the token is accepted as `?token=`
/// (percent-decoded) as well as in `Authorization`, and `GET /api/events`
/// also takes the events cookie ([`Subscriber::or_events_cookie`]); a wrong,
/// missing, or invalidly encoded token counts as none.
/// The query string of this route must never be logged.
pub struct Subscriber {
    token: bool,
    who: Identity,
}

impl FromRequestParts<AppState> for Subscriber {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // A value that is not validly encoded is no token.
        let query_token = query_param(parts, "token")
            .flatten()
            .is_some_and(|v| crate::auth::token_matches(&v, &state.token));
        Ok(Subscriber {
            token: query_token || has_token(&parts.headers, &state.token),
            who: Identity::of(&parts.headers, &state.token),
        })
    }
}

impl Subscriber {
    /// This subscriber, holding the token also when `headers` carry the
    /// events cookie for `token` ([`crate::auth::has_events_cookie`]).
    pub fn or_events_cookie(self, headers: &axum::http::HeaderMap, token: &str) -> Self {
        Subscriber {
            token: self.token || crate::auth::has_events_cookie(headers, token),
            who: self.who,
        }
    }

    /// The subscriber's level and viewer, as `base_caller` decides them.
    pub fn resolve(&self, st: &Store) -> clax_core::Result<Caller> {
        base_caller(st, self.token, &self.who)
    }
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn percent_decoding_fails_closed() {
        assert_eq!(percent_decode("%61dmin+x").as_deref(), Some("admin x"));
        for bad in ["%zz", "%6", "%", "%ff", "a%c3"] {
            assert_eq!(percent_decode(bad), None, "{bad}");
        }
    }
}
