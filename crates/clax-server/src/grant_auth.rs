//! Key-bound requests of the grant flow (spec §6.3, §6.4, §6.6): the
//! in-memory index of issued grant tokens, the two nonce caches, and the
//! signature checks of `POST /api/grants` and of the continuation and
//! manage calls.
//!
//! A grant request carries no token: its key comes from its body, so it is
//! read (at most [`REQUEST_BODY_LIMIT`]) after `t` is tested, and its nonce
//! is checked, its signature verified and its nonce recorded under one lock
//! of a small cache of its own ([`RequestNonces`]). A request's continuation
//! calls, until its tokens are issued, use that cache too, their key read
//! from the store by grant. Every other signed call names its key through a
//! token the [`GrantIndex`] knows, so the main cache only ever holds keys
//! of issued grants: the token is looked up, the nonce checked, the body
//! read, the signature verified, and only then the nonce recorded (with a
//! second presence test), so a bad signature spends nothing.
//!
//! The index changes one grant at a time, in memory, after the change
//! committed; it is read whole from the store only on start and, after an
//! update that failed, on the next sweep.

use crate::error::ApiError;
use crate::state::AppState;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::Secret;
use clax_core::sig::{self, NonceCache, SigError, SignInput};
use clax_core::store::people::{CredentialKind, Unissued};
use clax_core::{Store, secret};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

/// The most a grant-flow request's body may hold: the first request of a
/// grant has no token yet (spec §6.6), and the continuation and manage
/// calls carry little.
pub const REQUEST_BODY_LIMIT: usize = 16 * 1024;
/// Seconds a grant request's nonce is remembered.
const REQUEST_WINDOW_S: i64 = 240;
/// Grant-request nonces per key in a window: a client makes one request
/// and polls it.
const REQUEST_NONCES_PER_KEY: usize = 16;
/// Keys the grant-request cache holds at once; a new key past them evicts
/// the oldest.
pub const REQUEST_KEYS: usize = 256;
/// Nonces the grant-request cache holds at once: every key at its cap.
const REQUEST_NONCES: usize = REQUEST_KEYS * REQUEST_NONCES_PER_KEY;
/// Keys one peer address holds in the grant-request cache; a new one past
/// them evicts that peer's oldest.
pub const REQUEST_KEYS_PER_PEER: usize = 16;

/// What one token is, as the index knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRef {
    pub grant_id: String,
    pub kind: CredentialKind,
    /// The key the token's requests are signed with.
    pub key: [u8; 32],
    /// The token's grant has ended (`grant_revoked`).
    pub revoked: bool,
}

/// The index could not take a change: it is marked for a rebuild from the
/// store on the next sweep ([`GrantIndex::rebuild_if_stale`]).
#[derive(Debug)]
pub struct IndexStale;

#[derive(Default)]
struct Tokens {
    by_hash: HashMap<[u8; 32], GrantRef>,
    by_grant: HashMap<String, Vec<[u8; 32]>>,
    /// Grants that ended, whether or not they held tokens then: tokens
    /// issued to one later (a race of an issue with a revocation, each
    /// changing the index after its own commit) are born revoked. Grant
    /// IDs are never reused.
    ended: std::collections::HashSet<String>,
}

impl Tokens {
    fn insert(&mut self, hash: [u8; 32], g: GrantRef) {
        self.by_grant
            .entry(g.grant_id.clone())
            .or_default()
            .push(hash);
        self.by_hash.insert(hash, g);
    }
}

/// The SHA-256 of every token of every grant whose tokens were issued
/// (spec §6.4): what each is, its grant, its key, and whether the grant has
/// ended. A request that never had tokens issued is not in it.
#[derive(Default)]
pub struct GrantIndex {
    tokens: RwLock<Tokens>,
    /// Bumped by every change, so a rebuild that raced one is not
    /// installed over it.
    changes: AtomicU64,
    stale: AtomicBool,
    /// The next change fails (debug builds, for tests).
    #[cfg(debug_assertions)]
    fail_next: AtomicBool,
}

