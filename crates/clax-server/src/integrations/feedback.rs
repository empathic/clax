//! The instance as an integrator of its own report queues (spec
//! 2026-10-29-feedback-loop-design §10): its people report problems with
//! Clax itself the way any product's users report problems with it. This
//! module is the backend half an outside product writes, and uses only
//! what such a product has: a queue's ID, a service grant's token and key
//! (`POST /api/queues/<qid>/service-grants`), the public
//! `POST /api/queues/<qid>/reporter-tokens` route and its signature
//! (`clax-ed25519 v2`, [`clax_core::sig`]). It sends that route the very
//! request an outside backend sends over HTTP ([`mint_request`]), through
//! the instance's own router in process ([`mint`]).
//!
//! The product's own routes, behind its sign-in (the instance's session):
//!
//! - `POST /_app/feedback/token {public_key}` (same origin) → `{token,
//!   expires_at}`: a reporter token for the signed-in person, bound to the
//!   browser's Ed25519 `public_key` (unpadded base64url) and to the
//!   integration's origin; `reporter_id` is the person's public ID and the
//!   profile `{name?, github?}` comes from their GitHub account (their
//!   name as Clax shows it when they have none). 404 when the integration
//!   is off; 403 `no_flag` without the person's `feedback` flag; 400 as
//!   the public route answers a bad `public_key`; 503
//!   `feedback_unavailable` when the public route refuses the integration
//!   itself (a revoked grant, an origin the queue no longer lists).
//! - `GET /_app/feedback/config` → what the shell mounts the overlay with
//!   ([`config`]).
//!
//! Who files is the product's choice, made here as any backend makes it:
//! the owner and connected people whose `feedback` flag is on. Agents are
//! refused (403 `not_for_agents`).

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use clax_core::sig::{self, KeyPair, SignInput};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::Caller;
use clax_http::error::ApiError;
use clax_http::state::{AppState, FeedbackIntegration};
use clax_http::viewer::SameOrigin;
use clax_store::Store;
use clax_store::roles::OWNER;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;
use std::convert::Infallible;
use tower::ServiceExt as _;

/// clax.dev's origin: reports filed from it are the `prod` channel's,
/// from anywhere else `dev`'s.
pub const PROD_ORIGIN: &str = "https://clax.dev";
/// The repository Clax is built from, which a report's context names
/// (`app.repo`) so triage can link its commit.
pub const REPOSITORY: &str = "https://github.com/empathic/clax";
/// The most a profile field holds, in bytes (spec §5.4).
const PROFILE_FIELD_BYTES: usize = 256;
/// The most of the public route's answer read.
const ANSWER_LIMIT: usize = 64 * 1024;

/// A queue service grant's credential as the integration keeps it (a
/// file beside `config.toml`, or the Worker secret
/// `CLAX_FEEDBACK_SERVICE_GRANT`): `POST
/// /api/queues/<qid>/service-grants`'s `token` and `private_key`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    token: String,
    /// The key's PKCS#8 v2, unpadded base64url.
    private_key_pkcs8: String,
}

/// The integration that files into queue `queue_id` with the service grant
/// in `grant_json` (`{token, private_key_pkcs8}`), minting reporter tokens
/// for `origin`.
///
/// # Errors
/// A message naming what is wrong: an empty or unsafe queue ID, a grant
/// that is not that JSON, an empty token, or a key that is not an Ed25519
/// PKCS#8 one.
pub fn integration(
    queue_id: &str,
    origin: &str,
    grant_json: &str,
) -> Result<FeedbackIntegration, String> {
    let queue_id = queue_id.trim();
    if queue_id.is_empty()
        || !queue_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(format!("'{queue_id}' is not a queue ID"));
    }
    let g: GrantFile = serde_json::from_str(grant_json)
        .map_err(|e| format!("the service grant is not {{token, private_key_pkcs8}}: {e}"))?;
    let token = g.token.trim();
    if token.is_empty() {
        return Err("the service grant's token is empty".into());
    }
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(g.private_key_pkcs8.trim().trim_end_matches('='))
        .map_err(|_| "the service grant's private_key_pkcs8 is not base64url".to_string())?;
    KeyPair::from_pkcs8(&key)
        .map_err(|_| "the service grant's private_key_pkcs8 is not an Ed25519 PKCS#8 key")?;
    Ok(FeedbackIntegration {
        queue_id: queue_id.to_string(),
        grant_token: token.to_string(),
        grant_key_pkcs8: key,
        origin: origin.trim_end_matches('/').to_string(),
    })
}

