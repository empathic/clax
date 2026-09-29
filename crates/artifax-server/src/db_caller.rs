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
use artifax_core::Store;
use artifax_core::db::{Caller, Level};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// The value of the first `key=` parameter of `parts`' query string, undecoded.
fn query_param<'a>(parts: &'a Parts, key: &str) -> Option<&'a str> {
    parts
        .uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// The level and viewer of a caller that holds the token (`token`) and/or
/// the viewer cookie `cookie` (a cookie with no viewer row is no viewer).
fn base_caller(st: &Store, token: bool, cookie: Option<&str>) -> artifax_core::Result<Caller> {
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
/// viewer cookie, and the `?as_level=` it narrows to. Rejects an `as_level`
/// other than `view`, `interact`, or `admin` with 400 `invalid_argument`.
pub struct CallerParts {
    pub token: bool,
    pub cookie: Option<String>,
    pub as_level: Option<Level>,
}

impl FromRequestParts<AppState> for CallerParts {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let as_level = query_param(parts, "as_level")
            .map(|v| match v {
                "view" | "interact" | "admin" => Ok(Level::parse(v).expect("a listed level")),
                _ => Err(ApiError::bad_request(
                    "invalid_argument",
                    format!("as_level is view, interact, or admin, not '{v}'"),
                )),
            })
            .transpose()?;
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
    pub fn resolve(&self, st: &Store) -> artifax_core::Result<Caller> {
        let base = base_caller(st, self.token, self.cookie.as_deref())?;
        Ok(Caller {
            level: self.as_level.map_or(base.level, |l| l.min(base.level)),
            viewer: base.viewer,
        })
    }
}

/// Who is subscribing to `/api/events`, for filtering `doc` events. An
/// `EventSource` cannot send headers, so the token is accepted as `?token=`
/// as well as in `Authorization`; a wrong or missing token counts as none.
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
        // The token is 64 hex characters, so the query value needs no decoding.
        let query_token = query_param(parts, "token")
            .is_some_and(|v| crate::auth::token_matches(v, &state.token));
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
    pub fn resolve(&self, st: &Store) -> artifax_core::Result<Caller> {
        base_caller(st, self.token, self.cookie.as_deref())
    }
}
