//! The extension's credentials on the daemon (spec
//! 2026-10-05-chrome-overlay-design §5.3, §10): an in-memory map from each
//! live credential's hash to its extension ID and last use, loaded on start
//! and reloaded after every mint and revoke, so checking a credential reads
//! no store. A credential unused for [`CREDENTIAL_TTL_DAYS`] stops being
//! accepted while the daemon runs, as it would after a restart. A live
//! credential is the owner identity (spec L6); it names no viewer.
//!
//! Also the extension gateway ([`gateway`], spec L5, §9.2, §10 item 6), the
//! one way a request from the extension's origin reaches a handler.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use clax_core::Store;
use clax_core::extension::{CREDENTIAL_TTL_DAYS, credential_hash, extension_origin, is_credential};
use clax_core::working::{Clock, SystemClock};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// How often one credential's `last_used_at` is written.
const TOUCH_EVERY: Duration = Duration::hours(1);

/// What a live credential stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cred {
    pub extension_id: String,
}

struct Entry {
    cred: Cred,
    /// The last use the store holds (or this daemon is about to write).
    last_used: DateTime<Utc>,
}

/// The live credentials, by hash. A panic while a lock is held does not
/// poison later requests: every access takes the map as it was left (each
/// change replaces or edits one entry, or the whole map, at once).
pub struct Credentials {
    clock: Arc<dyn Clock>,
    map: RwLock<HashMap<String, Entry>>,
    /// Held across a store change and the reload after it, so two changes
    /// never replace the map out of order.
    refreshing: Mutex<()>,
}

impl Credentials {
    /// No credentials, reading the time from `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Credentials {
        Credentials {
            clock,
            map: RwLock::default(),
            refreshing: Mutex::new(()),
        }
    }

    /// The live credentials the store holds now, on the system clock.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<Credentials> {
        Self::load_with(st, Arc::new(SystemClock))
    }

    /// The live credentials the store holds now, reading the time from `clock`.
    ///
    /// # Errors
    /// The store's.
    pub fn load_with(st: &Store, clock: Arc<dyn Clock>) -> clax_core::Result<Credentials> {
        let c = Credentials::new(clock);
        *c.map.write().unwrap_or_else(PoisonError::into_inner) = read_map(st)?;
        Ok(c)
    }

    /// The credential whose hash is `hash`, while it is live. One unused for
    /// [`CREDENTIAL_TTL_DAYS`] is dropped and answers `None`.
    pub fn get(&self, hash: &str) -> Option<Cred> {
        let now = self.clock.now();
        {
            let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
            match map.get(hash) {
                None => return None,
                Some(e) if live(e, now) => return Some(e.cred.clone()),
                Some(_) => {}
            }
        }
        let mut map = self.map.write().unwrap_or_else(PoisonError::into_inner);
        if map.get(hash).is_some_and(|e| !live(e, now)) {
            map.remove(hash);
        }
        None
    }

    /// Adds a credential, last used now.
    pub fn insert(&self, hash: &str, c: Cred) {
        let last_used = self.clock.now();
        self.map
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(hash.to_string(), Entry { cred: c, last_used });
    }

    /// Takes `other`'s credentials in place of these.
    pub fn replace_with(&self, other: Credentials) {
        let map = other
            .map
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        *self.map.write().unwrap_or_else(PoisonError::into_inner) = map;
    }

    /// Runs `change` on the store (a mint or a revoke), then reloads these
    /// credentials from it; concurrent refreshes run one at a time, so the
    /// map always ends as the store's latest state.
    ///
    /// # Errors
    /// `change`'s, or the store's.
    pub fn refresh<T>(
        &self,
        st: &Store,
        change: impl FnOnce(&Store) -> clax_core::Result<T>,
    ) -> clax_core::Result<T> {
        let _one = self
            .refreshing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let out = change(st)?;
        *self.map.write().unwrap_or_else(PoisonError::into_inner) = read_map(st)?;
        Ok(out)
    }

    /// Whether a use, now, of the live credential whose hash is `hash` should
    /// be written to the store: when it was last used [`TOUCH_EVERY`] ago or
    /// more. Answering `true` records the use here.
    pub fn due_for_touch(&self, hash: &str) -> bool {
        let now = self.clock.now();
        let mut map = self.map.write().unwrap_or_else(PoisonError::into_inner);
        match map.get_mut(hash) {
            Some(e) if live(e, now) && now - e.last_used >= TOUCH_EVERY => {
                e.last_used = now;
                true
            }
            _ => false,
        }
    }
}

