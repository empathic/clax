//! Agents of other machines (spec §6.6, §6.7, §7.5, M4, M7): what each
//! route needs of an agent ([`Need`], declared with `need!`), the
//! [`Agent`] extractor that checks it, and the two layers that
//! authenticate a `Clax-Grant` request before any handler runs.
//!
//! - [`authenticate`] runs in `lib.rs`'s `wrap`, after the host rewrite, so
//!   it sees every request the daemon serves. For an agent credential it
//!   checks, before any of the body is read: no `Origin` or cookie, the
//!   token in the [`crate::grant_auth::GrantIndex`], the `Clax-Signature`
//!   header's shape, `t` within the skew, and the nonce's absence (without
//!   recording it).
//! - [`verify_layer`] is a `route_layer` over every route, so it knows the
//!   route by method and `MatchedPath`. It passes every request without an
//!   agent credential through untouched. For one, it finds the route's
//!   `need!` (403 `forbidden` without one), reads the body (none, or at most
//!   the route's `LIMIT`) within the bounds of [`Gate`], verifies the
//!   signature, records the nonce (a second presence test, under the
//!   cache's lock), and hands the handler the buffered body.

use crate::auth::Caller;
use crate::error::ApiError;
use crate::grant_auth::{self, Refusal};
use crate::identity::Identity;
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, MatchedPath, RawPathParams, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use clax_core::rights::RightType;
use clax_core::sig::{self, SigError};
use clax_core::{ArtifactId, Secret, secret};
use futures::StreamExt;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// What an agent may do with a route (spec §7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Every author field of the response goes through the visibility
    /// filter; until a route's filter is live ([`FILTERED_LIVE`]), a
    /// person's agent gets 503 `not_yet`.
    Filtered,
    /// Only the caller's own person's fields and Clax's own.
    Own,
    /// Refused to a person's agent.
    DeniedToGrants,
    /// The grant flow.
    GrantFlow,
}

/// A route's `need!` declaration, as a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeedInfo {
    pub name: &'static str,
    pub right: Option<(RightType, &'static str)>,
    pub class: Class,
    /// The most body an agent's request may carry, read and verified
    /// before the handler runs; the route's own body limit.
    pub limit: usize,
}

/// What an agent needs to use a route: the right checked at the artifact
/// its path names, its class, and its body limit (M7).
pub trait Need: Send + Sync + 'static {
    const NAME: &'static str;
    const RIGHT: Option<(RightType, &'static str)>;
    const CLASS: Class;
    const LIMIT: usize;
    const INFO: NeedInfo = NeedInfo {
        name: Self::NAME,
        right: Self::RIGHT,
        class: Self::CLASS,
        limit: Self::LIMIT,
    };
}

