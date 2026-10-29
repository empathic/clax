//! The credentials of a report queue's own routes (spec
//! 2026-10-27-clax-sdk-design §5.3): a deployer's backend with its queue
//! service grant ([`ServiceCaller`]) and a reporter's page with a reporter
//! token ([`ReporterCaller`]). These routes are matched in front of every
//! other credential layer (`clax_server::router`), so these extractors are
//! the whole of their authentication.
//!
//! Each request carries one `Authorization: <scheme> <token>` and a
//! `Clax-Signature` (`clax-ed25519 v2`, [`clax_core::sig`]) over the same
//! canonical lines as an agent's signed request, the token's line holding
//! the SHA-256 of the token and the key being the token's own: the service
//! grant's, or the one the reporter token was minted for. The header
//! checks come first and spend nothing: the token is looked up, its queue
//! must be the path's, `t` must be within [`sig::MAX_SKEW_S`] and the nonce
//! unspent for its key. Then the body is read within the agent gate's
//! bounds ([`crate::agent::Gate`]), the signature verified, and only then
//! the nonce recorded ([`crate::grant_auth::GrantAuth::record_queue_nonce`]).
//!
//! A service request is server to server: one carrying an `Origin` or a
//! cookie is 403 `forbidden_origin`. A reporter request must carry exactly
//! one `Origin`, equal to the token's and still among the queue's origins
//! (403 `wrong_origin` otherwise, a missing one included). [`cors`] answers
//! a reporter route's cross-origin reads for the queue's origins alone.

use crate::agent;
use crate::error::ApiError;
use crate::grant_auth::{self, Refusal};
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, FromRequestParts, Path, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use clax_core::Secret;
use clax_core::sig::{self, SigError};
use clax_store::queues::{ReporterToken, ServiceGrant};

/// The `Authorization` scheme of a queue service grant's token (`cxq_…`).
pub const QUEUE_SCHEME: &str = "Clax-Queue";
/// The `Authorization` scheme of a reporter token (`cxr_…`).
pub const REPORTER_SCHEME: &str = "Clax-Reporter";
/// The most a queue route's body holds, but for filing a report: a
/// reporter token request or a comment (at most
/// [`clax_store::threads::MAX_BODY_CHARS`] characters) in JSON.
pub const SMALL_BODY_LIMIT: usize = 64 * 1024;
/// The most a report's multipart body (a filing or a reopen) holds: a
/// clip of at most [`clax_store::threads::MAX_CLIP_BYTES`], diagnostics of
/// at most [`clax_core::report::MAX_DIAG_BYTES`], a context as long as
/// [`clax_core::report::parse_context`] reads, and room for the text
/// fields (thread metadata at most
/// [`clax_core::queue::MAX_THREAD_METADATA_BYTES`]).
pub const REPORT_BODY_LIMIT: usize = clax_store::threads::MAX_CLIP_BYTES
    + clax_core::report::MAX_DIAG_BYTES
    + 4 * clax_core::report::MAX_CONTEXT_BYTES
    + 256 * 1024;
/// The request headers a reporter's page may send cross-origin.
pub const ALLOW_HEADERS: &str = "authorization, clax-signature, content-type";
/// The methods of the reporter routes.
pub const ALLOW_METHODS: &str = "GET, POST";
/// Seconds a browser may keep a preflight's answer.
const PREFLIGHT_MAX_AGE: &str = "600";

/// Whether the request carries a queue credential ([`QUEUE_SCHEME`] or
/// [`REPORTER_SCHEME`]), valid or not: one is accepted on the queue routes
/// alone, and refused everywhere else ([`refused_elsewhere`]).
pub fn carries_queue_credential(headers: &HeaderMap) -> bool {
    crate::auth::has_scheme(headers, QUEUE_SCHEME)
        || crate::auth::has_scheme(headers, REPORTER_SCHEME)
}

/// 401 `invalid_token` for a queue credential on a route that is not a
/// queue route: it never stands for anyone there.
pub fn refused_elsewhere() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_token",
        "a queue's service or reporter token is accepted only on that queue's reporter routes",
    )
}

fn invalid_token() -> Refusal {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_token",
        "that token is not one of this queue's",
    )
    .into()
}

fn wrong_origin() -> Refusal {
    ApiError::forbidden(
        "wrong_origin",
        "a reporter token is accepted only from the origin it was minted for",
    )
    .into()
}

/// The one `Authorization` value of the request, when its scheme is
/// `scheme` (ignoring case) and its token is not empty.
fn token_of(headers: &HeaderMap, scheme: &str) -> Result<Secret, Refusal> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(v), None) = (all.next(), all.next()) else {
        return Err(invalid_token());
    };
    let v = v.to_str().map_err(|_| invalid_token())?;
    let (s, token) = v.split_once(' ').ok_or_else(invalid_token)?;
    let token = token.trim_matches(' ');
    if !s.eq_ignore_ascii_case(scheme) || token.is_empty() {
        return Err(invalid_token());
    }
    Ok(Secret::new(token.to_string()))
}

