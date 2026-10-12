//! The caller level of a `db` request (spec §9 "db", §14): the bearer token
//! from no browser (an agent, the CLI, a script; a cookie that names no
//! viewer row is no browser) is `owner`; the bearer token from a browser (the
//! owner shell on localhost) is `admin`; without the token, a viewer with a
//! display name is `interact`; anything else is `view`.
//! `?as_level=view|interact|admin` narrows the level and never raises it. The
//! caller's viewer identity is the public ID of the viewer the request speaks
//! for ([`crate::identity`]: the owner's for an owner's browser), whatever the
//! level. A browser's writes are a page's (the `db` capability is the only
//! browser writer of documents), so they add the page's publisher to a
//! document's authors as well as the writer (spec
//! 2026-10-08-multi-agent-multi-person §8.2).
//!
//! A request carrying `X-Clax-Reader: <A>` reads another app's documents on
//! A's behalf (spec 2026-10-12-flutter-app-design §3.8): it is served only
//! when [`Store::may_read`] says `Granted`, at `min(level, view)` with no
//! viewer identity, so neither a rule above `view` nor a private
//! `data/users/<id>/` document is ever readable that way. Writes with the
//! header, and the header on any route but the document reads, are refused
//! before any handler runs ([`refuse_reader_elsewhere`]).

use crate::error::ApiError;
use crate::identity::Identity;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use clax_core::db::{Caller, Level};
use clax_core::visibility::Author;
use clax_core::{ArtifactId, CoreError};
use clax_store::Store;
use clax_store::grants::ReadAccess;

/// The header naming the app a request reads another app's documents for.
pub const READER_HEADER: &str = "x-clax-reader";

/// The artifact `X-Clax-Reader` names; `Ok(None)` without the header.
///
/// # Errors
/// 400 `invalid_args` when the header is repeated or its value is not an
/// artifact ID.
pub fn reader_of(headers: &axum::http::HeaderMap) -> Result<Option<ArtifactId>, ApiError> {
    let mut values = headers.get_all(READER_HEADER).iter();
    let Some(v) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ApiError::bad_request(
            "invalid_args",
            "X-Clax-Reader names one artifact",
        ));
    }
    v.to_str()
        .ok()
        .and_then(|v| ArtifactId::parse(v).ok())
        .map(Some)
        .ok_or_else(|| ApiError::bad_request("invalid_args", "X-Clax-Reader is not an artifact ID"))
}

/// Whether `path` is a route that serves a reader: an artifact's document
/// list (`/api/artifacts/<aid>/docs`), one document
/// (`/api/artifacts/<aid>/docs/<path>`), or `docs:collections` (which
/// refuses it itself). Matched on the raw path, so any other spelling is
/// another route.
fn reader_route(path: &str) -> bool {
    let segs: Vec<&str> = path.split('/').collect();
    match segs.as_slice() {
        ["", "api", "artifacts", aid, "docs", rest @ ..] => {
            !aid.is_empty() && rest.iter().all(|s| !s.is_empty())
        }
        ["", "api", "artifacts", aid, "docs:collections"] => !aid.is_empty(),
        _ => false,
    }
}

/// Refuses a request that carries `X-Clax-Reader` anywhere it is not
/// honoured, before any handler runs: a reader only ever reads, and only
/// another app's documents. With a method other than `GET` or `HEAD`: 403
/// `read_only`. A `GET` or `HEAD` outside the document read routes
/// ([`reader_route`]): 403 `not_granted`, since serving it with the
/// caller's own credentials would ignore the reader. The event stream takes
/// its reader as `?reader=`, and `/api/stream` in its topic, never the
/// header.
pub async fn refuse_reader_elsewhere(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::Method;
    use axum::response::IntoResponse;
    if !req.headers().contains_key(READER_HEADER) {
        return next.run(req).await;
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return ApiError::forbidden(
            "read_only",
            "a request with X-Clax-Reader only reads another app's documents",
        )
        .into_response();
    }
    if !reader_route(req.uri().path()) {
        return ApiError::forbidden(
            "not_granted",
            "X-Clax-Reader is honoured only on an artifact's document reads",
        )
        .into_response();
    }
    next.run(req).await
}

/// The caller `reader` reading `source`'s documents with the credentials
/// that give `level`: `min(level, view)` with no viewer, once
/// [`Store::may_read`] says `Granted` and `source` declares `db`.
///
/// # Errors
/// `NotFound` when either artifact is missing; `not_granted` (403) without
/// consent; [`CoreError::NotDeclared`] when `source` does not declare `db`.
pub fn reader_caller(
    st: &Store,
    reader: &ArtifactId,
    source: &ArtifactId,
    level: Level,
) -> clax_core::Result<Caller> {
    if st.may_read(reader, source)? != ReadAccess::Granted {
        return Err(not_granted(reader, source));
    }
    if !st.doc_declared(source)? {
        return Err(CoreError::NotDeclared { capability: "db" });
    }
    Ok(Caller::new(level.min(Level::View), None))
}