/// Declares a route's [`Need`]: `need!(Name, Type, "action", Class,
/// LIMIT)`, or `need!(Name, None, Class, LIMIT)` for a route that checks no
/// right.
macro_rules! need {
    ($name:ident, None, $class:ident, $limit:expr) => {
        pub struct $name;
        impl Need for $name {
            const NAME: &'static str = stringify!($name);
            const RIGHT: Option<(RightType, &'static str)> = None;
            const CLASS: Class = Class::$class;
            const LIMIT: usize = $limit;
        }
    };
    ($name:ident, $ty:ident, $action:literal, $class:ident, $limit:expr) => {
        pub struct $name;
        impl Need for $name {
            const NAME: &'static str = stringify!($name);
            const RIGHT: Option<(RightType, &'static str)> = Some((RightType::$ty, $action));
            const CLASS: Class = Class::$class;
            const LIMIT: usize = $limit;
        }
    };
}

/// axum's default body limit, which routes without a layer of their own
/// keep.
pub const DEFAULT_LIMIT: usize = 2 * 1024 * 1024;

need!(SessionsList, None, Own, DEFAULT_LIMIT);
need!(SessionsRegister, None, Own, DEFAULT_LIMIT);
need!(SessionsJoin, None, Own, DEFAULT_LIMIT);
need!(SessionsGet, None, Own, DEFAULT_LIMIT);
need!(SessionsPatch, None, Own, DEFAULT_LIMIT);
need!(ArtifactsList, Artifacts, "read", Filtered, DEFAULT_LIMIT);
need!(
    Publish,
    Artifacts,
    "update",
    Filtered,
    crate::routes::artifacts::PUBLISH_BODY_LIMIT
);
need!(
    AssetsUpload,
    Artifacts,
    "update",
    Filtered,
    crate::routes::assets::UPLOAD_BODY_LIMIT
);
need!(DocsPut, Db, "write", Filtered, DEFAULT_LIMIT);
need!(FeedbackPoll, Comments, "watch", Filtered, DEFAULT_LIMIT);
need!(FeedbackNotices, Comments, "watch", Filtered, DEFAULT_LIMIT);
need!(FeedbackAck, Comments, "watch", Own, DEFAULT_LIMIT);
need!(WorkingForSession, Comments, "watch", Own, DEFAULT_LIMIT);
need!(WatchesList, Comments, "watch", Own, DEFAULT_LIMIT);
need!(WatchesPut, Comments, "watch", Own, DEFAULT_LIMIT);
need!(WatchesDelete, Comments, "watch", Own, DEFAULT_LIMIT);

/// Every route an agent of another machine may reach, by method and the
/// path it is routed at (`MatchedPath`), with its need: [`verify_layer`]
/// refuses every other route to an agent credential before reading
/// anything. A guard test in `routes` checks each entry against the
/// handler routed there, which takes `Agent<Name>`.
pub const ROUTES: &[(&str, &str, NeedInfo)] = &[
    ("GET", "/api/sessions", SessionsList::INFO),
    ("POST", "/api/sessions", SessionsRegister::INFO),
    ("POST", "/api/sessions/join", SessionsJoin::INFO),
    ("GET", "/api/sessions/{id}", SessionsGet::INFO),
    ("PATCH", "/api/sessions/{id}", SessionsPatch::INFO),
    ("GET", "/api/artifacts", ArtifactsList::INFO),
    ("POST", "/api/artifacts/{aid}/versions", Publish::INFO),
    ("POST", "/api/artifacts/{aid}/assets", AssetsUpload::INFO),
    ("PUT", "/api/artifacts/{aid}/docs/{*path}", DocsPut::INFO),
    ("GET", "/api/sessions/{id}/feedback", FeedbackPoll::INFO),
    ("GET", "/api/sessions/{id}/notices", FeedbackNotices::INFO),
    ("POST", "/api/sessions/{id}/feedback/ack", FeedbackAck::INFO),
    ("GET", "/api/sessions/{id}/working", WorkingForSession::INFO),
    ("GET", "/api/sessions/{id}/watches", WatchesList::INFO),
    ("PUT", "/api/sessions/{id}/watches/{aid}", WatchesPut::INFO),
    (
        "DELETE",
        "/api/sessions/{id}/watches/{aid}",
        WatchesDelete::INFO,
    ),
];

/// The `Filtered` needs whose filter is live: a person's agent reaches
/// them. Tasks 9–12 add to it as they filter each route.
pub const FILTERED_LIVE: &[&str] = &[];

/// The need of the route `matched` serves for `method` (`HEAD` as `GET`).
pub fn need_of(method: &Method, matched: &str) -> Option<NeedInfo> {
    let m = if method == Method::HEAD {
        "GET"
    } else {
        method.as_str()
    };
    ROUTES
        .iter()
        .find(|(rm, rp, _)| *rm == m && *rp == matched)
        .map(|(_, _, n)| *n)
}

/// Whether a route routed at `matched` names a session in its path
/// (`{id}` under `/api/sessions/`).
fn names_a_session(matched: &str) -> bool {
    ["/api/sessions/{id}", "/api/_test/sessions/{id}"]
        .iter()
        .any(|p| matched == *p || matched.starts_with(&format!("{p}/")))
}

/// The session `parts`' route names in its path, decoded as its handler
/// reads it.
async fn path_session(parts: &mut Parts, s: &AppState) -> Option<String> {
    let matched = parts.extensions.get::<MatchedPath>()?.as_str().to_string();
    if !names_a_session(&matched) {
        return None;
    }
    let params = RawPathParams::from_request_parts(parts, s).await.ok()?;
    params
        .iter()
        .find(|(k, _)| *k == "id")
        .map(|(_, v)| v.to_string())
}

/// The caller of a request, its store reads (if any) on the blocking
/// pool. `path_session` is the session its route's path names.
pub async fn resolve(
    s: &AppState,
    id: Identity,
    path_session: Option<String>,
) -> Result<Caller, ApiError> {
    if let Some(c) = id.quick_caller(path_session.as_deref()) {
        return Ok(c);
    }
    s.store
        .call(move |st| Ok(id.caller(st, path_session.as_deref())))
        .await
        .map_err(ApiError::from)?
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        let id = Identity::from_parts(&parts.headers, &parts.extensions, &s.token);
        let ps = path_session(parts, s).await;
        resolve(s, id, ps).await
    }
}

/// A route's caller, checked against the route's [`Need`] (M7): for a
/// person's agent, the right `N::RIGHT` at the artifact the route's path
/// names (`{aid}`; 403 `not_permitted`), and for a `Filtered` route whose
/// filter is not live, 503 `not_yet`. The sessions the request names
/// (path and `X-Clax-Session`) are the caller's ([`Identity::caller`]).
/// Every other caller passes as it is: the handler decides what it may do.
pub struct Agent<N: Need> {
    pub caller: Caller,
    /// The artifact the route's path names, when it parses.
    pub artifact: Option<ArtifactId>,
    /// The request carried the daemon token.
    token: bool,
    _n: PhantomData<N>,
}

impl<N: Need> Agent<N> {
    /// 401 `unauthorized` unless the request carried the daemon token or is
    /// an agent's: the gate of the routes that took only the token before
    /// agents of other machines could reach them.
    pub fn require_credential(&self) -> Result<(), ApiError> {
        if self.token || self.caller.is_agent() {
            Ok(())
        } else {
            Err(ApiError::unauthorized())
        }
    }

    /// The grant of a person's agent; `None` for every other caller.
    pub fn grant(&self) -> Option<&str> {
        match &self.caller {
            Caller::PersonAgent { grant, .. } => Some(grant),
            _ => None,
        }
    }
}

/// [`Agent`] on a route that took only the daemon token before agents of
/// other machines could reach it: every other caller is 401
/// `unauthorized` before anything else is extracted, as before.
pub struct TokenOrAgent<N: Need>(pub Agent<N>);

impl<N: Need> FromRequestParts<AppState> for TokenOrAgent<N> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        if !crate::auth::is_grant_request(&parts.headers)
            && !crate::auth::has_token(&parts.headers, &s.token)
        {
            return Err(ApiError::unauthorized());
        }
        let a = Agent::<N>::from_request_parts(parts, s).await?;
        a.require_credential()?;
        Ok(TokenOrAgent(a))
    }
}