impl GrantIndex {
    /// The index of `st`'s issued tokens.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<GrantIndex> {
        let index = GrantIndex::default();
        *index.tokens.write().unwrap_or_else(|e| e.into_inner()) = Self::read(st)?;
        Ok(index)
    }

    fn read(st: &Store) -> clax_core::Result<Tokens> {
        let mut t = Tokens {
            ended: st.ended_grant_ids()?.into_iter().collect(),
            ..Default::default()
        };
        for c in st.credential_index()? {
            t.insert(
                c.sha256,
                GrantRef {
                    grant_id: c.grant_id,
                    kind: c.kind,
                    key: c.key,
                    revoked: c.revoked,
                },
            );
        }
        Ok(t)
    }

    /// The token whose SHA-256 is `hash`.
    pub fn lookup(&self, hash: &[u8; 32]) -> Option<GrantRef> {
        self.tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_hash
            .get(hash)
            .cloned()
    }

    /// How many tokens it holds.
    pub fn len(&self) -> usize {
        self.tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_hash
            .len()
    }

    /// Whether it holds no token.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a change failed and a rebuild is due.
    pub fn is_stale(&self) -> bool {
        self.stale.load(Ordering::SeqCst)
    }

    /// Makes the next change fail, as a failure after the store committed
    /// would (debug builds, for tests).
    #[cfg(debug_assertions)]
    pub fn fail_next_change(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn change(&self, f: impl FnOnce(&mut Tokens)) -> Result<(), IndexStale> {
        #[cfg(debug_assertions)]
        if self.fail_next.swap(false, Ordering::SeqCst) {
            self.stale.store(true, Ordering::SeqCst);
            return Err(IndexStale);
        }
        let mut t = self.tokens.write().unwrap_or_else(|e| e.into_inner());
        self.changes.fetch_add(1, Ordering::SeqCst);
        f(&mut t);
        Ok(())
    }

    /// Grant `gid`, signing with `key`, was issued `token`, `manage` and
    /// `continue_token`: they replace whatever tokens it had.
    ///
    /// # Errors
    /// [`IndexStale`] when the change could not be made.
    pub fn issued(
        &self,
        gid: &str,
        key: [u8; 32],
        token: &Secret,
        manage: &Secret,
        continue_token: &Secret,
    ) -> Result<(), IndexStale> {
        self.change(|t| {
            for h in t.by_grant.remove(gid).unwrap_or_default() {
                t.by_hash.remove(&h);
            }
            let revoked = t.ended.contains(gid);
            for (s, kind) in [
                (token, CredentialKind::Access),
                (manage, CredentialKind::Manage),
                (continue_token, CredentialKind::Continue),
            ] {
                t.insert(
                    secret::sha256(s.expose()),
                    GrantRef {
                        grant_id: gid.to_string(),
                        kind,
                        key,
                        revoked,
                    },
                );
            }
        })
    }

    /// Grant `gid` ended: every token of it is `grant_revoked` from now,
    /// including tokens [`GrantIndex::issued`] records for it later.
    ///
    /// # Errors
    /// [`IndexStale`] when the change could not be made.
    pub fn ended(&self, gid: &str) -> Result<(), IndexStale> {
        self.change(|t| {
            t.ended.insert(gid.to_string());
            let hashes = t.by_grant.get(gid).cloned().unwrap_or_default();
            for h in hashes {
                if let Some(g) = t.by_hash.get_mut(&h) {
                    g.revoked = true;
                }
            }
        })
    }

    /// Forgets the ended grants without tokens that have no row any more
    /// (purged requests); whether any went. `existing` says which of the
    /// grants it is given still have a row: it is asked under the index's
    /// lock, so a grant that ends while the sweep runs is either recorded
    /// before the question (and kept, its row being there) or after it.
    /// Grant IDs are never reused, so a grant without a row can never be
    /// issued tokens.
    pub fn forget_ended(
        &self,
        existing: impl FnOnce(&[String]) -> std::collections::HashSet<String>,
    ) -> bool {
        let mut t = self.tokens.write().unwrap_or_else(|e| e.into_inner());
        let candidates: Vec<String> = t
            .ended
            .iter()
            .filter(|g| !t.by_grant.contains_key(*g))
            .cloned()
            .collect();
        if candidates.is_empty() {
            return false;
        }
        let kept = existing(&candidates);
        let before = t.ended.len();
        for g in candidates.iter().filter(|g| !kept.contains(*g)) {
            t.ended.remove(g);
        }
        t.ended.len() != before
    }

    /// The ended grants it remembers.
    pub fn ended_grants(&self) -> Vec<String> {
        self.tokens
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ended
            .iter()
            .cloned()
            .collect()
    }

    /// Reads the whole index from `st` again when a change failed. A change
    /// made while it read leaves the rebuild for the next call. Whether it
    /// rebuilt.
    ///
    /// # Errors
    /// The store's; the index stays marked.
    pub fn rebuild_if_stale(&self, st: &Store) -> clax_core::Result<bool> {
        if !self.is_stale() {
            return Ok(false);
        }
        let before = self.changes.load(Ordering::SeqCst);
        let fresh = Self::read(st)?;
        let mut t = self.tokens.write().unwrap_or_else(|e| e.into_inner());
        if self.changes.load(Ordering::SeqCst) != before {
            return Ok(false);
        }
        *t = fresh;
        self.stale.store(false, Ordering::SeqCst);
        Ok(true)
    }
}

