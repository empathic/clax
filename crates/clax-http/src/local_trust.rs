//! What a local daemon trusts a request for: nothing for reaching its port.
//!
//! Owner power on a local daemon needs the daemon token (`<home>/daemon.json`,
//! readable by the owner's OS user alone) or an owner browser session that a
//! token holder opened for a browser (`clax open`, `clax login`): `POST
//! /api/browser-login` makes a one-use login code, and `GET /login` redeems
//! it in the browser, which then holds the session's secret as its owner
//! and events cookies ([`session_cookies`]). No owner credential is derived
//! from, or handed out against, anything a process on the machine can
//! forge: `Sec-Fetch-Site`, `Origin`, `Host` or `X-Clax-Channel`.
//!
//! - [`OwnerSessions`] keeps the live sessions in memory, loaded on start
//!   and changed with every open and end, so checking a cookie reads no
//!   store; the store keeps them across restarts
//!   ([`clax_store::browser_login`]).
//! - [`sessions`] marks a request carrying a live session's cookie (or the
//!   app's key cookie, [`crate::auth::key_cookie_name`]) with
//!   [`OwnerSession`]. [`crate::identity::Identity`] counts the mark as a
//!   browser of the owner's on a request from this machine that the
//!   browser does not say another origin made.
//! - [`gate`] lets a request with no credential reach only what anyone may:
//!   the paths [`open_to_anyone`] names, and a site host's requests (its
//!   access decides each). Anything else answers 401 `no_credential`; a
//!   browser's page load gets [`COLD_BROWSER_PAGE`].
//!
//! A hosted daemon serves the internet and signs browsers in its own way
//! ([`crate::hosted`]): none of this applies there.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use clax_core::secret::{Secret, sha256_hex};
use clax_core::working::Clock;
use clax_store::Store;
use clax_store::browser_login::{OWNER_SESSION_PREFIX, OWNER_SESSION_TTL_SECS};
use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

/// The live owner browser sessions, by their cookie's SHA-256 (lowercase
/// hex), each with its expiry and whether its login code was minted while
/// an agent session was live ([`OwnerSessions::minted_while_agent`]). A
/// panic while the lock is held does not poison later requests: every
/// change inserts or removes one entry.
pub struct OwnerSessions {
    clock: Arc<dyn Clock>,
    map: RwLock<HashMap<String, (DateTime<Utc>, bool)>>,
}

impl OwnerSessions {
    /// No sessions, reading the time from `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> OwnerSessions {
        OwnerSessions {
            clock,
            map: RwLock::default(),
        }
    }

    /// The live sessions `st` holds now, reading the time from its clock.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<OwnerSessions> {
        let s = OwnerSessions::new(st.clock());
        let mut map = s.map.write().unwrap_or_else(PoisonError::into_inner);
        for live in st.live_owner_sessions()? {
            if let Some(at) = parse(&live.expires_at) {
                map.insert(live.sha256, (at, live.minted_while_agent));
            }
        }
        drop(map);
        Ok(s)
    }

    /// Whether the session whose cookie's SHA-256 is `sha256` is live: open,
    /// not ended, and not past its expiry.
    pub fn is_live(&self, sha256: &str) -> bool {
        let now = self.clock.now();
        self.map
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sha256)
            .is_some_and(|(at, _)| *at > now)
    }

    /// Whether the session whose cookie's SHA-256 is `sha256` was opened by
    /// a login code minted while an agent session was live: an agent
    /// holding the token could have minted it for a browser it drives, so
    /// such a session confirms no decision ([`crate::confirm`]). False for
    /// a session not held here.
    pub fn minted_while_agent(&self, sha256: &str) -> bool {
        self.map
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sha256)
            .is_some_and(|(_, minted)| *minted)
    }

    /// Opens a session in `st`, marked `minted_while_agent` when its login
    /// code was, and counts it live: its cookie value.
    ///
    /// # Errors
    /// The store's.
    pub fn open(&self, st: &Store, minted_while_agent: bool) -> clax_core::Result<Secret> {
        let (secret, live) = st.open_owner_session(minted_while_agent)?;
        if let Some(at) = parse(&live.expires_at) {
            self.map
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(live.sha256, (at, live.minted_while_agent));
        }
        Ok(secret)
    }

    /// Ends the session whose cookie's SHA-256 is `sha256`, in `st` and
    /// here: true when a live one ended.
    ///
    /// # Errors
    /// The store's.
    pub fn end(&self, st: &Store, sha256: &str) -> clax_core::Result<bool> {
        let ended = st.end_owner_session(sha256)?;
        self.map
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(sha256);
        Ok(ended)
    }
}

fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Marks a request that carries an owner browser credential: a live
/// session's cookie (`session` is its SHA-256), as the owner cookie
/// (`owner`) or the events cookie (`events`, which stands for the token on
/// the event streams alone) or both; or the app's key cookie
/// ([`OwnerSession::key`], `key`: its value is the token itself) or the
/// in-process app's own frame ([`OwnerSession::browser`]). Only
/// [`sessions`] and the in-process app's request builder insert it.
#[derive(Clone, Debug)]
pub struct OwnerSession {
    pub session: Option<String>,
    pub owner: bool,
    pub events: bool,
    pub key: bool,
}

impl OwnerSession {
    /// The mark of the in-process app's own frame: a browser of the
    /// owner's that holds no session of its own, and that no request from
    /// the network can carry.
    pub fn browser() -> OwnerSession {
        OwnerSession {
            session: None,
            owner: true,
            events: false,
            key: false,
        }
    }

    /// The mark of the app's key cookie, whose value is the daemon token:
    /// anyone holding the token can send it, so it confirms no decision
    /// ([`crate::confirm`]).
    pub fn key() -> OwnerSession {
        OwnerSession {
            key: true,
            ..OwnerSession::browser()
        }
    }
}

/// The values of every cookie named `name` in `headers`.
fn cookie_values<'a>(headers: &'a HeaderMap, name: &'a str) -> impl Iterator<Item = &'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(move |(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The mark of the live session whose secret `headers`' owner or events
/// cookies (named for the `Host`'s port) hold, naming which of the two
/// held it.
pub fn live_session(headers: &HeaderMap, sessions: &OwnerSessions) -> Option<OwnerSession> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let live = |name: &str| {
        cookie_values(headers, name)
            .filter(|v| v.starts_with(OWNER_SESSION_PREFIX))
            .map(sha256_hex)
            .find(|h| sessions.is_live(h))
    };
    let owner = live(&crate::identity::owner_cookie_name(host));
    let events = live(&crate::auth::events_cookie_name(host));
    Some(OwnerSession {
        owner: owner.is_some(),
        events: events.is_some(),
        session: Some(owner.or(events)?),
        key: false,
    })
}

/// Middleware of a local daemon: marks a request carrying a live owner
/// browser session's cookie ([`live_session`]) or the key cookie with
/// [`OwnerSession`]. A request from the network carries no mark of its
/// own (marks are set in process); the in-process app's frames come
/// marked. A hosted daemon's requests pass unmarked.
pub async fn sessions(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    if s.hosted.is_none() {
        let mark = live_session(req.headers(), &s.owner_sessions).or_else(|| {
            crate::auth::has_key_cookie(req.headers(), &s.token).then(OwnerSession::key)
        });
        if let Some(m) = mark {
            req.extensions_mut().insert(m);
        }
    }
    next.run(req).await
}

/// Whether a request's owner browser session mark ([`OwnerSession`])
/// stands for the owner's browser: the request names no agent credential
/// of another machine, comes from this machine
/// ([`crate::identity::from_this_machine`]), and was not fetched by
/// another site's page ([`crate::viewer::fetched_from_elsewhere`]: an
/// artifact's `<id>.localhost` page or another localhost port, which send
/// the `SameSite=Lax` owner cookie as the same site), nor names an
/// `Origin` other than the daemon's own. [`gate`] and
/// [`crate::identity::Identity::from_parts`] both apply it, so a mark that
/// does not count is no credential at all.
pub fn session_counts(headers: &HeaderMap, extensions: &axum::http::Extensions) -> bool {
    !crate::auth::is_grant_request(headers)
        && crate::identity::from_this_machine(headers, crate::auth::conn_of(extensions))
        && !crate::viewer::fetched_from_elsewhere(headers)
        && from_own_origin(headers)
}

/// Whether a request names no `Origin`, or the daemon's own
/// (`http://<Host>`): a browser too old to send `Sec-Fetch-Site` still
/// sends another page's origin with its cross-origin requests.
fn from_own_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    match (origin.to_str().ok(), host) {
        (Some(o), Some(h)) => o.strip_prefix("http://") == Some(h),
        _ => false,
    }
}

