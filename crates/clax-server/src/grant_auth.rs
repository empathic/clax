//! Key-bound requests of the grant flow (spec §6.3, §6.4, §6.6): the
//! in-memory index of every grant token, the two nonce caches, and the
//! signature checks of `POST /api/grants` and of the continuation and
//! manage calls.
//!
//! A grant request carries no token: its key comes from its body, so it is
//! read (at most [`REQUEST_BODY_LIMIT`]) after `t` is tested, and its nonce
//! is checked, its signature verified and its nonce recorded under one lock
//! of a small cache of its own ([`GrantAuth::requests`]). Every other signed
//! call names its key through a token the [`GrantIndex`] knows, so the main
//! cache only ever holds keys of known grants: the token is looked up, the
//! nonce checked, the body read, the signature verified, and only then the
//! nonce recorded (with a second presence test), so a bad signature spends
//! nothing.

use crate::error::ApiError;
use crate::state::AppState;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::Secret;
use clax_core::sig::{self, NonceCache, SigError, SignInput};
use clax_core::store::people::CredentialKind;
use clax_core::{Store, secret};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// The most a grant-flow request's body may hold: the first request of a
/// grant has no token yet (spec §6.6), and the continuation and manage
/// calls carry little.
pub const REQUEST_BODY_LIMIT: usize = 16 * 1024;
/// Seconds a grant request's nonce is remembered.
const REQUEST_WINDOW_S: i64 = 240;
/// Grant-request nonces per key in a window: a client makes one request.
const REQUEST_NONCES_PER_KEY: usize = 16;
/// Keys and nonces the grant-request cache holds at once.
const REQUEST_KEYS: usize = 256;
const REQUEST_NONCES: usize = 4096;

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

/// Tokens by SHA-256.
type Tokens = HashMap<[u8; 32], GrantRef>;

/// Every grant token's SHA-256 with its grant (spec §6.4), loaded on start
/// and reloaded after every change. A reload reads the store after the
/// change committed; of two reloads, the one that started later wins, so
/// the index never goes back to an older state.
#[derive(Default)]
pub struct GrantIndex {
    map: RwLock<(u64, Arc<Tokens>)>,
    started: AtomicU64,
}

impl GrantIndex {
    /// The index of `st`'s tokens.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<GrantIndex> {
        let index = GrantIndex::default();
        index.reload(st)?;
        Ok(index)
    }

    /// The token whose SHA-256 is `hash`.
    pub fn lookup(&self, hash: &[u8; 32]) -> Option<GrantRef> {
        let map = self.map.read().unwrap_or_else(|e| e.into_inner()).1.clone();
        map.get(hash).cloned()
    }

    /// Reads every token from `st` again.
    ///
    /// # Errors
    /// The store's; the index keeps what it had.
    pub fn reload(&self, st: &Store) -> clax_core::Result<()> {
        let started = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        let fresh: HashMap<[u8; 32], GrantRef> = st
            .credential_index()?
            .into_iter()
            .map(|c| {
                (
                    c.sha256,
                    GrantRef {
                        grant_id: c.grant_id,
                        kind: c.kind,
                        key: c.key,
                        revoked: c.revoked,
                    },
                )
            })
            .collect();
        self.install(started, fresh);
        Ok(())
    }

    /// Installs `fresh`, read by the reload numbered `started`, unless a
    /// reload that started later installed its own already.
    fn install(&self, started: u64, fresh: HashMap<[u8; 32], GrantRef>) {
        let mut g = self.map.write().unwrap_or_else(|e| e.into_inner());
        if started > g.0 {
            *g = (started, Arc::new(fresh));
        }
    }
}

/// The grant flow's signature state: the token index and the nonce caches.
pub struct GrantAuth {
    pub index: GrantIndex,
    /// Nonces of grant requests, whose key comes from the body.
    requests: Mutex<NonceCache>,
    /// Nonces of keys known through [`GrantAuth::index`].
    nonces: Mutex<NonceCache>,
}

impl GrantAuth {
    /// The state for `st`'s grants, with empty caches.
    ///
    /// # Errors
    /// The store's.
    pub fn load(st: &Store) -> clax_core::Result<GrantAuth> {
        Ok(GrantAuth {
            index: GrantIndex::load(st)?,
            requests: Mutex::new(
                NonceCache::new(REQUEST_WINDOW_S, REQUEST_NONCES_PER_KEY)
                    .with_caps(REQUEST_KEYS, REQUEST_NONCES),
            ),
            nonces: Mutex::new(NonceCache::new(sig::NONCE_WINDOW_S, sig::NONCES_PER_KEY)),
        })
    }

    /// Forgets the nonces of both caches older than their window.
    pub fn sweep(&self, now: i64) {
        lock(&self.requests).sweep(now);
        lock(&self.nonces).sweep(now);
    }

    /// Keys with nonces remembered: (grant requests, the rest).
    pub fn cached_keys(&self) -> (usize, usize) {
        (lock(&self.requests).keys(), lock(&self.nonces).keys())
    }
}

fn lock(m: &Mutex<NonceCache>) -> std::sync::MutexGuard<'_, NonceCache> {
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