/// Forgets the index's ended grants whose rows `st` no longer holds
/// (purged requests), asking the store which rows exist while the index is
/// locked ([`GrantIndex::forget_ended`]); a store error forgets nothing.
pub fn forget_purged(index: &GrantIndex, st: &Store) -> bool {
    index.forget_ended(|ids| {
        st.existing_grant_ids(ids).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "reading grant rows to forget failed");
            ids.iter().cloned().collect()
        })
    })
}

/// Logs a failed index change after the store committed; the index is
/// rebuilt on the next sweep, and the request still succeeds.
pub fn note_stale(r: Result<(), IndexStale>, what: &str) {
    if r.is_err() {
        tracing::warn!(
            what,
            "grant index change failed; rebuilding on the next sweep"
        );
    }
}

/// The nonce cache of keys that come from no issued token: grant requests
/// (the key in the body) and a request's continuation calls until its
/// tokens are issued. Anyone on the LAN can make such keys, so instead of
/// refusing a new key when full, it evicts the oldest one: of the peer's
/// own when the peer holds [`REQUEST_KEYS_PER_PEER`], else of all when the
/// cache holds [`REQUEST_KEYS`]. Forgetting such a key's nonces is harmless:
/// replaying a grant request spends no invite twice (an invite is used
/// once, a live key is `key_in_use`) and hands out only a continuation the
/// replayer cannot sign for.
pub struct RequestNonces {
    cache: NonceCache,
    /// Keys in the order they arrived, with their peer.
    order: VecDeque<([u8; 32], String)>,
}

impl RequestNonces {
    fn new() -> RequestNonces {
        RequestNonces {
            cache: NonceCache::new(REQUEST_WINDOW_S, REQUEST_NONCES_PER_KEY)
                .with_caps(REQUEST_KEYS, REQUEST_NONCES),
            order: VecDeque::new(),
        }
    }

    fn evict_at(&mut self, i: usize) {
        if let Some((k, _)) = self.order.remove(i) {
            self.cache.forget(&k);
        }
    }

    /// Makes room for `key` of peer `ip`.
    fn admit(&mut self, key: &[u8; 32], ip: &str) {
        if self.order.iter().any(|(k, _)| k == key) {
            return;
        }
        let mine: Vec<usize> = self
            .order
            .iter()
            .enumerate()
            .filter(|(_, (_, p))| p == ip)
            .map(|(i, _)| i)
            .collect();
        if mine.len() >= REQUEST_KEYS_PER_PEER {
            self.evict_at(mine[0]);
        }
        while self.order.len() >= REQUEST_KEYS {
            self.evict_at(0);
        }
        self.order.push_back((*key, ip.to_string()));
    }

    fn check(&mut self, key: &[u8; 32], nonce: &str, t: i64, now: i64) -> Result<(), SigError> {
        // A new key enters (and may evict) only once its signature
        // verifies, in `record`: a bad signature evicts nothing.
        let known = self.order.iter().any(|(k, _)| k == key);
        match self.cache.check(key, nonce, t, now) {
            Err(SigError::Full) if !known => Ok(()),
            r => r,
        }
    }

    fn record(
        &mut self,
        key: &[u8; 32],
        ip: &str,
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        self.admit(key, ip);
        self.cache.record(key, nonce, t, now)
    }

    fn sweep(&mut self, now: i64) {
        self.cache.sweep(now);
        let cache = &self.cache;
        self.order.retain(|(k, _)| cache.has_key(k));
    }
}

/// Live requests whose tokens are not issued yet: at most
/// `MAX_PENDING` made in the last ten minutes, and live no longer, so this
/// many keys leave room to spare.
const PENDING_KEYS: usize = 64;
/// Continuation nonces a request's key may spend in a window before its
/// tokens are issued: a client polls every 5 s (48 polls in 240 s), with
/// room for retries.
const PENDING_NONCES_PER_KEY: usize = 256;