/// The `Authorization` scheme of the extension's requests:
/// `Authorization: Clax-Extension <credential>`.
pub const SCHEME: &str = "Clax-Extension";

/// Marks a request the gateway admitted. [`crate::identity::Identity`] counts
/// it as a browser of the owner's; [`crate::live::SeesLive`] and the stream
/// read it as "live pages only". Only [`gateway`] inserts it, after checking
/// the credential.
#[derive(Clone, Copy, Debug)]
pub struct ViaExtension;

/// The hash of the credential an admitted request carried, beside
/// [`ViaExtension`]: a stream opened with it ends when it stops being live.
#[derive(Clone, Debug)]
pub struct ExtensionCredential(String);

impl ExtensionCredential {
    /// The credential's SHA-256 (lowercase hex).
    pub fn hash(&self) -> &str {
        &self.0
    }
}

/// Whether the credential whose hash is `hash` is live in `creds` and was
/// minted for `extension_id` (the ID in effect).
pub fn is_live(creds: &Credentials, hash: &str, extension_id: &str) -> bool {
    creds
        .get(hash)
        .is_some_and(|c| c.extension_id == extension_id)
}

/// Ends the open streams of every extension credential that is no longer
/// live in `s` (after a revoke, or a mint that revoked the oldest).
pub fn end_dead_streams(s: &AppState) {
    s.stream
        .end_streams_where(|h| !is_live(&s.ext_creds, h, &s.extension_id));
}

/// What a route needs besides the credential: nothing more, or that the
/// artifact its path names (percent-encoded as in the path) is a live page.
#[derive(Debug, PartialEq, Eq)]
enum Rule<'a> {
    Any,
    Live(&'a str),
}

/// The routes the extension may use, and what each needs (spec 2026-10-05
/// §9.2); `None` for every other route. Routes taking an artifact ID in the
/// body or a topic check it themselves through [`crate::live::SeesLive`].
fn rule<'a>(m: &Method, path: &'a str) -> Option<Rule<'a>> {
    let segs: Vec<&str> = path.strip_prefix('/')?.split('/').collect();
    let (get, post, put, del) = (
        m == Method::GET,
        m == Method::POST,
        m == Method::PUT,
        m == Method::DELETE,
    );
    match segs.as_slice() {
        ["api", "live", "pages" | "site"] if get => Some(Rule::Any),
        ["api", "live", "threads" | "snapshots"] if post => Some(Rule::Any),
        ["api", "live", "threads", _, "move"] if post => Some(Rule::Any),
        ["api", "live", "rules"] if get || post => Some(Rule::Any),
        ["api", "live", "rules", _] if del => Some(Rule::Any),
        ["api", "viewers", "me"] if get || put => Some(Rule::Any),
        ["api", "viewers", "me", "looked" | "presence"] if put => Some(Rule::Any),
        ["api", "stream"] if get => Some(Rule::Any),
        ["api", "stream", _] if post => Some(Rule::Any),
        ["api", "artifacts", aid] if get => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads" | "working" | "presence"] if get => {
            Some(Rule::Live(aid))
        }
        ["api", "artifacts", aid, "threads:send"] if post => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads", _] if get || del => Some(Rule::Live(aid)),
        ["api", "artifacts", aid, "threads", _, "clip"] if get => Some(Rule::Live(aid)),
        [
            "api",
            "artifacts",
            aid,
            "threads",
            _,
            "comments" | "send" | "resolve" | "reopen",
        ] if post => Some(Rule::Live(aid)),
        _ => None,
    }
}

/// Lets the extension's origin read `res`.
fn cors(res: &mut Response, origin: &str) {
    let h = res.headers_mut();
    if let Ok(o) = HeaderValue::from_str(origin) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
    }
    h.append(header::VARY, HeaderValue::from_static("Origin"));
}

/// `e`, readable by the extension's origin.
fn refuse(e: ApiError, origin: &str) -> Response {
    let mut r = e.into_response();
    cors(&mut r, origin);
    r
}

/// The values of `headers`' `Authorization` headers.
fn authorizations(h: &HeaderMap) -> impl Iterator<Item = &str> {
    h.get_all(header::AUTHORIZATION)
        .iter()
        .filter_map(|v| v.to_str().ok())
}

/// Whether `v` is an `Authorization` value of `scheme` (case-insensitive).
fn has_scheme(v: &str, scheme: &str) -> bool {
    v.split_once(' ')
        .is_some_and(|(s, _)| s.eq_ignore_ascii_case(scheme))
}

