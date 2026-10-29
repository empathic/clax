//! The instance as an integrator of its own report queues (spec
//! 2026-10-29-feedback-loop-design §10): its people report problems with
//! Clax itself the way any product's users report problems with it.
//!
//! The module has two halves.
//!
//! **The backend half** ([`integration`], [`mint_request`], [`mint`]) is
//! what an outside product's backend writes, and uses only what such a
//! product has: a queue's ID, a service grant's token and key (`POST
//! /api/queues/<qid>/service-grants`), and the public `POST
//! /api/queues/<qid>/reporter-tokens` route with its signature
//! (`clax-ed25519 v2`, [`clax_core::sig`]). [`mint_request`] builds the
//! very request an outside backend sends over HTTP; [`mint`] sends it to
//! any `tower::Service`, here the instance's whole router in process.
//!
//! **The product's routes** ([`token`], [`config`]), behind its sign-in
//! (the instance's session), ask the instance only for what
//! [`super::instance::Instance`] gives:
//!
//! - `POST /_app/feedback/token {public_key}` (same origin) → `{token,
//!   expires_at}`: a reporter token for the signed-in person, bound to the
//!   browser's Ed25519 `public_key` (unpadded base64url) and to the
//!   integration's origin; `reporter_id` is the person's public ID and the
//!   profile `{name?, github?}` comes from their GitHub account (their
//!   name as Clax shows it when they have none). 404 when the integration
//!   is off; 403 `no_flag` without the person's `feedback` flag; 400 as
//!   the public route answers a bad `public_key`; 429 `limit_reached` as
//!   it answers one (with its `Retry-After`); 503 `feedback_unavailable`
//!   for any other refusal, which is the integration's own (a revoked
//!   grant, an origin the queue no longer lists), logged.
//! - `GET /_app/feedback/config` → what the shell mounts the overlay with
//!   ([`config`]).
//!
//! Who files is the product's choice, made here as any backend makes it:
//! the owner and connected people whose `feedback` flag is on. Agents are
//! refused (403 `not_for_agents`).

use super::instance::{Instance, json_body};
use axum::Json;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
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
use std::sync::Arc;
use tower::ServiceExt as _;

// ---------------------------------------------------------------------------
// The backend half: what any product's backend writes.
// ---------------------------------------------------------------------------

/// The most of the public route's answer read.
const ANSWER_LIMIT: usize = 64 * 1024;

/// A queue service grant's credential as the integration keeps it (a
/// file, or a secret): `POST /api/queues/<qid>/service-grants`'s `token`
/// and `private_key`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    token: String,
    /// The key's PKCS#8 v2, unpadded base64url.
    private_key_pkcs8: String,
}

/// Which input of [`integration`] is wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The queue ID.
    Queue,
    /// The service grant.
    Grant,
}

/// Why [`integration`] refused its inputs. The message never repeats
/// them: a grant is a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub input: Input,
    pub message: &'static str,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

/// The integration that files into queue `queue_id` on the instance at
/// `instance` (its base URL) with the service grant in `grant_json`
/// (`{token, private_key_pkcs8}`), minting reporter tokens for pages of
/// `origin`. For clax.dev's own shell the two are the same; an outside
/// product's pages have an origin of their own.
///
/// # Errors
/// [`Invalid`], naming the input: a queue ID that is empty or not of
/// letters, digits, `-` and `_`; a grant that is not that JSON, has an
/// empty token, or a key that is not an Ed25519 PKCS#8 one in base64url.
pub fn integration(
    queue_id: &str,
    instance: &str,
    origin: &str,
    grant_json: &str,
) -> Result<FeedbackIntegration, Invalid> {
    let bad = |input, message| Invalid { input, message };
    let queue_id = queue_id.trim();
    if queue_id.is_empty()
        || !queue_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(bad(Input::Queue, "the queue ID is not one"));
    }
    let g: GrantFile = serde_json::from_str(grant_json).map_err(|_| {
        bad(
            Input::Grant,
            "the service grant is not the JSON {token, private_key_pkcs8}",
        )
    })?;
    let token = g.token.trim();
    if token.is_empty() {
        return Err(bad(Input::Grant, "the service grant's token is empty"));
    }
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(g.private_key_pkcs8.trim().trim_end_matches('='))
        .map_err(|_| {
            bad(
                Input::Grant,
                "the service grant's private_key_pkcs8 is not base64url",
            )
        })?;
    KeyPair::from_pkcs8(&key).map_err(|_| {
        bad(
            Input::Grant,
            "the service grant's private_key_pkcs8 is not an Ed25519 PKCS#8 key",
        )
    })?;
    Ok(FeedbackIntegration {
        queue_id: queue_id.to_string(),
        grant_token: token.to_string(),
        grant_key_pkcs8: key,
        instance: instance.trim_end_matches('/').to_string(),
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
/// SHA-256, no agent headers). It is signed for the instance's host
/// (`i.instance`), while its body names the page origin the token is for
/// (`i.origin`). Over HTTP the same request goes to `<instance><path>`;
/// its URI here is the path alone.
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
    let host = host_of(&i.instance)?;
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
    /// A refusal: its status, its `Retry-After` if any, and its body
    /// (`{error: {code, message}}`).
    Refused {
        status: StatusCode,
        retry_after: Option<HeaderValue>,
        body: Value,
    },
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
    let retry_after = res.headers().get(header::RETRY_AFTER).cloned();
    let bytes = axum::body::to_bytes(res.into_body(), ANSWER_LIMIT)
        .await
        .map_err(|e| format!("reading the answer: {e}"))?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        return Ok(Minted::Refused {
            status,
            retry_after,
            body,
        });
    }
    match (body["token"].as_str(), body["expires_at"].as_str()) {
        (Some(t), Some(e)) => Ok(Minted::Token {
            token: t.to_string(),
            expires_at: e.to_string(),
        }),
        _ => Err("the answer is not {token, expires_at}".to_string()),
    }
}