/// The owner cookie's paths: every path of the daemon's host, since a
/// browser that holds no credential gets none of its pages ([`gate`]): the
/// shell's pages, the API, and the login route ([`LOGIN_PATH`]), where a
/// browser that already holds a session keeps it when it logs in again.
pub const OWNER_COOKIE_PATHS: [&str; 1] = ["/"];

/// The events cookie's paths: the event streams (`/api/stream` covers its
/// subscription route).
pub const EVENTS_COOKIE_PATHS: [&str; 2] = ["/api/events", "/api/stream"];

/// `Set-Cookie` values that give a browser on `host` the owner browser
/// session `session`: the owner cookie ([`crate::identity::owner_cookie_name`])
/// on [`OWNER_COOKIE_PATHS`], `SameSite=Lax`, and the events cookie
/// ([`crate::auth::events_cookie_name`]) on [`EVENTS_COOKIE_PATHS`],
/// `SameSite=Strict`; each host-only, `HttpOnly`, for as long as the
/// session lasts ([`OWNER_SESSION_TTL_SECS`]).
pub fn session_cookies(host: &str, session: &Secret) -> Vec<HeaderValue> {
    let owner = crate::identity::owner_cookie_name(host);
    let events = crate::auth::events_cookie_name(host);
    let v = session.expose();
    let age = OWNER_SESSION_TTL_SECS;
    OWNER_COOKIE_PATHS
        .iter()
        .map(|p| format!("{owner}={v}; Path={p}; Max-Age={age}; HttpOnly; SameSite=Lax"))
        .chain(
            EVENTS_COOKIE_PATHS.iter().map(|p| {
                format!("{events}={v}; Path={p}; Max-Age={age}; HttpOnly; SameSite=Strict")
            }),
        )
        .filter_map(|c| HeaderValue::from_str(&c).ok())
        .collect()
}

/// The path a browser redeems a login code at: `GET /login?code=&next=`.
pub const LOGIN_PATH: &str = "/login";

/// Whether `next`, where a login sends its browser, is a path of this
/// daemon: it starts with one `/`, holds no `\` and no control character,
/// and is at most 2048 bytes. Anything else could send the browser to
/// another origin.
pub fn local_path(next: &str) -> bool {
    next.len() <= 2048
        && next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && !next.chars().any(char::is_control)
}

/// The paths a local daemon serves a request with no credential on its
/// own host: [`crate::auth::open_without_credential`]'s, the login route
/// ([`LOGIN_PATH`]), and the invite and connect flows, which carry codes
/// and signatures of their own: `/connect/…`, `/join/…`, and
/// `/api/grants…`.
pub fn open_to_anyone(path: &str) -> bool {
    crate::auth::open_without_credential(path)
        || path == LOGIN_PATH
        || path.starts_with("/connect/")
        || path.starts_with("/join/")
        || path == "/api/grants"
        || path.starts_with("/api/grants/")
}

/// Whether a request's `Authorization` names a credential the layers
/// after [`gate`] check: the daemon token, an agent credential of another
/// machine (`Clax-Grant`, `Bearer cxa_…`), or the extension's
/// (`Clax-Extension`, which [`crate::extension::gateway`] checks).
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    crate::auth::has_token(headers, token)
        || crate::auth::is_grant_request(headers)
        || crate::auth::has_scheme(headers, crate::extension::SCHEME)
}