/// 403 `not_permitted`: the caller's rights lack `right`.
fn not_permitted((kind, action): (RightType, &'static str)) -> ApiError {
    let mut e = ApiError::forbidden(
        "not_permitted",
        format!("this agent may not {action} ({}) here", kind.as_str()),
    );
    e.extra.insert(
        "right".into(),
        serde_json::json!({"type": kind.as_str(), "action": action}),
    );
    e
}

impl<N: Need> FromRequestParts<AppState> for Agent<N> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &AppState) -> Result<Self, ApiError> {
        let id = Identity::from_parts(&parts.headers, &parts.extensions, &s.token);
        let token = id.token;
        let ps = path_session(parts, s).await;
        let aid = RawPathParams::from_request_parts(parts, s)
            .await
            .ok()
            .and_then(|p| {
                p.iter()
                    .find(|(k, _)| *k == "aid")
                    .map(|(_, v)| v.to_string())
            });
        let caller = resolve(s, id, ps).await?;
        let artifact = match (&caller, aid) {
            (Caller::PersonAgent { .. }, Some(a)) => {
                Some(ArtifactId::parse(&a).map_err(|_| ApiError::not_found())?)
            }
            (_, a) => a.and_then(|a| ArtifactId::parse(&a).ok()),
        };
        if let Caller::PersonAgent { person, rights, .. } = &caller {
            if let Some((kind, action)) = N::RIGHT {
                let allowed = match &artifact {
                    Some(a) => {
                        let (rights, person, aid) =
                            (rights.clone(), person.clone(), a.as_str().to_string());
                        s.store_call(move |st| {
                            let own = |x: &str| {
                                st.artifact_created_by(x, person.as_deref())
                                    .unwrap_or(false)
                            };
                            Ok(rights.allows(kind, action, Some(&aid), &own))
                        })
                        .await?
                    }
                    None if action == "create" => rights.allows(kind, action, None, &|_| false),
                    None => rights.allows_somewhere(kind, action),
                };
                if !allowed {
                    return Err(not_permitted((kind, action)));
                }
            }
            if N::CLASS == Class::Filtered && !FILTERED_LIVE.contains(&N::NAME) {
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "not_yet",
                    "this route is not yet available to agents of another machine",
                ));
            }
        }
        Ok(Agent {
            caller,
            artifact,
            token,
            _n: PhantomData,
        })
    }
}

