//! What a local daemon trusts a request for: nothing for reaching its port.
//!
//! Owner power on a local daemon needs the daemon token (`<home>/daemon.json`,
//! readable by the owner's OS user alone) or an owner browser session that a
//! token holder opened for a browser (`clax open`, `clax login`): `POST
//! /api/browser-login` makes a one-use login code, and `GET /login` redeems
//! it in the browser. A session is two secrets: its cookie, which the
//! browser holds as its owner and events cookies ([`session_cookies`]), and
//! its proof, which the login's page writes into the origin's
//! `localStorage` ([`PROOF_STORAGE_KEY`], [`login_page`]) and the shell
//! sends beside the cookie ([`PROOF_HEADER`], or [`PROOF_PARAM`] on the
//! realtime routes, [`takes_proof_param`]). No owner credential is derived
//! from, or handed out against, anything a process on the machine can
//! forge: `Sec-Fetch-Site`, `Origin`, `Host` or `X-Clax-Channel`.
//!
//! - [`OwnerSessions`] keeps the live sessions in memory, loaded on start
//!   and changed with every open and end, so checking a cookie reads no
//!   store; the store keeps them across restarts
//!   ([`clax_store::browser_login`]). Every session ends at once on
//!   `DELETE /api/browser-login` (`clax logout`,
//!   [`OwnerSessions::end_all`]), and so do the event streams and sockets
//!   opened with one ([`OwnerSessions::ended`]).
//! - Cookies ignore ports, `localStorage` does not: a browser sends the
//!   owner cookie to every server on the daemon's host name it is
//!   navigated to, but only the daemon's own origin reads the proof. A
//!   cookie without its proof is no credential except for a page load
//!   ([`gate`]), which the shell's routes serve bare of data.
//! - [`sessions`] marks a request carrying a live session's cookie and its
//!   proof (or the app's key cookie, [`crate::auth::key_cookie_name`]) with
//!   [`OwnerSession`], and one carrying the cookie alone with
//!   [`UnprovenSession`]. [`crate::identity::Identity`] counts an
//!   [`OwnerSession`] as a browser of the owner's on a request from this
//!   machine that the browser does not say another origin made.
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
use clax_store::browser_login::OpenedOwnerSession;
use clax_store::browser_login::{OWNER_SESSION_PREFIX, OWNER_SESSION_TTL_SECS};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// One live session as [`OwnerSessions`] keeps it: its expiry and its
/// proof's SHA-256 (lowercase hex).
struct Live {
    expires: DateTime<Utc>,
    proof_sha256: String,
}

/// The live owner browser sessions, by their cookie's SHA-256 (lowercase
/// hex), each with its expiry and its proof's SHA-256. A panic while a lock
/// is held does not poison later requests: every change inserts or removes
/// entries whole.
///
/// Opening and ending hold one lock across both the store's change and the
/// map's, so the two agree however a login and `clax logout` interleave;
/// checking reads the map alone.
pub struct OwnerSessions {
    clock: Arc<dyn Clock>,
    map: RwLock<HashMap<String, Live>>,
    writes: Mutex<()>,
    /// Counts the ends, so a stream opened with a session hears when one
    /// ended ([`OwnerSessions::ended`]).
    ends: tokio::sync::watch::Sender<u64>,
}

impl OwnerSessions {
    /// No sessions, reading the time from `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> OwnerSessions {
        OwnerSessions {
            clock,
            map: RwLock::default(),
            writes: Mutex::default(),
            ends: tokio::sync::watch::Sender::new(0),
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
                map.insert(
                    live.sha256,
                    Live {
                        expires: at,
                        proof_sha256: live.proof_sha256,
                    },
                );
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
            .is_some_and(|l| l.expires > now)
    }

