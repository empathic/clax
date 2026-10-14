//! Who a request speaks for: the owner of this install, a browser viewer, or
//! no one.
//!
//! The owner is one identity, a viewer row flagged as the owner
//! ([`clax_store::Store::owner_viewer`]). A request is the owner's when it
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
//! - the Clax Chrome extension's credential: the extension gateway
//!   ([`crate::extension::gateway`]) checks it and marks the request it admits
//!   with [`crate::extension::ViaExtension`], which counts as a browser of the
//!   owner's (spec 2026-10-05-chrome-overlay-design L6).
//!
//! The two cookies count only on a request from this machine (a loopback
//! peer naming a literal local host, the rule `GET /api/token` serves by)
//! that the browser does not mark as made from another origin: cookies
//! ignore ports, so another local server's page may send them too.
//!
//! Anyone else is the viewer the `clax_viewer` cookie names (a LAN viewer),
//! or no one.
//!
//! A hosted daemon ([`crate::hosted`]) serves the internet, so no request is
//! from this machine: the owner and events cookies and the `clax_viewer`
//! cookie count for nothing there. A browser is instead the viewer its
//! session cookie signs in as ([`crate::hosted::HostedRequest`]): the owner
//! for a session of the owner's row (a browser of the owner's), else a
//! person's or a guest's viewer, as the `clax_viewer` cookie would name it. An owner credential confers identity; what a request may do
//! still depends on the token (see [`crate::db_caller`]): the owner cookie
//! alone grants no more than a viewer cookie does.

use crate::auth::{Caller, Person};
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use clax_core::audit::{Actor, AgentActor, Via};
use clax_core::model::Viewer;
use clax_core::secret::Secret;
use clax_store::Store;
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

/// The paths the owner cookie is set for: the API, and the artifact pages
/// (`/a/...`), whose bootstrap names the viewer.
pub const OWNER_COOKIE_PATHS: [&str; 2] = ["/api", "/a"];

/// `Set-Cookie` for the owner cookie on `host` and `path` (one of
/// [`OWNER_COOKIE_PATHS`]): host-only, `HttpOnly`, `SameSite=Lax`, five years.
/// A new token makes it worthless; the shell's next token request sets it
/// again.
pub fn set_owner_cookie(host: &str, token: &str, path: &str) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{}={}; Path={path}; Max-Age=157680000; HttpOnly; SameSite=Lax",
        owner_cookie_name(host),
        owner_cookie_value(token)
    ))
    .ok()
}

/// The peer a request came from, from the connection info the daemon serves
/// with; `None` when it is missing (no connection: never local).
pub fn peer_of(extensions: &axum::http::Extensions) -> Option<std::net::SocketAddr> {
    crate::auth::conn_of(extensions).map(|c| c.peer)
}

/// Whether a request comes from this machine: a loopback `peer` and a `Host`
/// that names this machine literally ([`crate::auth::is_local_host`]).
pub fn is_local(headers: &HeaderMap, peer: Option<std::net::SocketAddr>) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    peer.is_some_and(crate::auth::is_loopback) && crate::auth::is_local_host(host)
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

/// Whether `headers` carry this port's owner cookie for `token` (whatever
/// the request's origin; [`Identity::of`] decides whether it counts).
pub fn has_owner_cookie(headers: &HeaderMap, token: &str) -> bool {
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
    /// The events cookie (only the event streams receive it), on a request
    /// from this machine.
    pub events_cookie: bool,
    /// The owner cookie, on a request from this machine.
    pub owner_cookie: bool,
    /// The `clax_viewer` cookie, when it has a cookie's form
    /// ([`crate::viewer::read`]).
    pub cookie: Option<String>,
    /// The request comes from this machine ([`is_local`]).
    pub local: bool,
    /// The extension gateway admitted the request with a live extension
    /// credential ([`crate::extension::ViaExtension`]).
    pub extension: bool,
    /// The request carries an agent credential of another machine
    /// (`Authorization: Clax-Grant`), verified or not: it is never the
    /// owner, a viewer or anonymous.
    pub grant_header: bool,
    /// The grant whose signature [`crate::agent::verify_layer`] verified.
    pub grant: Option<crate::agent::VerifiedGrant>,
    /// `X-Clax-Session`: the session an agent speaks for.
    pub header_session: Option<String>,
    /// `X-Clax-Session-Secret`: the secret proving a grant's session (spec
    /// §6.8); `Err` when the request carries it more than once.
    pub session_secret: Option<Result<Secret, ()>>,
    /// `X-Clax-Channel: agent`; counts only with the token.
    pub agent_marker: bool,
    /// A hosted daemon's session of the owner's row: a browser of the
    /// owner's ([`crate::hosted`]).
    pub hosted_owner: bool,
}