/// The grant a request's signature was verified for, set by
/// [`verify_layer`] for [`Identity`].
#[derive(Clone, Debug)]
pub struct VerifiedGrant {
    pub grant_id: String,
}

/// What [`authenticate`] found in a `Clax-Grant` request's headers, for
/// [`verify_layer`].
#[derive(Clone)]
struct GrantHead {
    grant_id: String,
    key: [u8; 32],
    token: Secret,
    head: grant_auth::Head,
}

/// A body over this is read for one request of a key at a time.
pub const LARGE_BODY: usize = 1024 * 1024;
/// Requests of one key that may wait for its permit.
pub const KEY_QUEUE: usize = 4;
/// The longest wait for a key's permit or for budget.
pub const WAIT: Duration = Duration::from_secs(30);
/// Unverified bytes in flight, overall.
pub const BUDGET: usize = 128 * 1024 * 1024;
/// How long an unverified body may take before its rate counts.
pub const GRACE: Duration = Duration::from_secs(10);
/// The slowest an unverified body may arrive on average after [`GRACE`],
/// in bytes a second.
pub const MIN_RATE: u64 = 64 * 1024;

/// The clock of [`Gate`]'s waits and rates: Tokio's in the daemon, one the
/// test drives in tests.
pub trait Timer: Send + Sync {
    /// The time since some fixed start.
    fn now(&self) -> Duration;
    /// Completes once [`Timer::now`] reaches `at`.
    fn sleep_until(&self, at: Duration) -> BoxFuture<'static, ()>;
}

/// [`Timer`] on Tokio's clock.
pub struct TokioTimer(tokio::time::Instant);

impl Default for TokioTimer {
    fn default() -> Self {
        TokioTimer(tokio::time::Instant::now())
    }
}

impl Timer for TokioTimer {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep_until(&self, at: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep_until(self.0 + at))
    }
}

struct Slot {
    sem: Arc<Semaphore>,
    waiting: usize,
}

/// The bounds on unverified bodies (spec §6.6): one large body per key at a
/// time with a short queue, a budget of bytes overall, and a minimum rate.
pub struct Gate {
    budget: Arc<Semaphore>,
    keys: Mutex<HashMap<[u8; 32], Slot>>,
    timer: Arc<dyn Timer>,
    /// Unverified bytes read, ever.
    read: AtomicU64,
    /// Requests waiting for budget.
    budget_waiting: std::sync::atomic::AtomicUsize,
    /// Woken whenever bytes are read or a request joins or leaves a queue
    /// or takes a permit (for tests that wait on the gate).
    changed: tokio::sync::Notify,
}

/// What a [`Gate`] holds now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct GateState {
    /// Bytes of budget reserved.
    pub reserved: usize,
    /// Keys whose permit is held.
    pub held: usize,
    /// Requests waiting for a key's permit.
    pub queued: usize,
    /// Requests waiting for budget.
    pub budget_waiting: usize,
    /// Unverified bytes read since the daemon started.
    pub read: u64,
}

impl Gate {
    pub fn new(timer: Arc<dyn Timer>) -> Gate {
        Gate {
            budget: Arc::new(Semaphore::new(BUDGET)),
            keys: Mutex::new(HashMap::new()),
            timer,
            read: AtomicU64::new(0),
            budget_waiting: Default::default(),
            changed: tokio::sync::Notify::new(),
        }
    }