    /// Whether the session whose cookie's SHA-256 is `sha256` is live and
    /// `proof` is its proof.
    pub fn proves(&self, sha256: &str, proof: &str) -> bool {
        let now = self.clock.now();
        let presented = sha256_hex(proof);
        self.map
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sha256)
            .is_some_and(|l| {
                l.expires > now
                    && clax_core::secret::constant_time_eq(
                        l.proof_sha256.as_bytes(),
                        presented.as_bytes(),
                    )
            })
    }

    fn writing(&self) -> std::sync::MutexGuard<'_, ()> {
        self.writes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn map_mut(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Live>> {
        self.map.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a session in `st` and counts it live: its cookie and its proof.
    /// The sessions past their expiry leave the map here, as they leave the
    /// store ([`Store::open_owner_session`]), so neither grows with logins.
    ///
    /// # Errors
    /// The store's.
    pub fn open(&self, st: &Store) -> clax_core::Result<OpenedOwnerSession> {
        let _w = self.writing();
        let opened = st.open_owner_session()?;
        let now = self.clock.now();
        let mut map = self.map_mut();
        map.retain(|_, l| l.expires > now);
        if let Some(at) = parse(&opened.live.expires_at) {
            map.insert(
                opened.live.sha256.clone(),
                Live {
                    expires: at,
                    proof_sha256: opened.live.proof_sha256.clone(),
                },
            );
        }
        drop(map);
        Ok(opened)
    }

    /// How many sessions the map holds, expired ones not yet dropped among
    /// them.
    pub fn len(&self) -> usize {
        self.map.read().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether the map holds no session.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ends, in `st` and here, every live session whose proof is among
    /// `proofs`, except `keep` (a cookie's SHA-256): how many ended. Only
    /// a request proven with `keep`'s cookie calls it, presenting the
    /// proofs its origin's storage holds ([`login_page`]): those of the
    /// sessions that browser's earlier logins opened, whose cookies it no
    /// longer holds. A session whose proof the caller does not present (any
    /// other browser's) is left alone. The streams opened with an ended
    /// session end ([`OwnerSessions::ended`]).
    ///
    /// # Errors
    /// The store's.
    pub fn retire(&self, st: &Store, keep: &str, proofs: &[&str]) -> clax_core::Result<usize> {
        let _w = self.writing();
        let presented: Vec<String> = proofs.iter().map(|p| sha256_hex(p)).collect();
        let gone: Vec<String> = self
            .map
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(k, l)| {
                k.as_str() != keep
                    && presented.iter().any(|p| {
                        clax_core::secret::constant_time_eq(
                            l.proof_sha256.as_bytes(),
                            p.as_bytes(),
                        )
                    })
            })
            .map(|(k, _)| k.clone())
            .collect();
        let mut ended = 0;
        for k in &gone {
            if st.end_owner_session(k)? {
                ended += 1;
            }
            self.map_mut().remove(k);
        }
        if !gone.is_empty() {
            self.ends.send_modify(|n| *n += 1);
        }
        Ok(ended)
    }

    /// Ends the session whose cookie's SHA-256 is `sha256`, in `st` and
    /// here: true when a live one ended. The streams opened with it end
    /// ([`OwnerSessions::ended`]).
    ///
    /// # Errors
    /// The store's.
    pub fn end(&self, st: &Store, sha256: &str) -> clax_core::Result<bool> {
        let _w = self.writing();
        let ended = st.end_owner_session(sha256)?;
        self.map_mut().remove(sha256);
        self.ends.send_modify(|n| *n += 1);
        Ok(ended)
    }

    /// Ends every owner browser session, in `st` and here: how many live
    /// ones ended. Every browser of the owner's needs a new login code
    /// (`clax open`, `clax login`) after, and every stream opened with a
    /// session ends ([`OwnerSessions::ended`]).
    ///
    /// # Errors
    /// The store's.
    pub fn end_all(&self, st: &Store) -> clax_core::Result<usize> {
        let _w = self.writing();
        let ended = st.end_all_owner_sessions()?;
        self.map_mut().clear();
        self.ends.send_modify(|n| *n += 1);
        Ok(ended)
    }

    /// Resolves once the session whose cookie's SHA-256 is `sha256` has
    /// ended (at once when it is not live now): what an event stream or
    /// socket opened with that session ends on. A session that lapses at
    /// its expiry while a stream is open is not heard here.
    pub fn ended(
        self: &Arc<Self>,
        sha256: String,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        let me = self.clone();
        let mut ends = self.ends.subscribe();
        async move {
            loop {
                if !me.is_live(&sha256) {
                    return;
                }
                if ends.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}

/// What a stream or socket opened by the request whose `extensions` are
/// given ends on besides its own ends: the end of the owner browser session
/// it was opened with ([`OwnerSessions::ended`]), or never for one opened
/// without.
pub fn session_ended(
    sessions: &Arc<OwnerSessions>,
    extensions: &axum::http::Extensions,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    match extensions
        .get::<OwnerSession>()
        .and_then(|m| m.session.clone())
    {
        Some(hash) => Box::pin(sessions.ended(hash)),
        None => Box::pin(std::future::pending()),
    }
}

fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Marks a request that carries an owner browser credential: a live
/// session's cookie (`session` is its SHA-256) with that session's proof
/// ([`PROOF_HEADER`], [`PROOF_PARAM`]), the cookie as the owner cookie
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

/// Marks a request that carries a live owner browser session's cookie
/// without that session's proof (`session` is the cookie's SHA-256): no
/// credential, except that [`gate`] lets a browser's page load through
/// (the shell's routes serve it bare of data).
#[derive(Clone, Debug)]
pub struct UnprovenSession {
    pub session: String,
}

/// The header the shell sends an owner browser session's proof in.
pub const PROOF_HEADER: &str = "x-clax-owner-proof";

/// The query parameter that carries an owner browser session's proof on
/// the routes [`takes_proof_param`] names, whose clients (a WebSocket, an
/// `EventSource`) send no header of their own.
pub const PROOF_PARAM: &str = "owner_proof";

/// The `localStorage` key the proofs are kept under, on the daemon's
/// origin: the login's page ([`login_page`]) keeps each under a key of its
/// own, this one followed by `.` and a label of that proof
/// ([`proof_storage_key`]), so two logins at once never overwrite each
/// other's. `web/shell/src/owner-proof.ts` reads this key and every key
/// it begins, and presents what they hold ([`presented`]).
pub const PROOF_STORAGE_KEY: &str = "clax.owner_proof";

/// The `localStorage` key the login's page keeps `proof` under:
/// [`PROOF_STORAGE_KEY`], `.`, and the first 16 hex digits of the proof's
/// SHA-256, which tell the proofs apart and give nothing of them away.
pub fn proof_storage_key(proof: &str) -> String {
    format!("{PROOF_STORAGE_KEY}.{}", &sha256_hex(proof)[..16])
}

/// The path the login's page retires the browser's superseded sessions at
/// (`POST`, proven with the new session: [`OwnerSessions::retire`]).
pub const RETIRE_PATH: &str = "/api/browser-login/retire";

/// Whether a `GET` of `path` may carry the proof as [`PROOF_PARAM`]: the
/// event streams (`/api/events`, `/api/stream`) and an artifact's room and
/// live sockets (`/api/artifacts/<id>/room`, `/api/artifacts/<id>/live`).
pub fn takes_proof_param(method: &Method, path: &str) -> bool {
    let segs: Vec<&str> = path.split('/').collect();
    *method == Method::GET
        && matches!(
            segs.as_slice(),
            ["", "api", "events" | "stream"] | ["", "api", "artifacts", _, "room" | "live"]
        )
}

/// The proof a request carries: [`PROOF_HEADER`], else, where
/// [`takes_proof_param`] allows it, [`PROOF_PARAM`] in `uri`'s query.
fn proof_of(method: &Method, uri: &axum::http::Uri, headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(PROOF_HEADER).and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    if !takes_proof_param(method, uri.path()) {
        return None;
    }
    url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .find(|(k, _)| k == PROOF_PARAM)
        .map(|(_, v)| v.into_owned())
}

/// How many proofs one request may present ([`presented`]).
pub const MAX_PROOFS: usize = 16;

/// The proofs a [`PROOF_HEADER`] or [`PROOF_PARAM`] value presents: its
/// comma-separated entries, trimmed, at most [`MAX_PROOFS`], each at its
/// place in the value (an empty one proves nothing). A browser presents
/// every proof its origin's storage holds, since its script cannot read
/// which session its `HttpOnly` cookie names: two logins at once can leave
/// it with one login's cookie and both logins' proofs ([`login_page`]).
pub fn presented(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(',').map(str::trim).take(MAX_PROOFS)
}

/// The live session whose secret `headers`' owner or events cookies (named
/// for the `Host`'s port) hold: `Ok` with its mark, naming which of the two
/// held it, when one of the proofs `proof` presents ([`presented`]) is its
/// proof; `Err` with the unproven mark when none of them is proven.
pub fn live_session(
    headers: &HeaderMap,
    proof: Option<&str>,
    sessions: &OwnerSessions,
) -> Option<Result<OwnerSession, UnprovenSession>> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let hashes = |name: &str| -> Vec<String> {
        cookie_values(headers, name)
            .filter(|v| v.starts_with(OWNER_SESSION_PREFIX))
            .map(sha256_hex)
            .collect()
    };
    let owner = hashes(&crate::identity::owner_cookie_name(host));
    let events = hashes(&crate::auth::events_cookie_name(host));
    let proven = |hs: &[String]| {
        proof.and_then(|raw| {
            hs.iter()
                .find(|h| presented(raw).any(|p| sessions.proves(h, p)))
                .cloned()
        })
    };
    let (owner_proven, events_proven) = (proven(&owner), proven(&events));
    if owner_proven.is_some() || events_proven.is_some() {
        return Some(Ok(OwnerSession {
            owner: owner_proven.is_some(),
            events: events_proven.is_some(),
            session: owner_proven.or(events_proven),
            key: false,
        }));
    }
    let live = |hs: &[String]| hs.iter().find(|h| sessions.is_live(h)).cloned();
    live(&owner)
        .or_else(|| live(&events))
        .map(|session| Err(UnprovenSession { session }))
}

/// Middleware of a local daemon: marks a request carrying a live owner
/// browser session's cookie and its proof ([`live_session`]), or the key
/// cookie, with [`OwnerSession`], and one carrying the cookie alone with
/// [`UnprovenSession`]. A request from the network carries no mark of its
/// own (marks are set in process); the in-process app's frames come
/// marked. A hosted daemon's requests pass unmarked.
pub async fn sessions(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    if s.hosted.is_none() {
        let proof = proof_of(req.method(), req.uri(), req.headers());
        match live_session(req.headers(), proof.as_deref(), &s.owner_sessions) {
            Some(Ok(m)) => {
                req.extensions_mut().insert(m);
            }
            unproven => {
                if crate::auth::has_key_cookie(req.headers(), &s.token) {
                    req.extensions_mut().insert(OwnerSession::key());
                }
                if let Some(Err(u)) = unproven {
                    req.extensions_mut().insert(u);
                }
            }
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
/// browser that already holds a session gets a new one when it logs in again.
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

/// The page a redeemed login answers with (200, beside the session's
/// cookies). Its script:
///
/// 1. writes `proof` into the origin's `localStorage` under its own key
///    ([`proof_storage_key`]), never over another login's;
/// 2. when the storage holds other proofs too ([`PROOF_STORAGE_KEY`] and
///    the keys it begins), presents them all, its own first, to
///    [`RETIRE_PATH`]: the server ends the sessions of every proof but the
///    one that proves the browser's cookie ([`OwnerSessions::retire`]) and
///    names that one's place (`{kept}`), and the script removes the other
///    keys it presented. Two logins at once may leave the browser with
///    either's cookie; whichever it is, its proof is among those kept;
/// 3. replaces itself with `next` (a path of this daemon, [`local_path`]).
///
/// A link to `next` stands in where scripts do not run. `nonce` is the
/// script's, which [`login_page_csp`] names.
pub fn login_page(proof: &str, next: &str, nonce: &str) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "&#39;")
    };
    let js = |s: &str| serde_json::to_string(s).expect("a string serialises");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Opening Clax</title>\
         <meta name=\"clax-login\" data-proof=\"{proof}\" data-key=\"{own}\" data-next=\"{next}\">\
         <style>:root{{color-scheme:light dark}}\
         body{{font:16px/1.5 system-ui,sans-serif;margin:0;padding:2rem 16px;background:Canvas;color:CanvasText}}</style>\
         <script nonce=\"{nonce}\">(function(){{var d=document.querySelector('meta[name=\"clax-login\"]').dataset,\
         K={key},ks=[d.key],ps=[d.proof],s,i,k,v;\
         function go(){{location.replace(d.next)}}\
         try{{s=localStorage;s.setItem(d.key,d.proof);\
         for(i=0;i<s.length;i++){{k=s.key(i);v=k&&s.getItem(k);\
         if(v&&k!==d.key&&(k===K||k.indexOf(K+'.')===0)){{ks.push(k);ps.push(v)}}}}}}catch(e){{}}\
         if(ps.length<2||typeof fetch!=='function'){{go();return}}\
         fetch({retire},{{method:'POST',credentials:'same-origin',cache:'no-store',headers:{{{header}:ps.join(',')}}}})\
         .then(function(r){{return r.ok?r.json():null}})\
         .then(function(a){{if(a&&typeof a.kept==='number')for(i=0;i<ks.length;i++)if(i!==a.kept)s.removeItem(ks[i])}})\
         .catch(function(){{}}).then(go)}})()</script>\
         </head><body><p>Opening Clax… <a href=\"{next}\">Continue</a></p></body></html>",
        proof = esc(proof),
        own = esc(&proof_storage_key(proof)),
        next = esc(next),
        nonce = esc(nonce),
        key = js(PROOF_STORAGE_KEY),
        retire = js(RETIRE_PATH),
        header = js(PROOF_HEADER),
    )
}

/// The `Content-Security-Policy` of [`login_page`] with script nonce
/// `nonce`: its one script, which may reach this origin alone (to
/// [`RETIRE_PATH`]), inline styles, nothing framing it.
pub fn login_page_csp(nonce: &str) -> String {
    format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; connect-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'"
    )
}

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

/// Whether a request is a browser's page load: a `GET` or `HEAD` that
/// accepts `text/html`, of a path outside the API (`/api…`) and `/mcp…`.
pub fn page_load(method: &Method, path: &str, headers: &HeaderMap) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
        && path != "/api"
        && !path.starts_with("/api/")
        && path != "/mcp"
        && !path.starts_with("/mcp/")
        && headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"))
}