/// The grant flow's signature state: the token index and the nonce caches.
pub struct GrantAuth {
    pub index: GrantIndex,
    /// Nonces of grant requests, whose key comes from the body; evicts.
    requests: Mutex<RequestNonces>,
    /// Nonces of live requests' continuation calls until their tokens are
    /// issued; never evicts (the request caps bound its keys).
    pending: Mutex<NonceCache>,
    /// Nonces of keys known through [`GrantAuth::index`].
    nonces: Mutex<NonceCache>,
    /// When each grant's use was last written.
    noted: Mutex<HashMap<String, i64>>,
}

impl GrantAuth {
    /// The state for `st`'s grants, with empty caches.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<GrantAuth> {
        Ok(GrantAuth {
            index: GrantIndex::load(st)?,
            requests: Mutex::new(RequestNonces::new()),
            pending: Mutex::new(pending_cache()),
            nonces: Mutex::new(NonceCache::new(sig::NONCE_WINDOW_S, sig::NONCES_PER_KEY)),
            noted: Mutex::new(HashMap::new()),
        })
    }

    /// Forgets the nonces of both caches older than their window.
    pub fn sweep(&self, now: i64) {
        lock(&self.requests).sweep(now);
        lock(&self.pending).sweep(now);
        lock(&self.nonces).sweep(now);
    }

    /// Keys with nonces remembered: (grant requests, the rest).
    pub fn cached_keys(&self) -> (usize, usize) {
        (lock(&self.requests).cache.keys(), lock(&self.nonces).keys())
    }
}

/// The cache of unissued requests' continuation nonces: never full below
/// [`PENDING_KEYS`] keys each within [`PENDING_NONCES_PER_KEY`].
fn pending_cache() -> NonceCache {
    NonceCache::new(sig::NONCE_WINDOW_S, PENDING_NONCES_PER_KEY)
        .with_caps(PENDING_KEYS, PENDING_KEYS * PENDING_NONCES_PER_KEY)
}

impl GrantAuth {
    /// Whether `nonce` of an issued grant's `key`, signed at `t`, may be
    /// spent at `now`: the header check, which records nothing.
    ///
    /// # Errors
    /// [`SigError::Stale`], [`SigError::Bad`], [`SigError::Replayed`] or
    /// [`SigError::Full`].
    pub fn check_nonce(
        &self,
        key: &[u8; 32],
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        lock(&self.nonces).check(key, nonce, t, now)
    }

    /// Records `nonce` of an issued grant's `key` once its signature
    /// verified, testing again under the cache's lock that it is new: of
    /// two copies of one request, one is recorded and the other is
    /// [`SigError::Replayed`].
    ///
    /// # Errors
    /// As [`NonceCache::record`].
    pub fn record_nonce(
        &self,
        key: &[u8; 32],
        nonce: &str,
        t: i64,
        now: i64,
    ) -> Result<(), SigError> {
        lock(&self.nonces).record(key, nonce, t, now)
    }