fn refuse(e: SigError, st: &Store) -> Refusal {
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

fn invalid_token() -> Refusal {
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
    /// The peer it came from.
    pub peer_ip: String,
}

/// The parts of a request that are signed, before its body is read.
struct Head {
    method: String,
    path_and_query: String,
    host: String,
    header: sig::SigHeader,
    peer_ip: String,
}

/// 403 `forbidden_origin` for a browser's request: an agent credential
/// never comes with an `Origin` or a cookie (spec §6.6).
fn no_browser(headers: &HeaderMap) -> Result<(), Refusal> {
    if headers.contains_key(header::ORIGIN) || headers.contains_key(header::COOKIE) {
        return Err(ApiError::forbidden(
            "forbidden_origin",
            "a signed request carries no Origin and no cookie",
        )
        .into());
    }
    Ok(())
}

fn head(parts: &axum::http::request::Parts, st: &Store) -> Result<Head, Refusal> {
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

fn input<'a>(h: &'a Head, body: &[u8], token: Option<&str>) -> SignInput<'a> {
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
/// [`REQUEST_BODY_LIMIT`]), whose key `key_of` reads; the nonce checked,
/// the signature verified over an empty token line, and the nonce recorded,
/// under one lock of the grant-request cache.
///
/// # Errors
/// 403 `forbidden_origin`, 401 `invalid_signature`, `stale_request` or
/// `replayed_request`, 413 `body_too_large`, 429 `limit_reached`, or what
/// `key_of` refuses the body with.
pub async fn grant_request(
    s: &AppState,
    req: axum::extract::Request,
    key_of: impl FnOnce(&[u8]) -> Result<[u8; 32], ApiError>,
) -> Result<Signed, Refusal> {
    let st = &s.store;
    let (parts, body) = req.into_parts();
    no_browser(&parts.headers)?;
    let h = head(&parts, st)?;
    let now = st.now_utc().timestamp();
    let skew = lock(&s.grant_auth.requests).skew();
    if h.header.t.abs_diff(now) > skew.unsigned_abs() {
        return Err(refuse(SigError::Stale, st));
    }
    let body = read_body(&parts.headers, body).await?;
    let key = key_of(&body)?;
    let now = st.now_utc().timestamp();
    {
        let mut c = lock(&s.grant_auth.requests);
        c.check(&key, &h.header.nonce, h.header.t, now)
            .map_err(|e| refuse(e, st))?;
        if !sig::verify(&key, &input(&h, &body, None), &h.header.sig) {
            return Err(refuse(SigError::Bad, st));
        }
        c.record(&key, &h.header.nonce, h.header.t, now)
            .map_err(|e| refuse(e, st))?;
    }
    Ok(Signed {
        body,
        host: h.host,
        key,
        token: None,
        peer_ip: h.peer_ip,
    })
}

/// Verifies a continuation or manage call on grant `gid` (spec §4.2,
/// §6.4, §6.6): `Authorization: GNAP <token>` naming a token of kind
/// `kind` of that grant in the index, then the signature header, `t` and
/// the nonce's absence, all before the body is read; then the body, the
/// signature over the token, and the nonce's record.
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
    let found = s
        .grant_auth
        .index
        .lookup(&secret::sha256(token.expose()))
        .filter(|g| g.kind == kind && g.grant_id == gid)
        .ok_or_else(invalid_token)?;
    if found.revoked {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "grant_revoked",
            "this machine's grant was revoked: connect again",
        )
        .into());
    }
    let h = head(&parts, st)?;
    lock(&s.grant_auth.nonces)
        .check(
            &found.key,
            &h.header.nonce,
            h.header.t,
            st.now_utc().timestamp(),
        )
        .map_err(|e| refuse(e, st))?;
    let body = read_body(&parts.headers, body).await?;
    if !sig::verify(
        &found.key,
        &input(&h, &body, Some(token.expose())),
        &h.header.sig,
    ) {
        return Err(refuse(SigError::Bad, st));
    }
    lock(&s.grant_auth.nonces)
        .record(
            &found.key,
            &h.header.nonce,
            h.header.t,
            st.now_utc().timestamp(),
        )
        .map_err(|e| refuse(e, st))?;
    Ok(Signed {
        body,
        host: h.host,
        key: found.key,
        token: Some(token),
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

    fn one(kind: CredentialKind) -> HashMap<[u8; 32], GrantRef> {
        HashMap::from([(
            [1u8; 32],
            GrantRef {
                grant_id: "g_1".into(),
                kind,
                key: [2u8; 32],
                revoked: false,
            },
        )])
    }

    #[test]
    fn a_reload_that_started_earlier_never_replaces_a_later_one() {
        let index = GrantIndex::default();
        index.install(2, one(CredentialKind::Manage));
        index.install(1, one(CredentialKind::Access));
        assert_eq!(
            index.lookup(&[1u8; 32]).map(|g| g.kind),
            Some(CredentialKind::Manage)
        );
        index.install(3, HashMap::new());
        assert_eq!(index.lookup(&[1u8; 32]), None);
    }
}