/// The path's queue ID (`{qid}`).
async fn qid_of(parts: &mut Parts) -> Result<String, Refusal> {
    let Path(params) = Path::<Vec<(String, String)>>::from_request_parts(parts, &())
        .await
        .map_err(|_| invalid_token())?;
    params
        .into_iter()
        .find_map(|(k, v)| (k == "qid").then_some(v))
        .ok_or_else(invalid_token)
}

/// The header checks that spend nothing: `t` within the skew and the
/// nonce unspent for `key` of queue `qid`.
fn fresh(s: &AppState, qid: &str, head: &grant_auth::Head, key: &[u8; 32]) -> Result<(), Refusal> {
    let st = &s.store;
    let now = st.now_utc().timestamp();
    if head.header.t.abs_diff(now) > sig::MAX_SKEW_S.unsigned_abs() {
        return Err(grant_auth::refuse(SigError::Stale, st));
    }
    s.grant_auth
        .check_queue_nonce(qid, key, &head.header.nonce, head.header.t, now)
        .map_err(|e| grant_auth::refuse(e, st))
}

/// Reads the body (at most `limit` bytes, within the agent gate's bounds),
/// verifies the signature over it with `key` and `token`, and records the
/// nonce under queue `qid`.
async fn signed_body(
    s: &AppState,
    parts: &Parts,
    body: Body,
    qid: &str,
    head: &grant_auth::Head,
    key: [u8; 32],
    token: &Secret,
    limit: usize,
) -> Result<Bytes, Refusal> {
    let st = &s.store;
    let agent = grant_auth::agent_headers(&parts.headers, st)?;
    let body = if agent::bodyless(&parts.headers) {
        Bytes::new()
    } else {
        agent::read_unverified(&s.agent_gate, key, &parts.headers, body, limit).await?
    };
    let input = grant_auth::input(head, &body, Some(token.expose()), agent);
    if !sig::verify(&key, &input, &head.header.sig) {
        return Err(grant_auth::refuse(SigError::Bad, st));
    }
    let now = st.now_utc().timestamp();
    s.grant_auth
        .record_queue_nonce(qid, &key, &head.header.nonce, head.header.t, now)
        .map_err(|e| grant_auth::refuse(e, st))?;
    Ok(body)
}

/// A verified request of a queue's service grant (`Authorization:
/// Clax-Queue <token>`) on its own queue's route, with its body.
#[derive(Debug)]
pub struct ServiceCaller {
    pub grant: ServiceGrant,
    /// The verified body, at most [`SMALL_BODY_LIMIT`] bytes.
    pub body: Bytes,
}

impl FromRequest<AppState> for ServiceCaller {
    type Rejection = Refusal;

    /// # Errors
    /// 403 `forbidden_origin` for an `Origin` or a cookie; 401
    /// `invalid_token` for another credential, an unknown or revoked
    /// token, or another queue's; 401 `invalid_signature`, `stale_request`
    /// or `replayed_request`; 429 `limit_reached`; 413 `body_too_large`.
    async fn from_request(req: Request, s: &AppState) -> Result<Self, Refusal> {
        let (mut parts, body) = req.into_parts();
        grant_auth::no_browser(&parts.headers)?;
        let token = token_of(&parts.headers, QUEUE_SCHEME)?;
        let qid = qid_of(&mut parts).await?;
        let hash = token.sha256_hex();
        let grant = s
            .store_call(move |st| st.service_grant_by_token(&hash))
            .await?
            .filter(|g| g.queue_id == qid)
            .ok_or_else(invalid_token)?;
        let key =
            sig::decode_public(&grant.public_key).map_err(|e| grant_auth::refuse(e, &s.store))?;
        let head = grant_auth::head(&parts, &s.store)?;
        fresh(s, &grant.queue_id, &head, &key)?;
        let body = signed_body(
            s,
            &parts,
            body,
            &grant.queue_id,
            &head,
            key,
            &token,
            SMALL_BODY_LIMIT,
        )
        .await?;
        Ok(ServiceCaller { grant, body })
    }
}

/// A verified request of a reporter token (`Authorization: Clax-Reporter
/// <token>`) on its own queue's route, from its own origin, with its body
/// of at most `LIMIT` bytes.
#[derive(Debug)]
pub struct ReporterCaller<const LIMIT: usize = SMALL_BODY_LIMIT> {
    pub token: ReporterToken,
    /// The verified body.
    pub body: Bytes,
    /// The request's `Content-Type`, for reading the body.
    pub content_type: Option<HeaderValue>,
    extensions: axum::http::Extensions,
}

impl<const LIMIT: usize> ReporterCaller<LIMIT> {
    /// The request again, with its `Content-Type`, its extensions and the
    /// verified body, for an extractor that reads a body (`Multipart`).
    pub fn request(&self) -> Request {
        let mut req = Request::new(Body::from(self.body.clone()));
        if let Some(ct) = &self.content_type {
            req.headers_mut().insert(header::CONTENT_TYPE, ct.clone());
        }
        *req.extensions_mut() = self.extensions.clone();
        req
    }
}

