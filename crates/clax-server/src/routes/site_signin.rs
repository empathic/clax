//! The public host's half of the site handshake
//! ([`clax_http::site_access`]): it knows the browser, so it mints the
//! one-use code an artifact's site host redeems for a site session
//! (`crate::routes::site`, `/_clax/session`).
//!
//! - `POST /api/artifacts/<id>/site-code` (`{"next": "<path>",
//!   "partitioned": true, "state": "<state>"}`): the shell's frame, with
//!   the state its frame's `/_clax/session/start` handed it. Answers
//!   `{"src": "<site origin>/_clax/session?code=<code>&next=<path>"}`, 400
//!   without a state, 404 when the caller may not read the artifact.
//! - `GET /site-signin?host=<site host>&next=<path>&state=<state>`: a visit
//!   the site sent here with a state (400 without one). An unknown host is
//!   404; with no session and no link to the artifact, the sign-in page,
//!   which returns here; without the right, the artifact's page, which
//!   tells them; else a code and on to the site's `/_clax/session`. A frame
//!   is sent here too (`Sec-Fetch-Dest: iframe`: a partitioned code), a
//!   hosted Clax's fallback for a frame whose shell asked no code: its
//!   session cookie is `SameSite=Lax`, so a frame of the public origin's
//!   shell carries it; a local daemon's owner cookie is scoped to `/api`
//!   and `/a`, so a local frame needs the shell's code.
//!
//! Every code is bound to its state: `/_clax/session` redeems it only
//! beside the state cookie that names it.
//!
//! Both answer `no-store` and `Referrer-Policy: no-referrer`: the code is
//! in the address they hand out.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::live_access::SeesLive;
use clax_http::site_access::{holder_reads, request_holder, safe_next, site_origin};
use clax_http::state::AppState;
use clax_store::site_sessions::Holder;
use serde::Deserialize;
use serde_json::json;

/// `POST /api/artifacts/<id>/site-code`'s body.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CodeBody {
    /// Where on the site the session starts (`/` without it).
    #[serde(default)]
    next: Option<String>,
    /// Whether the session's cookie is partitioned (the shell's frame).
    #[serde(default)]
    partitioned: bool,
    /// The handshake's state, from the frame's `/_clax/session/start`.
    #[serde(default)]
    state: Option<String>,
}

/// What a code is minted for: where it leads, whether its session is
/// partitioned, and the state it is bound to.
struct Ask<'a> {
    next: &'a str,
    partitioned: bool,
    state: &'a str,
}

/// The state a request names: 400 `invalid_args` without one.
fn state_of(state: Option<&str>) -> Result<&str, ApiError> {
    state
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_args",
                "a site code is bound to the browser's state: send the state its site handed out",
            )
        })
}

/// `no-store` and no referrer: the answer carries a code.
fn private(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    res
}

fn host_of(headers: &HeaderMap) -> &str {
    headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
}

/// A code for `holder` (bound to `browser`) on `aid`, when they may read
/// it, and the address that redeems it for `next`; `None` when they may not
/// (or the artifact is a live page they may not see).
async fn mint(
    s: &AppState,
    headers: &HeaderMap,
    sees: &SeesLive,
    aid: ArtifactId,
    (holder, browser): (Holder, Option<String>),
    ask: Ask<'_>,
) -> Result<Option<String>, ApiError> {
    let (next, partitioned, state) = (ask.next, ask.partitioned, ask.state.to_string());
    if sees.check(&s.live_ids, aid.as_str()).is_err() {
        return Ok(None);
    }
    let (lookup, hosted) = (aid.clone(), s.hosted.is_some());
    let code = s
        .store_call(move |st| {
            st.get_artifact(&lookup)?.ok_or(CoreError::NotFound)?;
            if !holder_reads(st, hosted, &holder, &lookup)? {
                return Ok(None);
            }
            st.site_code_mint(&lookup, &holder, browser.as_deref(), partitioned, &state)
                .map(Some)
        })
        .await;
    let code = match code {
        Ok(c) => c,
        Err(e) if e.status == StatusCode::NOT_FOUND => None,
        Err(e) => return Err(e),
    };
    Ok(code.map(|c| {
        let q = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("code", &c)
            .append_pair("next", next)
            .finish();
        format!(
            "{}/_clax/session?{q}",
            site_origin(s, &aid, host_of(headers))
        )
    }))
}

/// `POST /api/artifacts/<id>/site-code` (see the module docs).
pub async fn site_code(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    p: Result<Path<String>, PathRejection>,
    body: Result<Json<CodeBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let aid = clax_http::headers::parse_id(&super::artifacts::path(p)?)?;
    let body = match body {
        Ok(Json(b)) => b,
        Err(e) => return Err(ApiError::bad_request("invalid_json", e.body_text())),
    };
    let next = safe_next(body.next.as_deref().unwrap_or("/"));
    let Some(holder) = request_holder(&s, &headers, &extensions) else {
        return Err(ApiError::not_found());
    };
    let state = state_of(body.state.as_deref())?;
    let ask = Ask {
        next: &next,
        partitioned: body.partitioned,
        state,
    };
    match mint(&s, &headers, &sees, aid, holder, ask).await? {
        Some(src) => Ok(private(Json(json!({"src": src})).into_response())),
        None => Err(ApiError::not_found()),
    }
}

/// `GET /site-signin`'s query.
#[derive(Deserialize)]
pub struct SigninQuery {
    host: Option<String>,
    next: Option<String>,
    state: Option<String>,
}

fn found(to: &str) -> Response {
    private((StatusCode::FOUND, [(header::LOCATION, to.to_string())]).into_response())
}

/// `GET /site-signin?host=<site host>&next=<path>&state=<state>` (see the
/// module docs).
pub async fn page(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    uri: axum::http::Uri,
    q: Result<Query<SigninQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let Ok(Query(q)) = q else {
        return Err(ApiError::not_found());
    };
    let Some(target) = q.host.as_deref().and_then(|h| s.sites().target(h)) else {
        return Err(ApiError::not_found());
    };
    let aid = target.artifact;
    sees.check(&s.live_ids, aid.as_str())?;
    let next = safe_next(q.next.as_deref().unwrap_or("/"));
    state_of(q.state.as_deref())?;
    let holder = request_holder(&s, &headers, &extensions);
    let signed_in = !matches!(holder, None | Some((Holder::Link, _)));
    if !signed_in && s.hosted.is_some() {
        let lookup = aid.as_str().to_string();
        let shared = s.store_call(move |st| st.link_access(&lookup)).await?;
        if shared.is_none() {
            let here = uri
                .path_and_query()
                .map_or("/site-signin", |p| p.as_str())
                .to_string();
            let q = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("next", &here)
                .finish();
            return Ok(found(&format!("/signin?{q}")));
        }
    }
    let src = match holder {
        Some(h) => {
            let ask = Ask {
                next: &next,
                partitioned: clax_http::site_access::framed(&headers),
                state: state_of(q.state.as_deref())?,
            };
            mint(&s, &headers, &sees, aid.clone(), h, ask).await?
        }
        None => None,
    };
    match src {
        Some(src) => Ok(found(&src)),
        // The artifact's own page says it is not theirs to read.
        None => Ok(found(&format!("/a/{aid}"))),
    }
}