/// The well-formed credential in `Authorization: Clax-Extension <credential>`.
fn presented(h: &HeaderMap) -> Option<&str> {
    authorizations(h)
        .filter(|v| has_scheme(v, SCHEME))
        .find_map(|v| {
            let c = v.split_once(' ')?.1.trim();
            is_credential(c).then_some(c)
        })
}

/// Whether a request is the extension's: its `Origin` is `origin`, or it
/// is a GET with no `Origin` that carries a `Clax-Extension` credential and
/// is marked `Sec-Fetch-Site: none`. Chrome sends no `Origin` on an
/// extension's GET to an origin the extension holds a host permission for
/// (`<all_urls>`, or "On all sites"), and marks it `none`, which no web
/// page can send: a page's request carries its `Origin`, or is
/// `same-origin`, `same-site` or `cross-site`. Chrome sends `Origin` on
/// every other method, so a write without one is never the extension's.
/// (Provisional, pending the owner's confirmation.)
fn from_extension(method: &Method, h: &HeaderMap, origin: &str) -> bool {
    match h.get(header::ORIGIN) {
        Some(v) => v.to_str().ok() == Some(origin),
        None => {
            method == Method::GET
                && h.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("none")
                && authorizations(h).any(|v| has_scheme(v, SCHEME))
        }
    }
}

/// Middleware: the extension gateway (spec 2026-10-05-chrome-overlay-design
/// L5, L6, §9.2, §10 item 6).
///
/// A request of the extension's ([`from_extension`]: its `Origin` is
/// `chrome-extension://<ID in effect>`, or a privileged GET without one)
/// is admitted only from a loopback peer, only to [`rule`]'s
/// routes, only with a live credential for that ID, and only for live pages;
/// otherwise 403 `forbidden` (401 `unknown_credential` for a missing,
/// unknown, expired or revoked credential; 404 for an artifact that is not
/// a live page, as for a missing one). A bearer token from that origin is
/// refused 403. An admitted request reaches its handler without `Origin`,
/// `Sec-Fetch-Site`, `Cookie` or `Authorization`, marked [`ViaExtension`]
/// (the owner identity, a browser of the owner's); `Set-Cookie` is removed
/// from its response. Every response to the extension's origin, refusals
/// included, carries `Access-Control-Allow-Origin: <that origin>` and
/// `Vary: Origin`. Preflights are answered 204 for the allowlisted routes and
/// 403 otherwise, without `Access-Control-Allow-Credentials` (the extension
/// sends no cookies).
///
/// A `Clax-Extension` credential with any other `Origin`, or none on a
/// method other than GET or with a `Sec-Fetch-Site` other than `none`, is
/// refused 403 `forbidden_origin`.
/// Every other request passes untouched.
pub async fn gateway(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let origin = extension_origin(&s.extension_id);
    let ours = from_extension(req.method(), req.headers(), &origin);
    if !ours {
        if authorizations(req.headers()).any(|v| has_scheme(v, SCHEME)) {
            return ApiError::forbidden(
                "forbidden_origin",
                "the extension's credential is accepted only from the extension",
            )
            .into_response();
        }
        return next.run(req).await;
    }
    let loopback = crate::identity::peer_of(req.extensions()).is_some_and(crate::auth::is_loopback);
    if !loopback {
        return refuse(
            ApiError::forbidden(
                "forbidden",
                "the extension reaches the daemon on this machine only",
            ),
            &origin,
        );
    }
    if req.method() == Method::OPTIONS {
        return preflight(&req, &origin);
    }
    if authorizations(req.headers()).any(|v| has_scheme(v, "Bearer")) {
        return refuse(
            ApiError::forbidden(
                "forbidden",
                "the daemon token is not accepted from the extension",
            ),
            &origin,
        );
    }
    let path = req.uri().path().to_string();
    let Some(r) = rule(req.method(), &path) else {
        return refuse(
            ApiError::forbidden("forbidden", "the extension may not use this route"),
            &origin,
        );
    };
    let unknown = || {
        refuse(
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unknown_credential",
                "the extension's credential is not live; pair again",
            ),
            &origin,
        )
    };
    let Some(hash) = presented(req.headers()).map(credential_hash) else {
        return unknown();
    };
    if !is_live(&s.ext_creds, &hash, &s.extension_id) {
        return unknown();
    }
    if let Rule::Live(aid) = r
        && !crate::live::percent_decoded(aid).is_some_and(|a| s.live_ids.contains(&a))
    {
        return refuse(clax_core::CoreError::NotFound.into(), &origin);
    }
    if s.ext_creds.due_for_touch(&hash) {
        let store = s.store.clone();
        let hash = hash.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = store.touch_extension_credential(&hash) {
                tracing::warn!(error = %e, "could not record an extension credential's use");
            }
        });
    }
    let h = req.headers_mut();
    for name in [header::ORIGIN, header::COOKIE, header::AUTHORIZATION] {
        h.remove(name);
    }
    h.remove("sec-fetch-site");
    req.extensions_mut().insert(ViaExtension);
    req.extensions_mut().insert(ExtensionCredential(hash));
    let mut res = next.run(req).await;
    res.headers_mut().remove(header::SET_COOKIE);
    cors(&mut res, &origin);
    res
}

