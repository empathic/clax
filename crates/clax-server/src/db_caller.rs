//! The caller level of a `db` request (spec §9 "db", §14): the bearer token
//! without a viewer cookie (an agent, the CLI, a script) is `owner`; the
//! bearer token with a viewer cookie (the owner shell on localhost) is
//! `admin`; a cookie naming a viewer with a display name is `interact`;
//! anything else is `view`. `?as_level=view|interact|admin` narrows the level
//! and never raises it. The caller's viewer identity is the cookie's viewer's
//! public ID, whatever the level.

use crate::auth::has_token;
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use clax_core::Store;
use clax_core::db::{Caller, Level};

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

/// The level and viewer of a caller that holds the token (`token`) and/or
/// the viewer cookie `cookie` (a cookie with no viewer row is no viewer).
fn base_caller(st: &Store, token: bool, cookie: Option<&str>) -> clax_core::Result<Caller> {
    let viewer = match cookie {
        Some(c) => st.get_viewer(c)?,
        None => None,
    };
    let level = if token && cookie.is_none() {
        Level::Owner
    } else if token {
        Level::Admin
    } else if viewer.as_ref().is_some_and(|v| v.display_name.is_some()) {
        Level::Interact
    } else {
        Level::View
    };
    Ok(Caller {
        level,
        viewer: viewer.map(|v| v.public_id),
    })
}

/// Who is making a `db` request: whether it carries the bearer token, its
/// viewer cookie, and the `?as_level=` it narrows to (percent-decoded).
/// Rejects an `as_level` other than `view`, `interact`, or `admin`, or one
/// not validly encoded, with 400 `invalid_argument`.
pub struct CallerParts {
    pub token: bool,
    pub cookie: Option<String>,
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
            cookie: crate::viewer::read(&parts.headers),
            as_level,
        })
    }
}

impl CallerParts {
    /// The caller, looking up the cookie's viewer (a cookie with no viewer row
    /// is no viewer).
    pub fn resolve(&self, st: &Store) -> clax_core::Result<Caller> {
        let base = base_caller(st, self.token, self.cookie.as_deref())?;
        Ok(Caller {
            level: self.as_level.map_or(base.level, |l| l.min(base.level)),
            viewer: base.viewer,
        })
    }
}

/// Who is subscribing to `/api/events`, for filtering `doc` events. An
/// `EventSource` cannot send headers, so the token is accepted as `?token=`
/// (percent-decoded) as well as in `Authorization`; a wrong, missing, or
/// invalidly encoded token counts as none.
/// The query string of this route must never be logged.
pub struct Subscriber {
    token: bool,
    cookie: Option<String>,
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
            cookie: crate::viewer::read(&parts.headers),
        })
    }
}

impl Subscriber {
    /// The subscriber's level and viewer: a valid token with a viewer cookie
    /// is `admin` (the owner shell), without one `owner` (an agent, the CLI);
    /// a cookie alone is `interact` for a named viewer, else `view`; neither
    /// is `view`.
    pub fn resolve(&self, st: &Store) -> clax_core::Result<Caller> {
        base_caller(st, self.token, self.cookie.as_deref())
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