use crate::headers::SESSION_HEADER;
/// The agent-channel marker's header (`agent`).
pub const CHANNEL_HEADER: &str = "x-clax-channel";

impl Identity {
    /// The credentials in `headers`, for a daemon whose token is `token`, on
    /// a request from `peer`.
    pub fn of(headers: &HeaderMap, token: &str, peer: Option<std::net::SocketAddr>) -> Identity {
        let local = is_local(headers, peer);
        let cookies_count = local && !crate::viewer::fetched_from_elsewhere(headers);
        let grant_header = crate::auth::is_grant_request(headers);
        Identity {
            token: !grant_header && crate::auth::has_token(headers, token),
            events_cookie: !grant_header
                && cookies_count
                && crate::auth::has_events_cookie(headers, token),
            owner_cookie: !grant_header && cookies_count && has_owner_cookie(headers, token),
            cookie: (!grant_header)
                .then(|| crate::viewer::read(headers))
                .flatten(),
            local,
            extension: false,
            grant_header,
            grant: None,
            header_session: headers
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            session_secret: session_secret(headers),
            agent_marker: headers
                .get(CHANNEL_HEADER)
                .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"agent")),
            hosted_owner: false,
        }
    }

    /// The credentials of a request with `headers` and `extensions` (its peer,
    /// and whether the extension gateway admitted it), for a daemon whose
    /// token is `token`.
    pub fn from_parts(
        headers: &HeaderMap,
        extensions: &axum::http::Extensions,
        token: &str,
    ) -> Identity {
        let id = Identity::of(headers, token, peer_of(extensions));
        let id = Identity {
            extension: !id.grant_header
                && extensions.get::<crate::extension::ViaExtension>().is_some(),
            grant: extensions.get::<crate::agent::VerifiedGrant>().cloned(),
            ..id
        };
        match extensions.get::<crate::hosted::HostedRequest>() {
            Some(h) => h.identity(id),
            None => id,
        }
    }

    /// The [`Caller`] when finding it needs no store read: the owner, the
    /// owner's agents naming no session, and anonymous callers.
    pub fn quick_caller(&self, path_session: Option<&str>) -> Option<Caller> {
        if self.grant_header {
            return None;
        }
        if self.token {
            return (path_session.is_none() && self.header_session.is_none()).then(|| {
                if self.agent_marker {
                    Caller::OwnerAgent { session: None }
                } else {
                    Caller::Owner {
                        browser: self.owner_browser(),
                    }
                }
            });
        }
        if self.owner_browser() {
            return Some(Caller::Owner { browser: true });
        }
        self.cookie.is_none().then_some(Caller::Anonymous)
    }

    /// Who the request speaks for (spec §6.1, M8). `path_session` is the
    /// session the route's path names (`/api/sessions/{id}/…`), decoded as
    /// the handler reads it.
    ///
    /// - An agent credential is a `PersonAgent` once
    ///   [`crate::agent::verify_layer`] verified it (else 401
    ///   `invalid_signature`); its `X-Clax-Session` and path session must be
    ///   its grant's (400 `unknown_session` for one that does not exist, 403
    ///   `foreign_session` for another credential's), each must be proved
    ///   by its secret in `X-Clax-Session-Secret` (401
    ///   `invalid_session_proof`, spec §6.8; so a path session is the
    ///   header's), a path session needs `X-Clax-Session` (400
    ///   `no_session`), and its rights are that session's, or with no
    ///   session the grant's narrowed by its live sessions' narrowings.
    /// - The token is an `OwnerAgent` when the request names a session or
    ///   carries the agent-channel marker, else the `Owner`; a session it
    ///   names (path or header) that is another credential's is 403
    ///   `foreign_session`; one that does not exist is left to the handler.
    /// - An owner browser credential is the `Owner`; a `clax_viewer` cookie
    ///   is a `Person` or a `Viewer`; anything else is `Anonymous`.
    ///
    /// # Errors
    /// As above, `grant_revoked` for a grant that ended, and the store's.
    pub fn caller(&self, st: &Store, path_session: Option<&str>) -> Result<Caller, ApiError> {
        if let Some(c) = self.quick_caller(path_session) {
            return Ok(c);
        }
        if self.grant_header {
            let Some(g) = &self.grant else {
                return Err(ApiError::new(
                    axum::http::StatusCode::UNAUTHORIZED,
                    "invalid_signature",
                    "the request's signature was not verified",
                ));
            };
            if path_session.is_some() && self.header_session.is_none() {
                return Err(ApiError::bad_request(
                    "no_session",
                    "a session route needs X-Clax-Session naming one of this machine's sessions",
                ));
            }
            for sid in [self.header_session.as_deref(), path_session]
                .into_iter()
                .flatten()
            {
                own_session(st, sid, Some(&g.grant_id))?;
                // The session's own secret, not the machine's key alone
                // (spec §6.8): a sibling agent cannot act as it.
                self.prove_session(st, sid, &g.grant_id)?;
            }
            // The agent acting is the session whose resources the path
            // names; else the one the header names.
            let acting = path_session.or(self.header_session.as_deref());
            // Read first: a change committed meanwhile leaves the rights
            // stale for the next reader, never newer than their label.
            let generation = st.visibility_generation();
            let gc = st.grant_caller(&g.grant_id, acting)?;
            return Ok(Caller::PersonAgent {
                person: gc.person,
                grant: gc.grant_id,
                session: gc.session,
                rights: std::sync::Arc::new(gc.rights),
                generation,
            });
        }
        if self.token {
            for sid in [self.header_session.as_deref(), path_session]
                .into_iter()
                .flatten()
            {
                if st.get_session(sid)?.is_some_and(|s| s.grant_id.is_some()) {
                    return Err(foreign_session());
                }
            }
            let session = path_session
                .map(str::to_string)
                .or_else(|| self.header_session.clone());
            return Ok(if session.is_some() || self.agent_marker {
                Caller::OwnerAgent { session }
            } else {
                Caller::Owner {
                    browser: self.owner_browser(),
                }
            });
        }
        if self.owner_browser() {
            return Ok(Caller::Owner { browser: true });
        }
        let viewer = match &self.cookie {
            Some(c) => st.get_viewer(c)?,
            None => None,
        };
        Ok(match viewer {
            Some(v) if v.person && v.person_revoked_at.is_none() => Caller::Person { viewer: v },
            Some(v) => Caller::Viewer { viewer: v },
            None => Caller::Anonymous,
        })
    }

    /// That the request carries the secret of grant `grant`'s session
    /// `sid` (spec §6.8).
    ///
    /// # Errors
    /// 401 `invalid_session_proof` when `X-Clax-Session-Secret` is missing,
    /// repeated, or not that live session's secret; the store's.
    fn prove_session(&self, st: &Store, sid: &str, grant: &str) -> Result<(), ApiError> {
        let ok = match &self.session_secret {
            Some(Ok(secret)) => st.session_secret_matches(sid, grant, secret)?,
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(clax_store::sessions::invalid_session_proof().into())
        }
    }

    /// Whose a person-scoped route's data is ([`Caller::person`]).
    ///
    /// # Errors
    /// [`Identity::caller`]'s.
    pub fn person(&self, st: &Store) -> Result<Person, ApiError> {
        self.caller(st, None).map(|c| c.person())
    }

    /// Whether the request speaks for the owner: any owner credential. An
    /// agent credential of another machine never does.
    pub fn is_owner(&self) -> bool {
        !self.grant_header
            && (self.token
                || self.events_cookie
                || self.owner_cookie
                || self.extension
                || self.hosted_owner)
    }

    /// 403 `forbidden` ("only the owner `what`") unless the request speaks
    /// for the owner ([`Identity::is_owner`]).
    pub fn require_owner(&self, what: &str) -> Result<(), crate::error::ApiError> {
        if self.is_owner() {
            Ok(())
        } else {
            Err(crate::error::ApiError::forbidden(
                "forbidden",
                format!("only the owner {what}"),
            ))
        }
    }

    /// Whether the request comes from a browser of the owner's: only the
    /// shell's token request hands out the owner and events cookies, and the
    /// extension is a browser of the owner's.
    pub fn owner_browser(&self) -> bool {
        !self.grant_header
            && (self.owner_cookie || self.events_cookie || self.extension || self.hosted_owner)
    }

    /// The viewer the request speaks for: the owner's row for the owner
    /// (made for a browser of the owner's when missing, never for the token
    /// alone), else the cookie's viewer when it has a row.
    pub fn viewer(&self, st: &Store) -> clax_core::Result<Option<Viewer>> {
        if self.owner_browser() {
            return st.owner_viewer(true).map(Some);
        }
        if self.is_owner() {
            return st.owner();
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

    /// Like [`Identity::viewer`] for a request that acts (a comment, a
    /// resolve, a seen mark): the owner's row is made when missing, for the
    /// token alone too (the CLI acting before any browser; see
    /// [`Store::owner_viewer`]); else the viewer the cookie names. A cookie
    /// that names no viewer makes none: only `GET /api/viewers/me` makes a
    /// viewer, with a cookie it mints (spec 2026-10-08 §21.4). `None` without
    /// any credential, or with a cookie that names no viewer.
    pub fn ensure_viewer(&self, st: &Store) -> clax_core::Result<Option<Viewer>> {
        if self.is_owner() {
            return st.owner_viewer(self.owner_browser()).map(Some);
        }
        match &self.cookie {
            Some(c) => st.get_viewer(c),
            None => Ok(None),
        }
    }
}

impl Identity {
    /// The verified grant of an agent credential of another machine.
    pub fn verified_grant(&self) -> Option<&str> {
        self.grant
            .as_ref()
            .filter(|_| self.grant_header)
            .map(|g| g.grant_id.as_str())
    }

    /// Whether the agent side's own headers (`x-clax-via`, `x-clax-git`,
    /// `x-clax-call`) count: on a verified agent channel, the daemon token
    /// or a verified grant, never a browser's.
    pub fn agent_headers_count(&self) -> bool {
        self.token || self.verified_grant().is_some()
    }

    /// Who a request that acts is, as the audit journal records it (audit
    /// spec §5.2), told apart as [`Identity::caller`] tells callers apart:
    ///
    /// - A verified grant's request is that grant's agent: the session
    ///   `session` names when it is the grant's (its actor carries the
    ///   grant and person), else an agent with no session naming the grant,
    ///   its person and the channel. An unverified one is anonymous.
    /// - The token is an agent when it names a known session (the token's,
    ///   not a grant's), or with no known session when the request carries
    ///   the agent-channel marker (`X-Clax-Channel: agent`: the shim, the
    ///   hooks, Pi, and the CLI under a harness) or names the `mcp`
    ///   channel. Such an agent records its channel unless it is `mcp`;
    ///   a token request naming no channel is `cli`'s.
    /// - Otherwise the owner, or the viewer (`person` for a connected
    ///   person, as [`Caller::Person`] counts one)
    ///   [`Identity::viewer`] names, by public ID, its row made when
    ///   missing as [`Identity::ensure_viewer`] makes it; else anonymous.
    pub fn audit_actor(
        &self,
        st: &Store,
        session: Option<&str>,
        channel: Option<Via>,
    ) -> clax_core::Result<Actor> {
        let known = |grant: Option<&str>| -> clax_core::Result<Option<AgentActor>> {
            let Some(sid) = session.filter(|s| clax_core::is_ulid(s)) else {
                return Ok(None);
            };
            Ok(st
                .session_actor(sid)?
                .filter(|a| a.grant_id.as_deref() == grant))
        };
        let sessionless = |channel: Option<Via>| AgentActor {
            channel: Some(channel.unwrap_or(Via::Cli)).filter(|c| *c != Via::Mcp),
            ..AgentActor::default()
        };
        if self.grant_header {
            let Some(gid) = self.verified_grant() else {
                return Ok(Actor::Anonymous);
            };
            if let Some(agent) = known(Some(gid))? {
                return Ok(Actor::Agent(agent));
            }
            let v = st.viewer_of_grant(gid)?;
            return Ok(Actor::Agent(AgentActor {
                grant_id: Some(gid.to_string()),
                person: v.person.then_some(v.public_id),
                ..sessionless(channel)
            }));
        }
        if self.token {
            if let Some(agent) = known(None)? {
                return Ok(Actor::Agent(agent));
            }
            if self.agent_marker || channel == Some(Via::Mcp) {
                return Ok(Actor::Agent(sessionless(channel)));
            }
        }
        let viewer = match self.viewer(st)? {
            Some(v) => Some(v),
            None => self.ensure_viewer(st)?,
        };
        Ok(match viewer {
            Some(v) if self.is_owner() => Actor::Owner {
                public_id: v.public_id,
            },
            Some(v) => Actor::Viewer {
                person: v.person && v.person_revoked_at.is_none(),
                public_id: v.public_id,
                display_name: v.display_name,
            },
            None => Actor::Anonymous,
        })
    }
}

impl FromRequestParts<AppState> for Identity {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        Ok(Identity::from_parts(
            &parts.headers,
            &parts.extensions,
            &state.token,
        ))
    }
}