// ---------------------------------------------------------------------------
// The product's routes: clax.dev's sign-in, flags and shell.
// ---------------------------------------------------------------------------

/// clax.dev's origin: reports filed from it are the `prod` channel's,
/// from anywhere else `dev`'s.
pub const PROD_ORIGIN: &str = "https://clax.dev";
/// The repository Clax is built from, which a report's context names
/// (`app.repo`) so triage can link its commit.
pub const REPOSITORY: &str = "https://github.com/empathic/clax";
/// The most a profile field holds, in bytes (spec §5.4).
const PROFILE_FIELD_BYTES: usize = 256;

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

/// Whether a 400 of the public route is about the browser's `public_key`,
/// the one input of the mint the browser chose: `invalid_key`, or
/// `invalid_args` naming `public_key`.
fn about_the_key(body: &Value) -> bool {
    let e = &body["error"];
    match e["code"].as_str() {
        Some("invalid_key") => true,
        Some("invalid_args") => e["message"]
            .as_str()
            .is_some_and(|m| m.contains("public_key")),
        _ => false,
    }
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
    Extension(instance): Extension<Arc<Instance>>,
    d: DeniedToGrants,
    _o: SameOrigin,
    req: Result<Json<TokenBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let Some(i) = s.feedback.clone() else {
        return Err(ApiError::not_found());
    };
    let person = filer(&d.caller(&s).await?)?;
    let b = json_body(req)?;
    let (may_file, reporter_id, profile) = s.store_call(move |st| reporter(st, &person)).await?;
    if !may_file {
        return Err(ApiError::forbidden(
            "no_flag",
            "reporting problems with Clax is not turned on for you: an admin turns on your feedback flag",
        ));
    }
    let request = mint_request(
        &i,
        instance.now(),
        &reporter_id,
        &b.public_key,
        profile.as_ref(),
    )
    .map_err(unavailable)?;
    match mint(instance.router(), request)
        .await
        .map_err(unavailable)?
    {
        Minted::Token { token, expires_at } => {
            let mut res = Json(json!({"token": token, "expires_at": expires_at})).into_response();
            res.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(res)
        }
        // What the route says of the browser's key, or of its rate, is the
        // browser's to hear.
        Minted::Refused {
            status,
            retry_after,
            body,
        } if (status == StatusCode::BAD_REQUEST && about_the_key(&body))
            || status == StatusCode::TOO_MANY_REQUESTS =>
        {
            let mut headers = HeaderMap::new();
            if let Some(r) = retry_after {
                headers.insert(header::RETRY_AFTER, r);
            }
            Ok((status, headers, Json(body)).into_response())
        }
        Minted::Refused { status, body, .. } => {
            let code = body["error"]["code"].as_str().unwrap_or("unknown");
            tracing::warn!(%status, code, queue = %i.queue_id, "the feedback queue refused the instance's mint");
            Err(unavailable(format!(
                "the feedback queue refused this instance's mint ({status} {code}): an admin checks [feedback] or CLAX_FEEDBACK_SERVICE_GRANT"
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
pub async fn config(
    State(s): State<AppState>,
    Extension(instance): Extension<Arc<Instance>>,
    d: DeniedToGrants,
) -> Result<Json<Value>, ApiError> {
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
    let mut release = instance.release();
    release["channel"] = json!(channel(&i.origin));
    Ok(Json(json!({
        "enabled": true,
        "instance": i.instance,
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
        let i = integration(
            "q1",
            "https://clax.dev/",
            "https://clax.dev/",
            &grant_json(&key),
        )
        .unwrap();
        assert_eq!(
            (
                i.queue_id.as_str(),
                i.grant_token.as_str(),
                i.instance.as_str(),
                i.origin.as_str()
            ),
            ("q1", "cxq_t", "https://clax.dev", "https://clax.dev")
        );
        assert_eq!(&i.grant_key_pkcs8[..], key.pkcs8().expose());
        assert!(!format!("{i:?}").contains("cxq_t"));
        for (q, g, input) in [
            ("", grant_json(&key), Input::Queue),
            ("q/../x", grant_json(&key), Input::Queue),
            ("q1", "{}".into(), Input::Grant),
            (
                "q1",
                json!({"token": "", "private_key_pkcs8": "AA"}).to_string(),
                Input::Grant,
            ),
            (
                "q1",
                json!({"token": "t", "private_key_pkcs8": "AA"}).to_string(),
                Input::Grant,
            ),
            (
                "q1",
                json!({"token": "t", "private_key_pkcs8": "!"}).to_string(),
                Input::Grant,
            ),
        ] {
            let e = integration(q, "https://clax.dev", "https://clax.dev", &g).unwrap_err();
            assert_eq!(e.input, input, "{q} {g}");
        }
    }

    #[test]
    fn a_refused_grant_is_never_repeated_in_the_message() {
        let secret = "cxq_SECRET_TOKEN_VALUE";
        for g in [
            json!({"token": secret, "private_key_pkcs8": 7}).to_string(),
            json!({"token": secret, "private_key_pkcs8": secret, "x": secret}).to_string(),
            json!({"token": secret, "private_key_pkcs8": secret}).to_string(),
            format!("{{\"token\": \"{secret}\""),
        ] {
            let e = integration("q1", "https://clax.dev", "https://clax.dev", &g).unwrap_err();
            assert_eq!(e.input, Input::Grant);
            assert!(!e.to_string().contains("SECRET"), "{e}");
        }
    }

    #[test]
    fn only_a_400_about_the_browsers_key_is_the_browsers() {
        let err = |code: &str, message: &str| json!({"error": {"code": code, "message": message}});
        assert!(about_the_key(&err(
            "invalid_key",
            "public_key is not a key"
        )));
        assert!(about_the_key(&err("invalid_args", "public_key: bad")));
        assert!(!about_the_key(&err("invalid_args", "origin: bad")));
        assert!(!about_the_key(&err("invalid_origin", "public_key")));
        assert!(!about_the_key(&err("invalid_profile", "name")));
        assert!(!about_the_key(&Value::Null));
    }

    #[test]
    fn the_mint_request_is_the_signed_one_an_outside_backend_sends() {
        let key = KeyPair::generate();
        let i = integration(
            "q1",
            "http://localhost:7481",
            "http://localhost:7481",
            &grant_json(&key),
        )
        .unwrap();
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

    /// An outside product's backend signs for the instance's host and asks
    /// for a token for its own page origin: the two are kept apart.
    #[tokio::test]
    async fn the_mint_is_signed_for_the_instance_and_names_the_page_origin() {
        let key = KeyPair::generate();
        let i = integration(
            "q1",
            "https://clax.example:8443",
            "https://app.example.com",
            &grant_json(&key),
        )
        .unwrap();
        let req = mint_request(&i, 1_000, "u-42", "pk", None).unwrap();
        assert_eq!(req.headers()[header::HOST], "clax.example:8443");
        let parsed = sig::parse_header(req.headers()["clax-signature"].as_bytes()).unwrap();
        let body = axum::body::to_bytes(req.into_body(), 4096).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["origin"], "https://app.example.com", "{v}");
        assert_eq!(v["reporter_id"], "u-42", "{v}");
        let input = SignInput {
            method: "POST",
            path_and_query: "/api/queues/q1/reporter-tokens",
            host: "clax.example:8443",
            t: 1_000,
            nonce: &parsed.nonce,
            body_sha256: sha2::Sha256::digest(&body).into(),
            token_sha256: Some(clax_core::secret::sha256("cxq_t")),
            agent_headers_sha256: sig::no_agent_headers_sha256(),
        };
        assert!(sig::verify(&key.public(), &input, &parsed.sig));
    }
}