/// Who the backend says a reporter is (spec §5.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
}

#[derive(Serialize)]
struct MintBody<'a> {
    reporter_id: &'a str,
    origin: &'a str,
    public_key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a Profile>,
}

/// The `Host` of `origin` (`clax.dev`, `localhost:7481`), which the
/// request names and signs.
fn host_of(origin: &str) -> Result<String, String> {
    let u = url::Url::parse(origin).map_err(|_| format!("'{origin}' is not an origin"))?;
    let host = u
        .host_str()
        .ok_or_else(|| format!("'{origin}' has no host"))?;
    Ok(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    })
}

/// The request an integrator's backend sends to mint a reporter token for
/// `reporter_id`, bound to the browser's `public_key`, signed at `now`
/// (Unix seconds): `POST /api/queues/<qid>/reporter-tokens` with
/// `Authorization: Clax-Queue <token>` and a `Clax-Signature` by the
/// service grant's key over the canonical lines (method, path and query,
/// the instance's host, `now`, a fresh nonce, the body's and the token's
/// SHA-256, no agent headers). Over HTTP the same request goes to the
/// instance's origin; its URI here is the path alone.
///
/// # Errors
/// An origin with no host, or a key that does not sign.
pub fn mint_request(
    i: &FeedbackIntegration,
    now: i64,
    reporter_id: &str,
    public_key: &str,
    profile: Option<&Profile>,
) -> Result<Request<Body>, String> {
    let key = KeyPair::from_pkcs8(&i.grant_key_pkcs8)
        .map_err(|_| "the service grant's key is not an Ed25519 PKCS#8 key".to_string())?;
    let path = format!("/api/queues/{}/reporter-tokens", i.queue_id);
    let host = host_of(&i.origin)?;
    let body = serde_json::to_vec(&MintBody {
        reporter_id,
        origin: &i.origin,
        public_key,
        profile,
    })
    .expect("a mint body serialises");
    let nonce = sig::new_nonce();
    let signature = key
        .sign(&SignInput {
            method: "POST",
            path_and_query: &path,
            host: &host,
            t: now,
            nonce: &nonce,
            body_sha256: sha2::Sha256::digest(&body).into(),
            token_sha256: Some(clax_core::secret::sha256(&i.grant_token)),
            agent_headers_sha256: sig::no_agent_headers_sha256(),
        })
        .map_err(|e| format!("signing the request: {e}"))?
        .to_string();
    Request::builder()
        .method("POST")
        .uri(&path)
        .header(header::HOST, host)
        // The service grant's credential, on the request this backend sends
        // (no handler here reads a caller's credential: who calls is the
        // session's `Caller`).
        .header("authorization", format!("Clax-Queue {}", i.grant_token))
        .header("clax-signature", signature)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .map_err(|e| e.to_string())
}

/// What the public route answered a mint.
#[derive(Debug)]
pub enum Minted {
    /// `{token, expires_at}`.
    Token { token: String, expires_at: String },
    /// A refusal: its status and its body (`{error: {code, message}}`).
    Refused { status: StatusCode, body: Value },
}

/// Sends `req` ([`mint_request`]) to `instance` and reads its answer.
///
/// # Errors
/// An answer that is not the route's.
pub async fn mint<S>(instance: S, req: Request<Body>) -> Result<Minted, String>
where
    S: tower::Service<Request<Body>, Response = Response, Error = Infallible>,
{
    let res = match instance.oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), ANSWER_LIMIT)
        .await
        .map_err(|e| format!("reading the answer: {e}"))?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        return Ok(Minted::Refused { status, body });
    }
    match (body["token"].as_str(), body["expires_at"].as_str()) {
        (Some(t), Some(e)) => Ok(Minted::Token {
            token: t.to_string(),
            expires_at: e.to_string(),
        }),
        _ => Err(format!("the answer is not {{token, expires_at}}: {body}")),
    }
}