    /// Whether grant `gid`'s use at `now` is due to be written
    /// (`last_used_at`, at most once per
    /// [`clax_core::store::people::USE_NOTE_SECS`]): remembered here, so a
    /// request costs no store write otherwise.
    pub fn use_due(&self, gid: &str, now: i64) -> bool {
        let mut noted = lock(&self.noted);
        match noted.get(gid) {
            Some(&at) if now - at < clax_core::store::people::USE_NOTE_SECS => false,
            _ => {
                noted.insert(gid.to_string(), now);
                true
            }
        }
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A refused signed request: the error, with the daemon's `Date` when the
/// refusal is about time (`stale_request`), so the client can correct.
#[derive(Debug)]
pub struct Refusal {
    pub error: ApiError,
    pub date: Option<HeaderValue>,
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Refusal {
        Refusal { error, date: None }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut res = self.error.into_response();
        if let Some(d) = self.date {
            res.headers_mut().insert(header::DATE, d);
        }
        res
    }
}

/// `now` as an HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`).
fn http_date(st: &Store) -> Option<HeaderValue> {
    HeaderValue::from_str(&st.now_utc().format("%a, %d %b %Y %H:%M:%S GMT").to_string()).ok()
}

pub(crate) fn refuse(e: SigError, st: &Store) -> Refusal {
    let message = match e {
        SigError::Stale => "the request's time is more than 120 seconds from the daemon's clock",
        SigError::Replayed => "the request's nonce was already used",
        SigError::Full => "too many recent signed requests: retry shortly",
        SigError::Bad => "the request's Clax-Signature is missing, malformed or does not verify",
    };
    let status = match e {
        SigError::Full => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::UNAUTHORIZED,
    };
    Refusal {
        error: ApiError::new(status, e.code(), message),
        date: (e == SigError::Stale).then(|| http_date(st)).flatten(),
    }
}

pub(crate) fn invalid_token() -> Refusal {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_token",
        "that token is not this grant's",
    )
    .into()
}

/// A verified request of the grant flow.
pub struct Signed {
    /// The request's body, verified.
    pub body: Bytes,
    /// The `Host` the request was signed for.
    pub host: String,
    /// The key that signed it.
    pub key: [u8; 32],
    /// The `GNAP` token it carried (none for a grant request).
    pub token: Option<Secret>,
    /// The time it was signed at.
    pub t: i64,
    /// Its nonce.
    pub nonce: String,
    /// The peer it came from.
    pub peer_ip: String,
}

/// The parts of a request that are signed, before its body is read.
#[derive(Clone)]
pub(crate) struct Head {
    pub(crate) method: String,
    pub(crate) path_and_query: String,
    pub(crate) host: String,
    pub(crate) header: sig::SigHeader,
    pub(crate) peer_ip: String,
}

/// 403 `forbidden_origin` for a browser's request: an agent credential
/// never comes with an `Origin` or a cookie (spec §6.6).
pub(crate) fn no_browser(headers: &HeaderMap) -> Result<(), Refusal> {
    if headers.contains_key(header::ORIGIN) || headers.contains_key(header::COOKIE) {
        return Err(ApiError::forbidden(
            "forbidden_origin",
            "a signed request carries no Origin and no cookie",
        )
        .into());
    }
    Ok(())
}

pub(crate) fn head(parts: &axum::http::request::Parts, st: &Store) -> Result<Head, Refusal> {
    let header = parts
        .headers
        .get("clax-signature")
        .ok_or_else(|| refuse(SigError::Bad, st))
        .and_then(|v| sig::parse_header(v.as_bytes()).map_err(|e| refuse(e, st)))?;
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    Ok(Head {
        method: parts.method.as_str().to_string(),
        path_and_query: parts
            .uri
            .path_and_query()
            .map_or_else(|| parts.uri.path().to_string(), |p| p.as_str().to_string()),
        host,
        header,
        peer_ip: crate::identity::peer_of(&parts.extensions)
            .map(|p| p.ip().to_canonical().to_string())
            .unwrap_or_default(),
    })
}

fn too_large() -> Refusal {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        format!("a grant-flow request's body holds at most {REQUEST_BODY_LIMIT} bytes"),
    )
    .into()
}

/// Reads `body`, at most [`REQUEST_BODY_LIMIT`] bytes; a declared longer one
/// is refused before any of it is read.
async fn read_body(headers: &HeaderMap, body: axum::body::Body) -> Result<Bytes, Refusal> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > REQUEST_BODY_LIMIT as u64) {
        return Err(too_large());
    }
    axum::body::to_bytes(body, REQUEST_BODY_LIMIT)
        .await
        .map_err(|_| too_large())
}

pub(crate) fn input<'a>(h: &'a Head, body: &[u8], token: Option<&str>) -> SignInput<'a> {
    SignInput {
        method: &h.method,
        path_and_query: &h.path_and_query,
        host: &h.host,
        t: h.header.t,
        nonce: &h.header.nonce,
        body_sha256: Sha256::digest(body).into(),
        token_sha256: token.map(secret::sha256),
    }
}

/// Verifies `POST /api/grants` (spec §6.3, §6.6): no browser credential,
/// `t` within the skew before the body is read, then the body (at most
/// [`REQUEST_BODY_LIMIT`]), which `parse` reads into the request and its
/// key; the nonce checked, the signature verified over an empty token
/// line, and the nonce recorded, under one lock of the grant-request cache.
///
/// # Errors
/// 403 `forbidden_origin`, 401 `invalid_signature`, `stale_request` or
/// `replayed_request`, 413 `body_too_large`, 429 `limit_reached`, or what
/// `parse` refuses the body with.
pub async fn grant_request<T>(
    s: &AppState,
    req: axum::extract::Request,
    parse: impl FnOnce(&[u8]) -> Result<(T, [u8; 32]), ApiError>,
) -> Result<(Signed, T), Refusal> {
    let st = &s.store;
    let (parts, body) = req.into_parts();
    no_browser(&parts.headers)?;
    let h = head(&parts, st)?;
    let now = st.now_utc().timestamp();
    if h.header.t.abs_diff(now) > sig::MAX_SKEW_S.unsigned_abs() {
        return Err(refuse(SigError::Stale, st));
    }
    let body = read_body(&parts.headers, body).await?;
    let (parsed, key) = parse(&body)?;
    let now = st.now_utc().timestamp();
    {
        let mut c = lock(&s.grant_auth.requests);
        c.check(&key, &h.header.nonce, h.header.t, now)
            .map_err(|e| refuse(e, st))?;
        if !sig::verify(&key, &input(&h, &body, None), &h.header.sig) {
            return Err(refuse(SigError::Bad, st));
        }
        c.record(&key, &h.peer_ip, &h.header.nonce, h.header.t, now)
            .map_err(|e| refuse(e, st))?;
    }
    Ok((
        Signed {
            body,
            host: h.host,
            key,
            token: None,
            t: h.header.t,
            nonce: h.header.nonce.clone(),
            peer_ip: h.peer_ip,
        },
        parsed,
    ))
}