/// The answer to a preflight from the extension's `origin`: 204 with the
/// CORS grant when the method it asks for is allowed on the path, else 403.
fn preflight(req: &Request, origin: &str) -> Response {
    let asked = req
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|v| Method::from_bytes(v.as_bytes()).ok());
    if asked
        .as_ref()
        .and_then(|m| rule(m, req.uri().path()))
        .is_none()
    {
        return refuse(
            ApiError::forbidden("forbidden", "the extension may not use this route"),
            origin,
        );
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    let h = r.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PUT, DELETE"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization, content-type, last-event-id"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    cors(&mut r, origin);
    r
}

/// Whether `e` has been used within [`CREDENTIAL_TTL_DAYS`] of `now`.
fn live(e: &Entry, now: DateTime<Utc>) -> bool {
    now - e.last_used <= Duration::days(CREDENTIAL_TTL_DAYS)
}

fn read_map(st: &Store) -> clax_core::Result<HashMap<String, Entry>> {
    Ok(st
        .live_extension_credentials()?
        .into_iter()
        .filter_map(|k| {
            let last_used = DateTime::parse_from_rfc3339(&k.last_used_at)
                .ok()?
                .with_timezone(&Utc);
            Some((
                k.hash,
                Entry {
                    cred: Cred {
                        extension_id: k.extension_id,
                    },
                    last_used,
                },
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::working::ManualClock;

    const DAY: i64 = 86_400;

    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        (dir, Arc::new(st))
    }

    #[test]
    fn a_panic_while_the_credentials_are_locked_does_not_break_later_requests() {
        let (_d, st) = store();
        let c = Credentials::new(Arc::new(SystemClock));
        let cred = || Cred {
            extension_id: "x".into(),
        };
        c.insert("h1", cred());
        std::thread::scope(|s| {
            let poison = s.spawn(|| {
                let _map = c.map.write().unwrap();
                let _one = c.refreshing.lock().unwrap();
                panic!("poison the locks");
            });
            assert!(poison.join().is_err());
        });
        assert!(c.map.is_poisoned() && c.refreshing.is_poisoned());
        assert!(c.get("h1").is_some());
        c.insert("h2", cred());
        assert!(!c.due_for_touch("h2"));
        c.refresh(&st, |_| Ok(())).unwrap();
        assert!(c.get("h1").is_none(), "reloaded from the store");
        c.replace_with(Credentials::new(Arc::new(SystemClock)));
    }

    #[test]
    fn the_allowlist_names_each_route_and_the_artifact_it_needs_live() {
        let (g, p, u, d) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
        for (m, path, want) in [
            (&g, "/api/live/pages", Some(Rule::Any)),
            (&p, "/api/live/threads", Some(Rule::Any)),
            (&p, "/api/live/snapshots", Some(Rule::Any)),
            (&g, "/api/live/site", Some(Rule::Any)),
            (&p, "/api/live/threads/T/move", Some(Rule::Any)),
            (&g, "/api/live/rules", Some(Rule::Any)),
            (&p, "/api/live/rules", Some(Rule::Any)),
            (&d, "/api/live/rules/R", Some(Rule::Any)),
            (&p, "/api/live/site", None),
            (&g, "/api/live/threads/T/move", None),
            (&d, "/api/live/rules", None),
            (&p, "/api/live/rules/R", None),
            (&p, "/api/live/threads/T/resolve", None),
            (&g, "/api/viewers/me", Some(Rule::Any)),
            (&u, "/api/viewers/me", Some(Rule::Any)),
            (&u, "/api/viewers/me/looked", Some(Rule::Any)),
            (&u, "/api/viewers/me/presence", Some(Rule::Any)),
            (&g, "/api/stream", Some(Rule::Any)),
            (&p, "/api/stream/s1", Some(Rule::Any)),
            (&g, "/api/artifacts/A", Some(Rule::Live("A"))),
            (&g, "/api/artifacts/A/threads", Some(Rule::Live("A"))),
            (&g, "/api/artifacts/A/working", Some(Rule::Live("A"))),
            (&g, "/api/artifacts/A/presence", Some(Rule::Live("A"))),
            (&p, "/api/artifacts/A/threads:send", Some(Rule::Live("A"))),
            (&g, "/api/artifacts/A/threads/T", Some(Rule::Live("A"))),
            (&d, "/api/artifacts/A/threads/T", Some(Rule::Live("A"))),
            (&g, "/api/artifacts/A/threads/T/clip", Some(Rule::Live("A"))),
            (
                &p,
                "/api/artifacts/A/threads/T/comments",
                Some(Rule::Live("A")),
            ),
            (&p, "/api/artifacts/A/threads/T/send", Some(Rule::Live("A"))),
            (
                &p,
                "/api/artifacts/A/threads/T/resolve",
                Some(Rule::Live("A")),
            ),
            (
                &p,
                "/api/artifacts/A/threads/T/reopen",
                Some(Rule::Live("A")),
            ),
            (&p, "/api/artifacts/A/threads", None),
            (&d, "/api/artifacts/A", None),
            (&p, "/api/artifacts", None),
            (&g, "/api/artifacts", None),
            (&g, "/api/artifacts/A/docs", None),
            (&g, "/api/artifacts/A/versions", None),
            (&g, "/api/viewers/me/seen", None),
            (&g, "/api/viewers/me/attention", None),
            (&g, "/api/token", None),
            (&g, "/api/events", None),
            (&g, "/api/extension", None),
            (&g, "/api/live/pages/", None),
            (&g, "//api/live/pages", None),
            (&g, "api/live/pages", None),
            (&p, "/api/live/pages", None),
            (&g, "/c/A/v/1/", None),
        ] {
            assert_eq!(rule(m, path), want, "{m} {path}");
        }
    }

    #[test]
    fn only_a_well_formed_extension_credential_is_presented() {
        let c = clax_core::extension::new_credential();
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            format!("clax-extension {c}").parse().unwrap(),
        );
        assert_eq!(presented(&h), Some(c.as_str()));
        h.insert(
            header::AUTHORIZATION,
            format!("Bearer {c}").parse().unwrap(),
        );
        assert_eq!(presented(&h), None);
        h.insert(
            header::AUTHORIZATION,
            "Clax-Extension cxe_short".parse().unwrap(),
        );
        assert_eq!(presented(&h), None);
    }

    #[test]
    fn the_cache_holds_the_live_credentials_by_hash() {
        let (_dir, st) = store();
        let a = st.mint_extension_credential("abc").unwrap();
        let c = Credentials::load(&st).unwrap();
        assert_eq!(
            c.get(&a.hash),
            Some(Cred {
                extension_id: "abc".into()
            })
        );
        assert_eq!(c.get(&a.credential), None, "keyed by hash only");
        c.refresh(&st, |st| st.revoke_extension_credentials())
            .unwrap();
        assert_eq!(c.get(&a.hash), None);
    }

    #[test]
    fn an_idle_credential_expires_while_the_daemon_runs_and_a_use_keeps_one_live() {
        let (_dir, st) = store();
        let idle = st.mint_extension_credential("abc").unwrap();
        let used = st.mint_extension_credential("abc").unwrap();
        let clock = Arc::new(ManualClock::at(&Store::now()));
        let c = Credentials::load_with(&st, clock.clone()).unwrap();
        assert!(!c.due_for_touch(&used.hash), "used just now");
        clock.advance(29 * DAY);
        assert!(c.due_for_touch(&used.hash));
        assert!(!c.due_for_touch(&used.hash), "at most hourly");
        clock.advance(DAY + 1);
        assert_eq!(c.get(&idle.hash), None, "unused for more than 30 days");
        assert!(
            !c.due_for_touch(&idle.hash),
            "an expired credential is dropped"
        );
        assert!(c.get(&used.hash).is_some(), "used a day ago");
        clock.advance(30 * DAY);
        assert_eq!(c.get(&used.hash), None);
    }

    #[test]
    fn concurrent_mints_never_drop_a_fresh_credential() {
        let (_dir, st) = store();
        let c = Arc::new(Credentials::load(&st).unwrap());
        let mints: Vec<_> = (0..clax_core::extension::MAX_CREDENTIALS)
            .map(|_| {
                let (st, c) = (st.clone(), c.clone());
                std::thread::spawn(move || {
                    c.refresh(&st, |st| st.mint_extension_credential("abc"))
                        .unwrap()
                })
            })
            .collect();
        for m in mints {
            let m = m.join().unwrap();
            assert!(c.get(&m.hash).is_some());
        }
    }
}