/// The person key of a caller who may file: the owner, or a connected
/// person.
///
/// # Errors
/// 401 without a credential; 403 `not_for_agents` for an agent; 403
/// `not_a_person` for a viewer who is no connected person.
fn filer(caller: &Caller) -> Result<String, ApiError> {
    match caller {
        Caller::Owner { .. } => Ok(OWNER.to_string()),
        Caller::Person { viewer } => Ok(viewer.public_id.clone()),
        Caller::Anonymous => Err(ApiError::unauthorized()),
        Caller::OwnerAgent { .. } | Caller::PersonAgent { .. } => {
            Err(clax_http::auth::not_for_agents())
        }
        Caller::Viewer { .. } => Err(ApiError::forbidden(
            "not_a_person",
            "only the owner and connected people report problems with Clax",
        )),
    }
}

/// `s` cut to at most [`PROFILE_FIELD_BYTES`] bytes, on a character
/// boundary, or `None` when it is blank.
fn field(s: Option<&str>) -> Option<String> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    let mut end = s.len().min(PROFILE_FIELD_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].to_string())
}

/// The reporter `person` (a person key) files as: whether their
/// `feedback` flag is on, their public ID, and their profile.
fn reporter(st: &Store, person: &str) -> clax_core::Result<(bool, String, Option<Profile>)> {
    let may_file = st.flags_of(person)?.feedback;
    let public_id = if person == OWNER {
        st.owner_viewer(false)?.public_id
    } else {
        person.to_string()
    };
    let shown = st
        .viewer_by_public_id(&public_id)?
        .and_then(|v| v.display_name);
    let github = st.github_user_of(&public_id)?;
    let profile = Profile {
        name: field(github.as_ref().and_then(|g| g.name.as_deref())).or(field(shown.as_deref())),
        github: field(github.as_ref().map(|g| g.login.as_str())),
    };
    let profile = (profile != Profile::default()).then_some(profile);
    Ok((may_file, public_id, profile))
}

fn unavailable(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "feedback_unavailable",
        message,
    )
}

/// `POST /_app/feedback/token` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBody {
    public_key: String,
}

/// `POST /_app/feedback/token {public_key}` (see the module
/// documentation).
pub async fn token(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    req: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let Some(i) = s.feedback.clone() else {
        return Err(ApiError::not_found());
    };
    let person = filer(&d.caller(&s).await?)?;
    let b = crate::routes::artifacts::body(req)?;
    let (may_file, reporter_id, profile) = s.store_call(move |st| reporter(st, &person)).await?;
    if !may_file {
        return Err(ApiError::forbidden(
            "no_flag",
            "reporting problems with Clax is not turned on for you: an admin turns on your feedback flag",
        ));
    }
    let now = s.store.now_utc().timestamp();
    let request = mint_request(&i, now, &reporter_id, &b.public_key, profile.as_ref())
        .map_err(unavailable)?;
    let instance = crate::routes::queue_reporter::router(s.clone());
    match mint(instance, request).await.map_err(unavailable)? {
        Minted::Token { token, expires_at } => {
            let mut res = Json(json!({"token": token, "expires_at": expires_at})).into_response();
            res.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(res)
        }
        // The browser's key: what the route says of it is the browser's to hear.
        Minted::Refused { status, body } if status == StatusCode::BAD_REQUEST => {
            Ok((status, Json(body)).into_response())
        }
        Minted::Refused { status, body } => {
            let code = body["error"]["code"].as_str().unwrap_or("unknown");
            tracing::warn!(%status, code, queue = %i.queue_id, "the feedback queue refused the instance's service grant");
            Err(unavailable(format!(
                "the feedback queue refused this instance's service grant ({status} {code}): an admin checks [feedback] or CLAX_FEEDBACK_SERVICE_GRANT"
            )))
        }
    }
}

/// The `channel` reports filed for `origin` carry: `prod` from
/// [`PROD_ORIGIN`], `dev` from anywhere else.
pub fn channel(origin: &str) -> &'static str {
    if origin == PROD_ORIGIN { "prod" } else { "dev" }
}