/// Whether app `reader` may read `source`'s documents now (both artifact
/// IDs, as the stream hub holds them), as [`reader_caller`] decides; an
/// error is no.
pub fn reads_now(st: &Store, reader: &str, source: &str) -> bool {
    match (ArtifactId::parse(reader), ArtifactId::parse(source)) {
        (Ok(r), Ok(s)) => reader_caller(st, &r, &s, Level::View).is_ok(),
        _ => false,
    }
}

/// 403 `not_granted`: `reader` may not read `source`'s documents.
pub fn not_granted(reader: &ArtifactId, source: &ArtifactId) -> CoreError {
    CoreError::invalid(
        "not_granted",
        format!("{reader} may not read {source}'s documents"),
    )
}

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
pub fn base_caller(st: &Store, token: bool, who: &Identity) -> clax_core::Result<Caller> {
    let cookie_viewer = who.cookie_viewer(st)?;
    if token {
        return Ok(if who.owner_browser() || cookie_viewer.is_some() {
            Caller {
                level: Level::Admin,
                viewer: Some(st.owner_viewer(true)?.public_id),
                person: Author::Owner,
                via_page: true,
            }
        } else {
            Caller::new(Level::Owner, None)
        });
    }
    let owner = who.is_owner();
    let viewer = if owner {
        who.viewer(st)?
    } else {
        cookie_viewer
    };
    let level = if viewer.as_ref().is_some_and(|v| v.display_name.is_some()) {
        Level::Interact
    } else {
        Level::View
    };
    let viewer = viewer.map(|v| v.public_id);
    Ok(Caller {
        level,
        person: if owner {
            Author::Owner
        } else {
            Author::of_viewer(viewer.as_deref())
        },
        viewer,
        via_page: true,
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
/// viewer cookie, the `?as_level=` it narrows to (percent-decoded), and the
/// app it reads for (`X-Clax-Reader`, [`reader_of`]).
/// Rejects an `as_level` other than `view`, `interact`, or `admin`, or one
/// not validly encoded, with 400 `invalid_argument`, and a reader header
/// that names no artifact with 400 `invalid_args`.
pub struct CallerParts {
    pub token: bool,
    pub who: Identity,
    pub as_level: Option<Level>,
    /// A person's agent: its person (`None` for the owner's row) and
    /// rights, from which its level comes (spec §6.1).
    pub agent: Option<(Option<String>, std::sync::Arc<clax_core::rights::Rights>)>,
    /// The request writes (any method but `GET` and `HEAD`): a person's
    /// agent's level is its rights' for `write`, else for `read`.
    pub write: bool,
    /// It names itself a page's (`x-clax-via: page`, which the shell's `db`
    /// capability sends): its writes add the page's authors.
    pub page: bool,
    pub reader: Option<ArtifactId>,
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
        let who = Identity::from_parts(&parts.headers, &parts.extensions, &state.token);
        let page = parts
            .headers
            .get(crate::headers::VIA_HEADER)
            .and_then(|v| v.to_str().ok())
            == Some("page");
        // A page writes through a browser, never through a grant.
        if page && who.grant_header {
            return Err(crate::agent::invalid_via_page());
        }
        let agent = if who.grant_header {
            match crate::agent::resolve(state, who.clone(), None).await? {
                crate::auth::Caller::PersonAgent { person, rights, .. } => Some((person, rights)),
                _ => return Err(crate::auth::grants_refused()),
            }
        } else {
            None
        };
        Ok(CallerParts {
            token: who.token,
            who,
            as_level,
            agent,
            write: !matches!(
                parts.method,
                axum::http::Method::GET | axum::http::Method::HEAD
            ),
            page,
            reader: reader_of(&parts.headers)?,
        })
    }
}

impl CallerParts {
    /// The caller, looking up the cookie's viewer (a cookie with no viewer row
    /// is no viewer), before any reader clamp.
    fn resolve(&self, st: &Store) -> clax_core::Result<Caller> {
        let base = base_caller(st, self.token, &self.who)?;
        self.narrowed(base)
    }

    /// `base` narrowed by `?as_level=`.
    fn narrowed(&self, base: Caller) -> clax_core::Result<Caller> {
        Ok(Caller {
            level: self.as_level.map_or(base.level, |l| l.min(base.level)),
            via_page: base.via_page || self.page,
            ..base
        })
    }

    /// The caller of a request on artifact `id`'s documents, as
    /// [`CallerParts::resolve`]. A caller without the token is refused
    /// [`CoreError::NotDeclared`] when the artifact's current declaration
    /// does not include `db` (a publish or a metadata edit can drop it); the
    /// token keeps access, so agents can seed data before the page that
    /// reads it is published. With a reader, the caller is
    /// [`reader_caller`]'s, whatever the token.
    ///
    /// A person's agent's level is its rights' `clax-db` level for the
    /// request's action at `id` (`view` when none), its viewer its person's
    /// (the owner's for a grant of the owner's row); it needs the
    /// declaration as a viewer does.
    pub fn resolve_for(&self, st: &Store, id: &ArtifactId) -> clax_core::Result<Caller> {
        if let Some(reader) = &self.reader {
            let level = self.resolve(st)?.level;
            return reader_caller(st, reader, id, level);
        }
        if let Some((person, rights)) = &self.agent {
            if !st.doc_declared(id)? {
                return Err(CoreError::NotDeclared { capability: "db" });
            }
            let own = |x: &str| {
                st.artifact_created_by(x, person.as_deref())
                    .unwrap_or(false)
            };
            let action = if self.write { "write" } else { "read" };
            let level = rights
                .db_level(id.as_str(), action, &own)
                .unwrap_or(Level::View);
            let viewer = match person {
                Some(p) => Some(p.clone()),
                None => st.owner()?.map(|v| v.public_id),
            };
            return self.narrowed(Caller {
                level,
                viewer,
                person: Author::of_token(person.as_deref()),
                via_page: false,
            });
        }
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
        let who = Identity::from_parts(&parts.headers, &parts.extensions, &state.token);
        Ok(Subscriber {
            token: query_token || who.token,
            who,
        })
    }
}

impl Subscriber {
    /// This subscriber, holding the token also when it carries the events
    /// cookie from this machine ([`Identity::events_cookie`]).
    pub fn or_events_cookie(self) -> Self {
        Subscriber {
            token: self.token || self.who.events_cookie,
            who: self.who,
        }
    }

    /// What a host keeps to reopen this subscriber's socket
    /// ([`crate::identity::SocketCredentials::of_request`]), the token
    /// counted when the query carried it.
    pub fn credentials(
        &self,
        token: &str,
        extensions: &axum::http::Extensions,
    ) -> crate::identity::SocketCredentials {
        crate::identity::SocketCredentials {
            token: self
                .token
                .then(|| crate::identity::SocketCredentials::token_tag(token)),
            ..crate::identity::SocketCredentials::of_request(&self.who, token, extensions)
        }
    }

    /// The subscriber's level and viewer, as `base_caller` decides them.
    pub fn resolve(&self, st: &Store) -> clax_core::Result<Caller> {
        base_caller(st, self.token, &self.who)
    }
}

#[cfg(test)]
mod tests {
    use super::{percent_decode, reader_route};

    #[test]
    fn only_the_document_reads_are_reader_routes() {
        for ok in [
            "/api/artifacts/7q3k9mzx2b4t/docs",
            "/api/artifacts/7q3k9mzx2b4t/docs/runs/r1",
            "/api/artifacts/7q3k9mzx2b4t/docs:collections",
        ] {
            assert!(reader_route(ok), "{ok}");
        }
        for no in [
            "/api/artifacts/7q3k9mzx2b4t",
            "/api/artifacts/7q3k9mzx2b4t/",
            "/api/artifacts/7q3k9mzx2b4t/docs/",
            "/api/artifacts/7q3k9mzx2b4t/docs//x",
            "/api/artifacts//docs",
            "/api/artifacts/7q3k9mzx2b4t/do%63s",
            "/api/artifacts/7q3k9mzx2b4t/docs:batch",
            "/api/artifacts/7q3k9mzx2b4t/threads",
            "/api/artifacts/7q3k9mzx2b4t/versions/1/files/docs",
            "/c/7q3k9mzx2b4t/v/1/docs",
            "/api/events",
            "//api/artifacts/7q3k9mzx2b4t/docs",
        ] {
            assert!(!reader_route(no), "{no}");
        }
    }

    #[test]
    fn percent_decoding_fails_closed() {
        assert_eq!(percent_decode("%61dmin+x").as_deref(), Some("admin x"));
        for bad in ["%zz", "%6", "%", "%ff", "a%c3"] {
            assert_eq!(percent_decode(bad), None, "{bad}");
        }
    }
}