/// Whether `req` is one of the routes that take the token as `?token=`
/// (percent-decoded), since an `EventSource` or a WebSocket sends no
/// headers: `GET /api/events` and an artifact's room socket
/// (`/api/artifacts/<aid>/room`, [`crate::db_caller::Subscriber`]), and
/// carries it.
fn query_token(req: &Request, token: &str) -> bool {
    let segs: Vec<&str> = req.uri().path().split('/').collect();
    let takes = matches!(
        segs.as_slice(),
        ["", "api", "events"] | ["", "api", "artifacts", _, "room"]
    );
    takes
        && url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
            .any(|(k, v)| k == "token" && crate::auth::token_matches(&v, token))
}

/// The page a browser gets for a page load with no credential on a local
/// daemon: how to open Clax from the terminal.
pub const COLD_BROWSER_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Open Clax</title>
<style>
:root { color-scheme: light dark; --bg: #faf9f7; --fg: #1f1e1c; --muted: #6b6862; --code: #efece6; }
@media (prefers-color-scheme: dark) { :root { --bg: #1b1a18; --fg: #ecebe8; --muted: #a29f98; --code: #2a2926; } }
body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: var(--bg); color: var(--fg);
  font: 16px/1.5 system-ui, -apple-system, sans-serif; padding: 0 16px; }
main { max-width: 32rem; }
h1 { font-size: 1.25rem; margin: 0 0 .5rem; }
p { margin: .5rem 0; color: var(--muted); }
code { background: var(--code); color: var(--fg); padding: .1rem .35rem; border-radius: .25rem; }
</style>
</head>
<body>
<main>
<h1>Open Clax from your terminal</h1>
<p>Run <code>clax open</code> (or <code>clax login</code> for a link).</p>
<p>This browser is not signed in to this machine's Clax yet. Opening it from the terminal signs it in once.</p>
</main>
</body>
</html>
"#;

/// The refusal [`gate`] answers a request with no credential: 401
/// `no_credential`; for a browser's page load, [`COLD_BROWSER_PAGE`].
fn refuse(method: &Method, path: &str, headers: &HeaderMap) -> Response {
    let page_load = matches!(*method, Method::GET | Method::HEAD)
        && path != "/api"
        && !path.starts_with("/api/")
        && path != "/mcp"
        && !path.starts_with("/mcp/")
        && headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"));
    if page_load {
        return (
            StatusCode::UNAUTHORIZED,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                // No page frames it, as none frames the shell.
                (header::X_FRAME_OPTIONS, "DENY"),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
                ),
            ],
            COLD_BROWSER_PAGE,
        )
            .into_response();
    }
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "no_credential",
        "this needs a credential: open Clax from the terminal (`clax open`, or `clax login` for a link)",
    )
    .into_response()
}