/// Verifies a continuation or manage call on grant `gid` (spec §4.2,
/// §6.4, §6.6): `Authorization: GNAP <token>` naming a token of kind
/// `kind` of that grant (in the index, or for a request whose tokens were
/// never issued, its continuation in the store), then the signature
/// header, `t` and the nonce's absence, all before the body is read; then
/// the body, the signature over the token, and the nonce's record.
///
/// # Errors
/// 403 `forbidden_origin`; 401 `invalid_token` for a missing or unknown
/// token or one of another kind or grant, `grant_revoked` for a token of
/// an ended grant, `invalid_signature`, `stale_request` or
/// `replayed_request`; 413 `body_too_large`; 429 `limit_reached`.
pub async fn gnap(
    s: &AppState,
    req: axum::extract::Request,
    gid: &str,
    kind: CredentialKind,
) -> Result<Signed, Refusal> {
    let st = &s.store;
    let (parts, body) = req.into_parts();
    no_browser(&parts.headers)?;
    let token = gnap_token(&parts.headers).ok_or_else(invalid_token)?;
    let found = s.grant_auth.index.lookup(&secret::sha256(token.expose()));
    let (key, h, issued) = match found {
        Some(g) if g.kind != kind || g.grant_id != gid => return Err(invalid_token()),
        Some(g) if g.revoked => {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "grant_revoked",
                "this machine's grant was revoked: connect again",
            )
            .into());
        }
        Some(g) => (g.key, head(&parts, st)?, true),
        None if kind == CredentialKind::Continue => {
            // A request whose tokens are not issued: the header and `t`
            // first, so a guess costs no store read.
            let h = head(&parts, st)?;
            if h.header.t.abs_diff(st.now_utc().timestamp()) > sig::MAX_SKEW_S.unsigned_abs() {
                return Err(refuse(SigError::Stale, st));
            }
            let (gid, tok) = (gid.to_string(), token.clone());
            match s
                .store_call(move |st| st.unissued_continuation(&gid, tok.expose()))
                .await?
            {
                Some(Unissued::Live(key)) => (key, h, false),
                Some(Unissued::Ended) => {
                    return Err(ApiError::new(
                        StatusCode::FORBIDDEN,
                        "denied",
                        "this request was denied, cancelled or has expired",
                    )
                    .into());
                }
                None => return Err(invalid_token()),
            }
        }
        None => return Err(invalid_token()),
    };
    let cache = if issued {
        &s.grant_auth.nonces
    } else {
        &s.grant_auth.pending
    };
    let (n, t) = (&h.header.nonce, h.header.t);
    lock(cache)
        .check(&key, n, t, st.now_utc().timestamp())
        .map_err(|e| refuse(e, st))?;
    let body = read_body(&parts.headers, body).await?;
    if !sig::verify(&key, &input(&h, &body, Some(token.expose())), &h.header.sig) {
        return Err(refuse(SigError::Bad, st));
    }
    lock(cache)
        .record(&key, n, t, st.now_utc().timestamp())
        .map_err(|e| refuse(e, st))?;
    Ok(Signed {
        body,
        host: h.host,
        key,
        token: Some(token),
        t,
        nonce: h.header.nonce.clone(),
        peer_ip: h.peer_ip,
    })
}