/// `GET /_app/feedback/config`: what the shell mounts the overlay with.
/// For a caller who may file (the integration on, the owner or a connected
/// person, their `feedback` flag on):
///
/// ```json
/// {"enabled": true, "instance": "<origin>", "queue": "<queue ID>",
///  "context": {"release": {"version": "…", "commit": "…", "id": "<build>", "channel": "prod"|"dev"},
///              "flags": {"claxbot": true, "claxbot_unenforced": false, "feedback": true},
///              "app": {"repo": "https://github.com/empathic/clax"}}}
/// ```
///
/// `release.commit` and `release.id` are left out when `/healthz` reports
/// them `null`. For anyone else (the integration off, no credential, a
/// viewer, the flag off) `{"enabled": false, "instance": null, "queue":
/// null, "context": null}`. Agents are refused 403 `not_for_agents`.
pub async fn config(State(s): State<AppState>, d: DeniedToGrants) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let off = Json(json!({"enabled": false, "instance": null, "queue": null, "context": null}));
    let Some(i) = s.feedback.clone() else {
        return Ok(off);
    };
    let Ok(person) = filer(&d.caller(&s).await?) else {
        return Ok(off);
    };
    let flags = s.store_call(move |st| st.flags_of(&person)).await?;
    if !flags.feedback {
        return Ok(off);
    }
    let (commit, build) = crate::routes::health::stamped(&s);
    let mut release = json!({"version": s.version, "channel": channel(&i.origin)});
    if let Some(c) = commit {
        release["commit"] = json!(c);
    }
    if let Some(b) = build {
        release["id"] = json!(b);
    }
    Ok(Json(json!({
        "enabled": true,
        "instance": i.origin,
        "queue": i.queue_id,
        "context": {
            "release": release,
            "flags": flags,
            "app": {"repo": REPOSITORY},
        },
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant_json(key: &KeyPair) -> String {
        let k = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.pkcs8().expose());
        json!({"token": "cxq_t", "private_key_pkcs8": k}).to_string()
    }

    #[test]
    fn an_integration_takes_the_grant_as_its_route_answers_it() {
        let key = KeyPair::generate();
        let i = integration("q1", "https://clax.dev/", &grant_json(&key)).unwrap();
        assert_eq!(
            (
                i.queue_id.as_str(),
                i.grant_token.as_str(),
                i.origin.as_str()
            ),
            ("q1", "cxq_t", "https://clax.dev")
        );
        assert_eq!(&i.grant_key_pkcs8[..], key.pkcs8().expose());
        assert!(!format!("{i:?}").contains("cxq_t"));
        for (q, g) in [
            ("", grant_json(&key)),
            ("q/../x", grant_json(&key)),
            ("q1", "{}".into()),
            (
                "q1",
                json!({"token": "", "private_key_pkcs8": "AA"}).to_string(),
            ),
            (
                "q1",
                json!({"token": "t", "private_key_pkcs8": "AA"}).to_string(),
            ),
            (
                "q1",
                json!({"token": "t", "private_key_pkcs8": "!"}).to_string(),
            ),
        ] {
            assert!(integration(q, "https://clax.dev", &g).is_err(), "{q} {g}");
        }
    }

    #[test]
    fn the_mint_request_is_the_signed_one_an_outside_backend_sends() {
        let key = KeyPair::generate();
        let i = integration("q1", "http://localhost:7481", &grant_json(&key)).unwrap();
        let p = Profile {
            name: Some("Ana".into()),
            github: Some("ana".into()),
        };
        let req = mint_request(&i, 1_000, "pub1", "pk", Some(&p)).unwrap();
        assert_eq!(req.method(), "POST");
        assert_eq!(req.uri(), "/api/queues/q1/reporter-tokens");
        let h = req.headers();
        assert_eq!(h[header::HOST], "localhost:7481");
        assert_eq!(h["authorization"], "Clax-Queue cxq_t");
        assert!(!h.contains_key(header::ORIGIN) && !h.contains_key(header::COOKIE));
        let parsed = sig::parse_header(h["clax-signature"].as_bytes()).unwrap();
        assert_eq!(parsed.t, 1_000);
        assert_eq!(channel("https://clax.dev"), "prod");
        assert_eq!(channel("http://localhost:7481"), "dev");
        assert_eq!(field(Some(&"é".repeat(200))).unwrap().len(), 256);
        assert_eq!(field(Some("  ")), None);
    }
}