    /// Returns once `f` holds of the gate's state; `None` after `limit`
    /// of real time.
    pub async fn wait_until(
        &self,
        f: impl Fn(GateState) -> bool,
        limit: Duration,
    ) -> Option<GateState> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let now = self.state();
            if f(now) {
                return Some(now);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return None;
            }
        }
    }

    pub fn state(&self) -> GateState {
        let keys = grant_auth::lock(&self.keys);
        GateState {
            reserved: BUDGET - self.budget.available_permits(),
            held: keys
                .values()
                .filter(|s| s.sem.available_permits() == 0)
                .count(),
            queued: keys.values().map(|s| s.waiting).sum(),
            budget_waiting: self.budget_waiting.load(Ordering::SeqCst),
            read: self.read.load(Ordering::SeqCst),
        }
    }

    /// Forgets `key`'s slot when nothing holds or waits for it.
    fn tidy(&self, key: &[u8; 32]) {
        let mut keys = grant_auth::lock(&self.keys);
        if keys
            .get(key)
            .is_some_and(|s| s.waiting == 0 && s.sem.available_permits() == 1)
        {
            keys.remove(key);
        }
        drop(keys);
        self.changed.notify_waiters();
    }

    /// `key`'s permit to read a large body: at once, or after waiting in
    /// its queue (at most [`KEY_QUEUE`], at most [`WAIT`]).
    ///
    /// # Errors
    /// 429 `limit_reached` when the queue is full or the wait ends.
    async fn key_permit(self: &Arc<Self>, key: [u8; 32]) -> Result<KeyPermit, ApiError> {
        let sem = {
            let mut keys = grant_auth::lock(&self.keys);
            let slot = keys.entry(key).or_insert_with(|| Slot {
                sem: Arc::new(Semaphore::new(1)),
                waiting: 0,
            });
            if let Ok(p) = slot.sem.clone().try_acquire_owned() {
                self.changed.notify_waiters();
                return Ok(KeyPermit {
                    gate: self.clone(),
                    key,
                    permit: Some(p),
                });
            }
            if slot.waiting >= KEY_QUEUE {
                return Err(limit_reached());
            }
            slot.waiting += 1;
            slot.sem.clone()
        };
        self.changed.notify_waiters();
        let waiting = Waiting {
            gate: self.clone(),
            key,
        };
        let deadline = self.timer.sleep_until(self.timer.now() + WAIT);
        let got = tokio::select! {
            p = sem.acquire_owned() => p.ok(),
            () = deadline => None,
        };
        drop(waiting);
        match got {
            Some(p) => Ok(KeyPermit {
                gate: self.clone(),
                key,
                permit: Some(p),
            }),
            None => Err(limit_reached()),
        }
    }

    /// Reserves `n` bytes of the budget, waiting at most [`WAIT`].
    ///
    /// # Errors
    /// 503 `busy` when they cannot be reserved in time, or ever.
    async fn reserve(&self, n: usize) -> Result<Option<OwnedSemaphorePermit>, ApiError> {
        if n == 0 {
            return Ok(None);
        }
        let Ok(count) = u32::try_from(n) else {
            return Err(busy());
        };
        if n > BUDGET {
            return Err(busy());
        }
        if let Ok(p) = self.budget.clone().try_acquire_many_owned(count) {
            self.changed.notify_waiters();
            return Ok(Some(p));
        }
        let _waiting = BudgetWaiting::new(self);
        let deadline = self.timer.sleep_until(self.timer.now() + WAIT);
        tokio::select! {
            p = self.budget.clone().acquire_many_owned(count) => p.map(Some).map_err(|_| busy()),
            () = deadline => Err(busy()),
        }
    }
}

/// A request waiting for budget; counted until dropped.
struct BudgetWaiting<'a>(&'a Gate);

impl<'a> BudgetWaiting<'a> {
    fn new(gate: &'a Gate) -> Self {
        gate.budget_waiting.fetch_add(1, Ordering::SeqCst);
        gate.changed.notify_waiters();
        BudgetWaiting(gate)
    }
}