/// `X-Clax-Session-Secret`'s value (spec §6.8): `Err` when the header is
/// given more than once, or is not text.
fn session_secret(headers: &HeaderMap) -> Option<Result<Secret, ()>> {
    let mut all = headers
        .get_all(clax_core::remote::SESSION_SECRET_HEADER)
        .iter();
    let first = all.next()?;
    if all.next().is_some() {
        return Some(Err(()));
    }
    Some(
        first
            .to_str()
            .map(|v| Secret::new(v.trim().to_string()))
            .map_err(|_| ()),
    )
}

/// That session `sid` is one of credential `grant`'s (M4).
///
/// # Errors
/// 400 `unknown_session` when no session `sid` exists; 403
/// `foreign_session` when it is another credential's.
fn own_session(st: &Store, sid: &str, grant: Option<&str>) -> Result<(), ApiError> {
    match st.get_session(sid)? {
        None => Err(ApiError::bad_request(
            "unknown_session",
            "that session does not exist",
        )),
        Some(s) if s.grant_id.as_deref() == grant => Ok(()),
        Some(_) => Err(foreign_session()),
    }
}

/// 403 `foreign_session`: the request names a session of another
/// credential (M4).
pub fn foreign_session() -> ApiError {
    ApiError::forbidden(
        "foreign_session",
        "that session is not one of this credential's",
    )
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

    const LOOPBACK: Option<std::net::SocketAddr> = Some(std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        50000,
    ));

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
            LOOPBACK,
        );
        assert!(id.is_owner() && id.owner_browser() && !id.token);
        // Another token, another port, or a request from another origin is no owner.
        assert!(
            !Identity::of(
                &headers(&[("host", "localhost:7480"), ("cookie", &ok)]),
                "x",
                LOOPBACK
            )
            .is_owner()
        );
        assert!(
            !Identity::of(
                &headers(&[("host", "localhost:7481"), ("cookie", &ok)]),
                "tok",
                LOOPBACK
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
                "tok",
                LOOPBACK
            )
            .is_owner()
        );
    }

    #[test]
    fn the_owner_and_events_cookies_count_only_from_this_machine() {
        let owner = format!("clax_owner_7480={}", owner_cookie_value("tok"));
        let events = format!(
            "clax_events_7480={}",
            crate::auth::events_cookie_value("tok")
        );
        let lan: Option<std::net::SocketAddr> = Some("192.168.1.20:50000".parse().unwrap());
        for cookie in [&owner, &events] {
            // A LAN peer replaying the cookie, with or without Sec-Fetch-Site.
            for extra in [None, Some(("sec-fetch-site", "same-origin"))] {
                let mut h = headers(&[("host", "localhost:7480"), ("cookie", cookie)]);
                if let Some((k, v)) = extra {
                    h.insert(k, v.parse().unwrap());
                }
                assert!(
                    !Identity::of(&h, "tok", lan).is_owner(),
                    "{cookie} {extra:?}"
                );
            }
            // A loopback peer naming the LAN address as its Host.
            let h = headers(&[("host", "192.168.1.20:7480"), ("cookie", cookie)]);
            assert!(!Identity::of(&h, "tok", LOOPBACK).is_owner(), "{cookie}");
            // No connection info at all.
            let h = headers(&[("host", "localhost:7480"), ("cookie", cookie)]);
            assert!(!Identity::of(&h, "tok", None).is_owner(), "{cookie}");
            assert!(Identity::of(&h, "tok", LOOPBACK).is_owner(), "{cookie}");
        }
    }

    #[test]
    fn the_extension_gateways_mark_is_a_browser_of_the_owners() {
        let h = headers(&[("host", "localhost:7480")]);
        let mut ext = axum::http::Extensions::new();
        assert!(!Identity::from_parts(&h, &ext, "tok").is_owner());
        ext.insert(crate::extension::ViaExtension);
        let id = Identity::from_parts(&h, &ext, "tok");
        assert!(id.extension && id.is_owner() && id.owner_browser() && !id.token);
    }

    #[test]
    fn the_token_is_the_owner_from_anywhere_and_a_viewer_cookie_alone_is_not() {
        let lan: Option<std::net::SocketAddr> = Some("192.168.1.20:50000".parse().unwrap());
        let id = Identity::of(&headers(&[("authorization", "Bearer tok")]), "tok", lan);
        assert!(id.is_owner() && !id.owner_browser());
        let id = Identity::of(
            &headers(&[("cookie", "clax_viewer=01J9Z3K4M5N6P7Q8R9S0T1V2W3")]),
            "tok",
            LOOPBACK,
        );
        assert!(!id.is_owner());
        assert_eq!(id.cookie.as_deref(), Some("01J9Z3K4M5N6P7Q8R9S0T1V2W3"));
    }
}