/// The refusal [`gate`] answers a request with no credential: 401
/// `no_credential`; for a browser's page load, [`COLD_BROWSER_PAGE`].
fn refuse(method: &Method, path: &str, headers: &HeaderMap) -> Response {
    if page_load(method, path, headers) {
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
/// ([`OwnerSession`]) that counts for the request ([`session_counts`]);
/// for a browser's page load alone ([`page_load`]), a live session's cookie
/// without its proof ([`UnprovenSession`]) on the same terms; an
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
        || (req.extensions().get::<UnprovenSession>().is_some()
            && session_counts(req.headers(), req.extensions())
            && page_load(req.method(), req.uri().path(), req.headers()))
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
            Live {
                expires: parse("2026-10-30T10:00:00Z").unwrap(),
                proof_sha256: sha256_hex("cxp_proof"),
            },
        );
        let proof = Some("cxp_proof");
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "localhost:7480".parse().unwrap());
        h.insert(
            header::COOKIE,
            "clax_owner_7480=cxb_other; clax_events_7480=cxb_live"
                .parse()
                .unwrap(),
        );
        let found = live_session(&h, proof, &sessions).unwrap().unwrap();
        assert_eq!(
            (found.session, found.owner, found.events),
            (Some(sha256_hex(live)), false, true)
        );
        h.insert(header::COOKIE, "clax_owner_7480=cxb_live".parse().unwrap());
        let found = live_session(&h, proof, &sessions).unwrap().unwrap();
        assert_eq!((found.owner, found.events), (true, false));
        // Another port's cookie names nothing here.
        h.insert(header::HOST, "localhost:7481".parse().unwrap());
        assert!(live_session(&h, proof, &sessions).is_none());
        clock.advance(24 * 3600);
        h.insert(header::HOST, "localhost:7480".parse().unwrap());
        assert!(live_session(&h, proof, &sessions).is_none(), "expired");
    }

    #[test]
    fn a_cookie_without_its_proof_is_unproven() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let sessions = OwnerSessions::new(clock);
        sessions.map.write().unwrap().insert(
            sha256_hex("cxb_live"),
            Live {
                expires: parse("2026-10-30T10:00:00Z").unwrap(),
                proof_sha256: sha256_hex("cxp_proof"),
            },
        );
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "localhost:7480".parse().unwrap());
        h.insert(header::COOKIE, "clax_owner_7480=cxb_live".parse().unwrap());
        for proof in [None, Some("cxp_other"), Some(""), Some("cxb_live")] {
            let found = live_session(&h, proof, &sessions).unwrap();
            assert_eq!(
                found.err().map(|u| u.session),
                Some(sha256_hex("cxb_live")),
                "{proof:?}"
            );
        }
        assert!(
            live_session(&h, Some("cxp_proof"), &sessions)
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn the_proof_rides_the_query_on_the_realtime_routes_alone() {
        for (m, p) in [
            (Method::GET, "/api/events"),
            (Method::GET, "/api/stream"),
            (Method::GET, "/api/artifacts/7q3k9mzx2b4t/room"),
            (Method::GET, "/api/artifacts/7q3k9mzx2b4t/live"),
        ] {
            assert!(takes_proof_param(&m, p), "{p}");
        }
        for (m, p) in [
            (Method::POST, "/api/stream"),
            (Method::GET, "/api/token"),
            (Method::GET, "/api/stream/abc"),
            (Method::GET, "/"),
        ] {
            assert!(!takes_proof_param(&m, p), "{m} {p}");
        }
        let h = HeaderMap::new();
        let uri: axum::http::Uri = "/api/events?owner_proof=cxp_q".parse().unwrap();
        assert_eq!(proof_of(&Method::GET, &uri, &h).as_deref(), Some("cxp_q"));
        let uri: axum::http::Uri = "/api/token?owner_proof=cxp_q".parse().unwrap();
        assert_eq!(proof_of(&Method::GET, &uri, &h), None);
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&clax_core::Home::at(dir.path().join("ax"))).unwrap();
        (dir, st)
    }

    /// The sessions the map counts live, and those the store does.
    fn both(sessions: &OwnerSessions, st: &Store) -> (Vec<String>, Vec<String>) {
        let mut map: Vec<String> = sessions.map.read().unwrap().keys().cloned().collect();
        let mut kept: Vec<String> = st
            .live_owner_sessions()
            .unwrap()
            .into_iter()
            .map(|l| l.sha256)
            .collect();
        map.sort();
        kept.sort();
        (map, kept)
    }

    #[test]
    fn a_login_racing_a_logout_leaves_the_store_and_the_map_agreeing() {
        let (_d, st) = store();
        let st = Arc::new(st);
        let sessions = Arc::new(OwnerSessions::load(&st).unwrap());
        for round in 0..200 {
            let opener = {
                let (sessions, st) = (sessions.clone(), st.clone());
                std::thread::spawn(move || sessions.open(&st).unwrap())
            };
            let ender = {
                let (sessions, st) = (sessions.clone(), st.clone());
                std::thread::spawn(move || sessions.end_all(&st).unwrap())
            };
            opener.join().unwrap();
            ender.join().unwrap();
            let (map, kept) = both(&sessions, &st);
            assert_eq!(map, kept, "round {round}");
        }
    }

    #[tokio::test]
    async fn a_stream_hears_its_session_end() {
        let (_d, st) = store();
        let sessions = Arc::new(OwnerSessions::load(&st).unwrap());
        let a = sessions.open(&st).unwrap().live.sha256;
        let b = sessions.open(&st).unwrap().live.sha256;
        let ended_a = sessions.ended(a.clone());
        let ended_b = sessions.ended(b.clone());
        tokio::pin!(ended_a, ended_b);
        assert!(futures::poll!(ended_a.as_mut()).is_pending());
        sessions.end(&st, &a).unwrap();
        ended_a.await;
        assert!(
            futures::poll!(ended_b.as_mut()).is_pending(),
            "another session's end"
        );
        sessions.end_all(&st).unwrap();
        ended_b.await;
        sessions.ended("not-a-session".into()).await;
    }

    #[test]
    fn the_login_page_escapes_what_it_carries() {
        let page = login_page("cxp_a\"b", "/a/x?y=<z>&w", "n0nce");
        assert!(page.contains("data-proof=\"cxp_a&quot;b\""));
        assert!(page.contains("data-next=\"/a/x?y=&lt;z&gt;&amp;w\""));
        assert!(page.contains("<script nonce=\"n0nce\">"));
        assert!(page.contains("\"clax.owner_proof\""));
        assert!(page.contains(&format!("data-key=\"{}\"", proof_storage_key("cxp_a\"b"))));
        assert!(page.contains("\"/api/browser-login/retire\""));
        let csp = login_page_csp("n0nce");
        assert!(csp.contains("script-src 'nonce-n0nce'"));
        assert!(csp.contains("connect-src 'self'"));
    }

    #[test]
    fn each_proof_has_a_storage_key_of_its_own() {
        let a = proof_storage_key("cxp_a");
        assert!(a.starts_with("clax.owner_proof."), "{a}");
        assert_eq!(a.len(), "clax.owner_proof.".len() + 16);
        assert_ne!(a, proof_storage_key("cxp_b"));
        assert!(!a.contains("cxp_a"));
    }

    #[test]
    fn any_proof_presented_proves_its_cookie() {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let sessions = OwnerSessions::new(clock);
        sessions.map.write().unwrap().insert(
            sha256_hex("cxb_live"),
            Live {
                expires: parse("2026-10-30T10:00:00Z").unwrap(),
                proof_sha256: sha256_hex("cxp_proof"),
            },
        );
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "localhost:7480".parse().unwrap());
        h.insert(header::COOKIE, "clax_owner_7480=cxb_live".parse().unwrap());
        // Two logins at once: this browser kept one's cookie and holds
        // both proofs, in either order.
        for raw in ["cxp_other,cxp_proof", "cxp_proof, cxp_other", ",cxp_proof"] {
            assert!(live_session(&h, Some(raw), &sessions).unwrap().is_ok(), "{raw}");
        }
        assert!(live_session(&h, Some("cxp_other,cxp_x"), &sessions).unwrap().is_err());
        // Past MAX_PROOFS, nothing more is read.
        let many = format!("{}cxp_proof", "cxp_x,".repeat(MAX_PROOFS));
        assert!(live_session(&h, Some(&many), &sessions).unwrap().is_err());
        assert_eq!(presented("a, b,,c").collect::<Vec<_>>(), ["a", "b", "", "c"]);
    }

    #[test]
    fn retiring_ends_the_presented_sessions_but_the_callers() {
        let (_d, st) = store();
        let sessions = OwnerSessions::load(&st).unwrap();
        let old = sessions.open(&st).unwrap();
        let racing = sessions.open(&st).unwrap();
        let now = sessions.open(&st).unwrap();
        let other_browser = sessions.open(&st).unwrap();
        let ended = sessions
            .retire(
                &st,
                &now.live.sha256,
                &[now.proof.expose(), old.proof.expose(), racing.proof.expose(), "cxp_gone"],
            )
            .unwrap();
        assert_eq!(ended, 2);
        assert!(sessions.is_live(&now.live.sha256));
        assert!(sessions.is_live(&other_browser.live.sha256), "a proof not presented");
        assert!(!sessions.is_live(&old.live.sha256));
        assert!(!sessions.is_live(&racing.live.sha256));
        let (map, kept) = both(&sessions, &st);
        assert_eq!(map, kept);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn an_expired_session_leaves_the_map_at_the_next_login() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let st = Store::open_with_clock(&clax_core::Home::at(dir.path().join("ax")), clock.clone())
            .unwrap();
        let sessions = OwnerSessions::load(&st).unwrap();
        let first = sessions.open(&st).unwrap();
        assert_eq!(sessions.len(), 1);
        clock.advance(OWNER_SESSION_TTL_SECS + 1);
        let second = sessions.open(&st).unwrap();
        assert_eq!(sessions.len(), 1, "the expired one left");
        assert!(!sessions.is_live(&first.live.sha256));
        assert!(sessions.is_live(&second.live.sha256));
        assert_eq!(both(&sessions, &st), (vec![second.live.sha256.clone()], vec![second.live.sha256]));
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