impl Drop for BudgetWaiting<'_> {
    fn drop(&mut self) {
        self.0.budget_waiting.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

/// A request waiting in a key's queue; leaves it when dropped.
struct Waiting {
    gate: Arc<Gate>,
    key: [u8; 32],
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(s) = grant_auth::lock(&self.gate.keys).get_mut(&self.key) {
            s.waiting -= 1;
        }
        self.gate.tidy(&self.key);
    }
}

/// A key's permit to read a large unverified body; released when dropped.
struct KeyPermit {
    gate: Arc<Gate>,
    key: [u8; 32],
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for KeyPermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.gate.tidy(&self.key);
    }
}

fn limit_reached() -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "limit_reached",
        "this machine is already sending large requests: retry shortly",
    )
}

fn busy() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "busy",
        "the daemon is reading too much at once: retry shortly",
    )
}

fn too_large(limit: usize) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        format!("this route's request body holds at most {limit} bytes"),
    )
}

fn timeout() -> ApiError {
    ApiError::new(
        StatusCode::REQUEST_TIMEOUT,
        "timeout",
        "the request body arrived too slowly",
    )
}

/// The one `Authorization` value of a grant request, or the refusal of a
/// request with several (a credential never falls back to another).
fn grant_token(headers: &HeaderMap) -> Result<Secret, Refusal> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(v), None) = (all.next(), all.next()) else {
        return Err(grant_auth::invalid_token());
    };
    let v = v.to_str().map_err(|_| grant_auth::invalid_token())?;
    let (scheme, token) = v.split_once(' ').ok_or_else(grant_auth::invalid_token)?;
    let token = token.trim_matches(' ');
    if !scheme.eq_ignore_ascii_case(crate::auth::GRANT_SCHEME) || token.is_empty() {
        return Err(grant_auth::invalid_token());
    }
    Ok(Secret::new(token.to_string()))
}

/// Middleware: the header checks of a `Clax-Grant` request, before any of
/// its body is read (spec §6.6): no `Origin` or cookie (403
/// `forbidden_origin`), one `Authorization` naming an access token the
/// index knows (401 `invalid_token`; `grant_revoked` for an ended grant's),
/// a well-formed `Clax-Signature` (401 `invalid_signature`), `t` within the
/// skew (401 `stale_request`, with the daemon's `Date`), and a nonce not
/// yet spent (401 `replayed_request`; 429 `limit_reached` when the key has
/// spent its share); the nonce is not recorded here. Every other request
/// passes untouched.
pub async fn authenticate(State(s): State<AppState>, req: Request, next: Next) -> Response {
    if !crate::auth::is_grant_request(req.headers()) {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    match check_head(&s, &parts) {
        Ok(g) => {
            parts.extensions.insert(g);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(r) => r.into_response(),
    }
}

fn check_head(s: &AppState, parts: &Parts) -> Result<GrantHead, Refusal> {
    let st = &s.store;
    grant_auth::no_browser(&parts.headers)?;
    let token = grant_token(&parts.headers)?;
    let found = s
        .grant_auth
        .index
        .lookup(&secret::sha256(token.expose()))
        .filter(|g| g.kind == clax_core::store::people::CredentialKind::Access)
        .ok_or_else(grant_auth::invalid_token)?;
    if found.revoked {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "grant_revoked",
            "this machine's grant was revoked: connect again",
        )
        .into());
    }
    let head = grant_auth::head(parts, st)?;
    let now = st.now_utc().timestamp();
    if head.header.t.abs_diff(now) > sig::MAX_SKEW_S.unsigned_abs() {
        return Err(grant_auth::refuse(SigError::Stale, st));
    }
    s.grant_auth
        .check_nonce(&found.key, &head.header.nonce, head.header.t, now)
        .map_err(|e| grant_auth::refuse(e, st))?;
    Ok(GrantHead {
        grant_id: found.grant_id,
        key: found.key,
        token,
        head,
    })
}

/// Whether a request has no body: no or a zero `Content-Length`, and no
/// `Transfer-Encoding`.
fn bodyless(headers: &HeaderMap) -> bool {
    !headers.contains_key(header::TRANSFER_ENCODING)
        && headers
            .get(header::CONTENT_LENGTH)
            .is_none_or(|v| v.as_bytes() == b"0")
}