impl<const LIMIT: usize> FromRequest<AppState> for ReporterCaller<LIMIT> {
    type Rejection = Refusal;

    /// # Errors
    /// 401 `invalid_token` for another credential, an unknown token, one
    /// whose grant was revoked, or another queue's; 401 `token_expired`;
    /// 403 `wrong_origin` for an `Origin` other than the token's, none, or
    /// one the queue no longer lists; 401 `invalid_signature`,
    /// `stale_request` or `replayed_request`; 429 `limit_reached`; 413
    /// `body_too_large`.
    async fn from_request(req: Request, s: &AppState) -> Result<Self, Refusal> {
        let (mut parts, body) = req.into_parts();
        let token = token_of(&parts.headers, REPORTER_SCHEME)?;
        let qid = qid_of(&mut parts).await?;
        let hash = token.sha256_hex();
        let found = s
            .store_call(move |st| {
                let Some(t) = st.reporter_by_token(&hash)? else {
                    return Ok(None);
                };
                if t.queue_id != qid {
                    return Ok(None);
                }
                let origins = st.queue(&qid)?.map(|q| q.origins).unwrap_or_default();
                Ok(Some((t, origins)))
            })
            .await?;
        let (t, origins) = found.ok_or_else(invalid_token)?;
        let mut sent = parts.headers.get_all(header::ORIGIN).iter();
        let origin = match (sent.next(), sent.next()) {
            (Some(o), None) => o.to_str().ok(),
            _ => None,
        };
        if origin != Some(t.origin.as_str()) || !origins.contains(&t.origin) {
            return Err(wrong_origin());
        }
        let key = sig::decode_public(&t.public_key).map_err(|e| grant_auth::refuse(e, &s.store))?;
        let head = grant_auth::head(&parts, &s.store)?;
        fresh(s, &t.queue_id, &head, &key)?;
        let content_type = parts.headers.get(header::CONTENT_TYPE).cloned();
        let body = signed_body(s, &parts, body, &t.queue_id, &head, key, &token, LIMIT).await?;
        Ok(ReporterCaller {
            token: t,
            body,
            content_type,
            extensions: parts.extensions,
        })
    }
}

/// The queue ID of a reporter route's path, `/api/queues/<qid>/reporter/…`.
fn reporter_qid(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/queues/")?;
    let (qid, rest) = rest.split_once('/')?;
    rest.starts_with("reporter/").then_some(qid)
}

/// Route layer of the reporter routes: CORS (spec §5.3). Every answer
/// varies on `Origin`. When the request's one `Origin` is among the path's
/// queue's origins, the answer names it in `Access-Control-Allow-Origin`
/// (never `*`, and no credentials mode), and a preflight's answer also
/// names [`ALLOW_METHODS`] and [`ALLOW_HEADERS`]; for any other origin it
/// names none, and the browser keeps the answer from the page.
pub async fn cors(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let mut sent = req.headers().get_all(header::ORIGIN).iter();
    let origin = match (sent.next(), sent.next()) {
        (Some(o), None) => o.to_str().ok().map(str::to_string),
        _ => None,
    };
    let qid = reporter_qid(req.uri().path()).map(str::to_string);
    let preflight = req.method() == Method::OPTIONS;
    let mut res = next.run(req).await;
    res.headers_mut()
        .append(header::VARY, HeaderValue::from_static("Origin"));
    let (Some(origin), Some(qid)) = (origin, qid) else {
        return res;
    };
    let listed = s
        .store_call(move |st| Ok(st.queue(&qid)?.map(|q| q.origins).unwrap_or_default()))
        .await
        .is_ok_and(|origins| origins.contains(&origin));
    if !listed {
        return res;
    }
    let Ok(value) = HeaderValue::from_str(&origin) else {
        return res;
    };
    let h = res.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    if preflight {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(ALLOW_METHODS),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOW_HEADERS),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static(PREFLIGHT_MAX_AGE),
        );
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporter_qid_reads_only_reporter_paths() {
        assert_eq!(reporter_qid("/api/queues/q1/reporter/reports"), Some("q1"));
        assert_eq!(
            reporter_qid("/api/queues/q1/reporter/reports/t1/clip"),
            Some("q1")
        );
        assert_eq!(reporter_qid("/api/queues/q1/reporter-tokens"), None);
        assert_eq!(reporter_qid("/api/queues/q1/reports"), None);
        assert_eq!(reporter_qid("/api/queues/"), None);
    }

    #[test]
    fn a_queue_credential_is_known_by_either_scheme() {
        let mut h = HeaderMap::new();
        assert!(!carries_queue_credential(&h));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        assert!(!carries_queue_credential(&h));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("clax-reporter cxr_x"),
        );
        assert!(carries_queue_credential(&h));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Clax-Queue cxq_x"),
        );
        assert!(carries_queue_credential(&h));
    }
}