/// The token of `Authorization: GNAP <token>` (scheme matched ignoring
/// case).
fn gnap_token(headers: &HeaderMap) -> Option<Secret> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    let token = token.trim_matches(' ');
    (scheme.eq_ignore_ascii_case("gnap") && !token.is_empty())
        .then(|| Secret::new(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(s: &str) -> Secret {
        Secret::new(s.to_string())
    }

    #[test]
    fn the_index_changes_one_grant_at_a_time() {
        let index = GrantIndex::default();
        let (a, m, c) = (secret("cxg_a"), secret("cxm_a"), secret("cxc_a"));
        index.issued("g_1", [2u8; 32], &a, &m, &c).unwrap();
        index
            .issued(
                "g_2",
                [3u8; 32],
                &secret("cxg_b"),
                &secret("cxm_b"),
                &secret("cxc_b"),
            )
            .unwrap();
        assert_eq!(index.len(), 6);
        let found = index.lookup(&secret::sha256("cxm_a")).unwrap();
        assert_eq!(
            (
                found.grant_id.as_str(),
                found.kind,
                found.key,
                found.revoked
            ),
            ("g_1", CredentialKind::Manage, [2u8; 32], false)
        );
        // Issuing again replaces the grant's tokens.
        index
            .issued("g_1", [2u8; 32], &a, &m, &secret("cxc_a2"))
            .unwrap();
        assert_eq!(index.len(), 6);
        assert!(index.lookup(&secret::sha256("cxc_a")).is_none());
        index.ended("g_1").unwrap();
        assert!(index.lookup(&secret::sha256("cxg_a")).unwrap().revoked);
        assert!(!index.lookup(&secret::sha256("cxg_b")).unwrap().revoked);
        index.ended("g_unknown").unwrap();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_failed_change_marks_the_index_for_a_rebuild() {
        let index = GrantIndex::default();
        index.fail_next_change();
        assert!(index.ended("g_1").is_err());
        assert!(index.is_stale());
        assert!(index.ended("g_1").is_ok(), "only the next change fails");
    }

    #[test]
    fn a_full_request_cache_evicts_the_oldest_key_and_a_peers_own_first() {
        let mut r = RequestNonces::new();
        let n = sig::new_nonce();
        let key = |i: u32| {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_be_bytes());
            k
        };
        for i in 0..REQUEST_KEYS as u32 {
            let ip = format!("10.0.{}.{}", i / 8, i % 8);
            r.record(&key(i), &ip, &n, 0, 0).unwrap();
        }
        assert_eq!(r.cache.keys(), REQUEST_KEYS);
        r.record(&key(9999), "10.9.9.9", &n, 0, 0).unwrap();
        assert_eq!(r.cache.keys(), REQUEST_KEYS);
        assert!(!r.cache.has_key(&key(0)) && r.cache.has_key(&key(1)));
        // A peer at its own cap evicts its own oldest, nobody else's.
        let mut r = RequestNonces::new();
        r.record(&key(100), "10.0.0.2", &n, 0, 0).unwrap();
        for i in 0..REQUEST_KEYS_PER_PEER as u32 + 1 {
            r.record(&key(i), "10.0.0.1", &n, 0, 0).unwrap();
        }
        assert!(r.cache.has_key(&key(100)));
        assert!(!r.cache.has_key(&key(0)) && r.cache.has_key(&key(1)));
        assert_eq!(r.cache.keys(), REQUEST_KEYS_PER_PEER + 1);
        // A replay of a remembered key is still refused.
        assert_eq!(r.check(&key(1), &n, 0, 0), Err(SigError::Replayed));
        r.sweep(1000);
        assert!(r.order.is_empty() && r.cache.is_empty());
    }

    #[test]
    fn a_new_key_evicts_nothing_until_its_signature_verifies() {
        let mut r = RequestNonces::new();
        let n = sig::new_nonce();
        let key = |i: u32| {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_be_bytes());
            k
        };
        for i in 0..REQUEST_KEYS as u32 {
            r.record(&key(i), &format!("10.0.{}.{}", i / 8, i % 8), &n, 0, 0)
                .unwrap();
        }
        // The check of a new key passes on a full cache, and takes nothing.
        assert!(r.check(&key(9999), &n, 0, 0).is_ok());
        assert!(r.cache.has_key(&key(0)) && r.order.len() == REQUEST_KEYS);
    }

    #[test]
    fn a_pending_key_spends_at_most_its_cap_and_the_cache_holds_every_key_at_its_cap() {
        let mut c = pending_cache();
        let key = |i: u32| {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_be_bytes());
            k
        };
        for _ in 0..PENDING_NONCES_PER_KEY {
            c.record(&key(0), &sig::new_nonce(), 0, 0).unwrap();
        }
        assert_eq!(
            c.check(&key(0), &sig::new_nonce(), 0, 0),
            Err(SigError::Full),
            "a key's polls are capped"
        );
        // Every other live request still has room for all of its own.
        for i in 1..PENDING_KEYS as u32 {
            for _ in 0..PENDING_NONCES_PER_KEY {
                c.record(&key(i), &sig::new_nonce(), 0, 0).unwrap();
            }
        }
        assert_eq!(c.len(), PENDING_KEYS * PENDING_NONCES_PER_KEY);
    }

    fn store() -> (
        tempfile::TempDir,
        std::sync::Arc<clax_core::working::ManualClock>,
        Store,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-10-01T09:00:00Z"));
        let st = Store::open_with_clock(&clax_core::Home::at(dir.path().join("ax")), clock.clone())
            .unwrap();
        (dir, clock, st)
    }

    fn request(st: &Store, key: [u8; 32]) -> (String, String) {
        match st
            .request_grant(clax_core::store::people::NewGrant {
                key,
                client_name: "sam-mbp".into(),
                peer_ip: "192.168.1.20".into(),
                access: clax_core::rights::Rights::preset(clax_core::rights::PRESET_COLLABORATOR)
                    .unwrap(),
                invite: None,
            })
            .unwrap()
        {
            clax_core::store::people::GrantStart::Pending {
                grant, user_code, ..
            } => (grant.id, user_code.expose().to_string()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_revocation_during_the_sweep_keeps_its_record_and_only_purged_rows_are_forgotten() {
        use clax_core::store::people::EndedBy;
        let (_d, clock, st) = store();
        let index = GrantIndex::load(&st).unwrap();
        // A request approved, its tokens not yet collected.
        let (gid, code) = request(&st, [7; 32]);
        st.approve_grant(&gid, &code, "Sam", None).unwrap();
        // The sweep starts: at that moment the grant is live. Then the
        // revocation lands, before the sweep forgets anything.
        assert!(st.ended_grant_ids().unwrap().is_empty());
        st.revoke_grant(&gid, EndedBy::Owner).unwrap();
        index.ended(&gid).unwrap();
        assert!(!forget_purged(&index, &st), "its row is there: kept");
        // The poll that collects the tokens, racing the revocation, makes
        // them revoked.
        index
            .issued(
                &gid,
                [7; 32],
                &secret("cxg_r"),
                &secret("cxm_r"),
                &secret("cxc_r"),
            )
            .unwrap();
        assert!(index.lookup(&secret::sha256("cxg_r")).unwrap().revoked);
        // A denied request's record goes once its row is purged.
        let (gone, _) = request(&st, [8; 32]);
        st.deny_grant(&gone).unwrap();
        index.ended(&gone).unwrap();
        assert!(!forget_purged(&index, &st));
        clock.advance(clax_core::store::people::UNISSUED_RETENTION_SECS + 1);
        st.expire_grants().unwrap();
        assert!(forget_purged(&index, &st));
        assert_eq!(index.ended_grants(), vec![gid]);
    }

    #[test]
    fn a_grant_ended_before_its_tokens_are_recorded_keeps_them_revoked() {
        let index = GrantIndex::default();
        index.ended("g_1").unwrap();
        index
            .issued(
                "g_1",
                [2u8; 32],
                &secret("cxg_a"),
                &secret("cxm_a"),
                &secret("cxc_a"),
            )
            .unwrap();
        for t in ["cxg_a", "cxm_a", "cxc_a"] {
            assert!(index.lookup(&secret::sha256(t)).unwrap().revoked, "{t}");
        }
        assert_eq!(index.ended_grants(), vec!["g_1".to_string()]);
    }

    #[test]
    fn a_grant_ended_after_its_tokens_are_recorded_revokes_them() {
        let index = GrantIndex::default();
        index
            .issued(
                "g_1",
                [2u8; 32],
                &secret("cxg_a"),
                &secret("cxm_a"),
                &secret("cxc_a"),
            )
            .unwrap();
        index.ended("g_1").unwrap();
        for t in ["cxg_a", "cxm_a", "cxc_a"] {
            assert!(index.lookup(&secret::sha256(t)).unwrap().revoked, "{t}");
        }
        // A request that ended without tokens is forgotten once its row
        // is gone; a grant with tokens keeps its record.
        index.ended("g_2").unwrap();
        assert!(index.forget_ended(|_| Default::default()));
        assert_eq!(index.ended_grants(), vec!["g_1".to_string()]);
    }
}