/// Reads an unverified body of at most `limit` bytes within the gate's
/// bounds: a key's permit for a large one, the budget for every byte, and
/// the minimum rate after the grace. Permit and budget are released when
/// it returns.
async fn read_unverified(
    gate: &Arc<Gate>,
    key: [u8; 32],
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<Bytes, ApiError> {
    let declared = match headers.get(header::CONTENT_LENGTH) {
        Some(v) if !headers.contains_key(header::TRANSFER_ENCODING) => Some(
            v.to_str()
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .ok_or_else(|| {
                    ApiError::bad_request("invalid_request", "a malformed Content-Length")
                })?,
        ),
        _ => None,
    };
    if declared.is_some_and(|n| n > limit) {
        return Err(too_large(limit));
    }
    let mut permit = match declared {
        Some(n) if n > LARGE_BODY => Some(gate.key_permit(key).await?),
        _ => None,
    };
    let mut reserved = match declared {
        Some(n) => gate.reserve(n).await?,
        None => None,
    };
    let start = gate.timer.now();
    let mut buf = Vec::with_capacity(declared.unwrap_or(0));
    let mut stream = body.into_data_stream();
    loop {
        let allowed = Duration::from_secs_f64(buf.len() as f64 / MIN_RATE as f64);
        let deadline = gate.timer.sleep_until(start + allowed.max(GRACE));
        let chunk = tokio::select! {
            biased;
            c = stream.next() => c,
            () = deadline => return Err(timeout()),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| {
            ApiError::bad_request("invalid_request", "the request body could not be read")
        })?;
        if buf.len() + chunk.len() > limit {
            return Err(too_large(limit));
        }
        if declared.is_none() {
            if let Some(more) = gate.reserve(chunk.len()).await? {
                match reserved.as_mut() {
                    Some(r) => r.merge(more),
                    None => reserved = Some(more),
                }
            }
            if buf.len() + chunk.len() > LARGE_BODY && permit.is_none() {
                permit = Some(gate.key_permit(key).await?);
            }
        }
        gate.read.fetch_add(chunk.len() as u64, Ordering::SeqCst);
        gate.changed.notify_waiters();
        buf.extend_from_slice(&chunk);
    }
    drop((permit, reserved));
    Ok(Bytes::from(buf))
}

/// Route layer: verifies a `Clax-Grant` request's signature before its
/// handler runs, on every method (spec §6.6, M5); passes every other
/// request through untouched, its body unread. A route without a `need!`
/// ([`ROUTES`]) is 403 `forbidden` to an agent credential before anything
/// is read.
pub async fn verify_layer(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(g) = req.extensions_mut().remove::<GrantHead>() else {
        if crate::auth::is_grant_request(req.headers()) {
            // Not authenticated: never reaches a handler.
            return grant_auth::invalid_token().into_response();
        }
        return next.run(req).await;
    };
    let need = req
        .extensions()
        .get::<MatchedPath>()
        .and_then(|m| need_of(req.method(), m.as_str()));
    let Some(need) = need else {
        return crate::auth::grants_refused().into_response();
    };
    let (mut parts, body) = req.into_parts();
    let body = if bodyless(&parts.headers) {
        Bytes::new()
    } else {
        match read_unverified(&s.agent_gate, g.key, &parts.headers, body, need.limit).await {
            Ok(b) => b,
            Err(e) => return e.into_response(),
        }
    };
    let st = &s.store;
    if !sig::verify(
        &g.key,
        &grant_auth::input(&g.head, &body, Some(g.token.expose())),
        &g.head.header.sig,
    ) {
        return grant_auth::refuse(SigError::Bad, st).into_response();
    }
    let now = st.now_utc().timestamp();
    if let Err(e) = s
        .grant_auth
        .record_nonce(&g.key, &g.head.header.nonce, g.head.header.t, now)
    {
        return grant_auth::refuse(e, st).into_response();
    }
    if s.grant_auth.use_due(&g.grant_id, now) {
        let (store, gid) = (s.store.clone(), g.grant_id.clone());
        tokio::spawn(async move {
            if let Err(e) = store.call(move |st| st.note_grant_use(&gid)).await {
                tracing::warn!(error = %e, "noting a grant's use failed");
            }
        });
    }
    parts.extensions.insert(VerifiedGrant {
        grant_id: g.grant_id,
    });
    next.run(Request::from_parts(parts, Body::from(body))).await
}

/// `GET /api/_test/agent/gate` (debug builds): what the gate holds.
#[cfg(debug_assertions)]
pub async fn test_gate(
    State(s): State<AppState>,
    _t: crate::auth::RequireToken,
) -> axum::Json<GateState> {
    axum::Json(s.agent_gate.state())
}

/// `GET /api/_test/caller` and `GET /api/_test/sessions/{id}/caller`
/// (debug builds): the caller the request resolves to.
#[cfg(debug_assertions)]
pub async fn test_caller(caller: Caller) -> axum::Json<serde_json::Value> {
    use serde_json::json;
    axum::Json(match &caller {
        Caller::Owner { browser } => json!({"caller": "owner", "browser": browser}),
        Caller::OwnerAgent { session } => json!({"caller": "owner_agent", "session": session}),
        Caller::PersonAgent {
            person,
            grant,
            session,
            ..
        } => {
            json!({"caller": "person_agent", "person": person, "grant": grant, "session": session})
        }
        Caller::Person { viewer } => json!({"caller": "person", "viewer": viewer.public_id}),
        Caller::Viewer { viewer } => json!({"caller": "viewer", "viewer": viewer.public_id}),
        Caller::Anonymous => json!({"caller": "anonymous"}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_are_found_by_method_and_matched_path_only() {
        assert_eq!(
            need_of(&Method::GET, "/api/sessions").map(|n| n.name),
            Some("SessionsList")
        );
        assert_eq!(
            need_of(&Method::HEAD, "/api/sessions").map(|n| n.name),
            Some("SessionsList")
        );
        assert_eq!(
            need_of(&Method::POST, "/api/sessions").map(|n| n.name),
            Some("SessionsRegister")
        );
        assert!(need_of(&Method::DELETE, "/api/sessions").is_none());
        assert!(need_of(&Method::GET, "/api/sessions/x").is_none());
        assert!(need_of(&Method::GET, "/api/inbox").is_none());
        let p = need_of(&Method::POST, "/api/artifacts/{aid}/versions").unwrap();
        assert_eq!(p.limit, crate::routes::artifacts::PUBLISH_BODY_LIMIT);
        assert_eq!(p.right, Some((RightType::Artifacts, "update")));
        assert_eq!(p.class, Class::Filtered);
    }

    #[test]
    fn session_paths_are_those_under_a_session() {
        assert!(names_a_session("/api/sessions/{id}"));
        assert!(names_a_session("/api/sessions/{id}/feedback"));
        assert!(!names_a_session("/api/sessions"));
        assert!(!names_a_session("/api/sessions/join"));
        assert!(!names_a_session("/api/sessions/{idx}"));
        assert!(!names_a_session("/api/inbox/{id}"));
    }

    #[test]
    fn a_body_is_absent_without_a_length_or_an_encoding() {
        let h = |pairs: &[(&str, &str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in pairs {
                m.insert(
                    axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    v.parse().unwrap(),
                );
            }
            m
        };
        assert!(bodyless(&h(&[])));
        assert!(bodyless(&h(&[("content-length", "0")])));
        assert!(!bodyless(&h(&[("content-length", "1")])));
        assert!(!bodyless(&h(&[("transfer-encoding", "chunked")])));
        assert!(!bodyless(&h(&[
            ("content-length", "0"),
            ("transfer-encoding", "chunked")
        ])));
    }

    #[test]
    fn one_grant_credential_or_none() {
        let mut m = HeaderMap::new();
        m.append(header::AUTHORIZATION, "Clax-Grant cxg_a".parse().unwrap());
        assert_eq!(grant_token(&m).unwrap().expose(), "cxg_a");
        m.append(header::AUTHORIZATION, "Bearer test-token".parse().unwrap());
        assert!(grant_token(&m).is_err(), "two credentials are refused");
        let mut m = HeaderMap::new();
        m.append(header::AUTHORIZATION, "clax-grant  ".parse().unwrap());
        assert!(grant_token(&m).is_err());
    }
}