/// Middleware of a local daemon, before every route: a request that
/// carries no credential reaches only [`open_to_anyone`]'s paths and a
/// site host's routes. A credential is an owner browser session
/// ([`OwnerSession`]) that counts for the request ([`session_counts`]), an
/// `Authorization` the later layers check (the token, an agent credential
/// of another machine with its `Clax-Signature`, the extension's), the
/// token as `?token=` where a route takes it ([`query_token`]), or a
/// `clax_viewer` cookie
/// naming a viewer the daemon minted (a connected person's among them). A
/// request from the extension's origin passes to the extension gateway,
/// which admits it only with a live credential.
/// Anything else is refused ([`refuse`]): reaching the port alone makes
/// no viewer, writes no comment, and sends nothing to an agent. A hosted
/// daemon's requests pass: its own gate decides ([`crate::hosted::gate`]).
pub async fn gate(State(s): State<AppState>, req: Request, next: Next) -> Response {
    if s.hosted.is_some()
        || req.extensions().get::<crate::host::SiteRequest>().is_some()
        || open_to_anyone(req.uri().path())
        || (req.extensions().get::<OwnerSession>().is_some()
            && session_counts(req.headers(), req.extensions()))
    {
        return next.run(req).await;
    }
    let h = req.headers();
    if h.contains_key(header::AUTHORIZATION) {
        return if authorized(h, &s.token) {
            next.run(req).await
        } else {
            ApiError::unauthorized().into_response()
        };
    }
    // A `Clax-Signature` alone is no credential: the layers after this one
    // verify it only for an agent credential's `Authorization`, which
    // passed above.
    if query_token(&req, &s.token) {
        return next.run(req).await;
    }
    // The extension's origin, its preflights included: the extension
    // gateway decides each, and its refusals carry the origin's CORS
    // headers (`crate::extension::gateway`).
    let extension = clax_core::extension::extension_origin(&s.extension_id);
    if h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(extension.as_str()) {
        return next.run(req).await;
    }
    if let Some(cookie) = crate::viewer::read(h) {
        let named = s
            .store_call(move |st| Ok(st.get_viewer(&cookie)?.is_some()))
            .await;
        match named {
            Ok(true) => return next.run(req).await,
            Ok(false) => {}
            Err(e) => return e.into_response(),
        }
    }
    refuse(req.method(), req.uri().path(), req.headers())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::working::ManualClock;

    #[test]
    fn a_next_path_is_this_daemons_alone() {
        for ok in ["/", "/a/7q3k9mzx2b4t", "/a/x?thread=y#z", "/inbox"] {
            assert!(local_path(ok), "{ok}");
        }
        for bad in [
            "",
            "a",
            "//evil.example/",
            "/\\evil.example",
            "https://evil.example/",
            "/a\n",
            "javascript:alert(1)",
        ] {
            assert!(!local_path(bad), "{bad:?}");
        }
        assert!(!local_path(&format!("/{}", "a".repeat(2048))));
    }

    #[test]
    fn only_a_live_sessions_secret_counts_and_never_a_hash_of_the_token() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let sessions = OwnerSessions::new(clock.clone());
        let live = "cxb_live";
        sessions.map.write().unwrap().insert(
            sha256_hex(live),
            (parse("2026-10-30T10:00:00Z").unwrap(), false),
        );
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "localhost:7480".parse().unwrap());
        h.insert(
            header::COOKIE,
            "clax_owner_7480=cxb_other; clax_events_7480=cxb_live"
                .parse()
                .unwrap(),
        );
        let found = live_session(&h, &sessions).unwrap();
        assert_eq!(
            (found.session, found.owner, found.events),
            (Some(sha256_hex(live)), false, true)
        );
        h.insert(header::COOKIE, "clax_owner_7480=cxb_live".parse().unwrap());
        let found = live_session(&h, &sessions).unwrap();
        assert_eq!((found.owner, found.events), (true, false));
        // Another port's cookie names nothing here.
        h.insert(header::HOST, "localhost:7481".parse().unwrap());
        assert!(live_session(&h, &sessions).is_none());
        clock.advance(24 * 3600);
        h.insert(header::HOST, "localhost:7480".parse().unwrap());
        assert!(live_session(&h, &sessions).is_none(), "expired");
    }

    #[test]
    fn the_session_cookies_are_http_only_and_scoped() {
        let s = Secret::new("cxb_x".to_string());
        let c: Vec<String> = session_cookies("localhost:7480", &s)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(c.len(), 3);
        assert!(c[0].starts_with("clax_owner_7480=cxb_x; Path=/;"));
        assert!(c[1].starts_with("clax_events_7480=cxb_x; Path=/api/events;"));
        assert!(c[2].starts_with("clax_events_7480=cxb_x; Path=/api/stream;"));
        assert!(c.iter().all(|c| c.contains("HttpOnly")));
        assert!(c[0].ends_with("SameSite=Lax"));
        assert!(c[1..].iter().all(|c| c.ends_with("SameSite=Strict")));
    }

    #[test]
    fn anyone_reaches_only_the_open_paths() {
        for p in [
            "/healthz",
            "/_clax/shell/app.js",
            "/login",
            "/connect/b/abc",
            "/join/abc",
            "/api/grants",
            "/api/grants/g1/continue",
        ] {
            assert!(open_to_anyone(p), "{p}");
        }
        for p in [
            "/",
            "/a/7q3k9mzx2b4t",
            "/api/token",
            "/api/viewers/me",
            "/api/grantsx",
            "/login/x",
            "/mcp",
        ] {
            assert!(!open_to_anyone(p), "{p}");
        }
    }
}
