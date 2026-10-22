//! An artifact's site: what its site host (`<id>.<site domain>`,
//! `<id>.localhost:<port>` on a local daemon, [`clax_http::host`]) serves.
//! The host alone names the artifact ([`SiteRequest`]); no path does.
//!
//! - `/…`: the current version's files (`/` is its `index.html`),
//!   revalidated on every load, with an `ETag`.
//! - `/_clax/v/<n>/…`: version `n`'s files, for review of older versions.
//! - `/_clax/draft/<agent>/<token>/…`: a draft revision
//!   ([`super::drafts`]), its token its capability.
//! - `/_clax/blob/<asset ID>`: an asset of this artifact, and no other's.
//! - `/_clax/healthz`: health, on any well-formed site host, whether or not
//!   its artifact exists: it says nothing of the artifact.
//! - `/_clax/session?code=&next=`: redeems a site code for a site session
//!   ([`clax_http::site_access`]).
//! - `/_clax/api/…`: the page runtime's backend, on this artifact alone
//!   ([`super::site_api`]).
//! - `/_clax/…`: the UI's files and the bridge (`crate::shell::static_file`);
//!   any other `/_clax/` path is 404.
//! - On a legacy artifact (`Artifact::legacy_paths`) `/_blob/<asset ID>` is
//!   its assets' too, as its pages name them; on any other, `_blob/` is the
//!   artifact's own path.
//!
//! Every route but health, the session's and the UI's files is decided by the
//! artifact's access (`clax_http::site_access::site_access_layer`); before
//! that, a live page's site answers 404 to a caller that may not see live
//! pages (`clax_http::live_access::sees_live_pages`), as a missing one does.

use super::{assets, content, drafts, health};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use clax_core::publish::INDEX;
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::host::SiteRequest;
use clax_http::live_access::{SeesLive, sees_live_pages};
use clax_http::state::AppState;

/// The site routes, mounted for a request that carries [`SiteRequest`]
/// (`crate::router`).
pub fn site_routes(state: &AppState) -> Router<AppState> {
    // What the artifact's access decides (`site_access_layer`), after a
    // live page is hidden from who may not see it (the later route layer
    // is outermost).
    let site = Router::new()
        .route("/", get(current_index))
        .route("/_clax/v/{n}", get(version_slash))
        .route("/_clax/v/{n}/", get(version_index))
        .route("/_clax/v/{n}/{*path}", get(version_file))
        .route("/_clax/draft/{agent}/{token}", get(draft_slash))
        .route("/_clax/draft/{agent}/{token}/", get(draft_index))
        .route("/_clax/draft/{agent}/{token}/{*path}", get(draft_file))
        .route("/_clax/blob/{asset_id}", get(blob))
        .route("/_blob/{asset_id}", get(legacy_blob))
        .route("/{*path}", get(current_file))
        .merge(super::site_api::routes(state))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            clax_http::site_access::site_access_layer,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            hide_live_site,
        ));
    Router::new()
        // A deploy probes it from anywhere, and the shell from its own
        // origin.
        .route(
            "/_clax/healthz",
            get(health::healthz).layer(
                tower_http::cors::CorsLayer::new()
                    .allow_origin(tower_http::cors::Any)
                    .allow_methods(tower_http::cors::Any),
            ),
        )
        .route("/_clax/session", get(session))
        .route("/_clax/session/start", get(session_start))
        .route("/_clax/{*path}", get(crate::shell::static_file))
        .merge(site)
        .fallback(|| async { ApiError::not_found() })
        .layer(clax_http::http_cache::compression())
}

/// `/_clax/session?code=<code>&next=<path>`: redeems a site code made for
/// this artifact (`POST /api/artifacts/<id>/site-code`, or the public
/// host's `/site-signin`) beside the state cookie it was minted for, sets
/// the site cookie (partitioned as the code says) and redirects to `next`,
/// a path on this host ([`clax_http::site_access::safe_next`]). A top-level
/// redemption clears the state; a frame's keeps it, as another tab's frame
/// may be signing in with it ([`clax_http::site_access::state_for`]). A
/// code that does not redeem sets no session and sends the browser to
/// `/site-signin` for `next` ([`clax_http::site_access::signin`]: a frame
/// that sent none of the site's cookies gets the no-cookie page instead).
async fn session(
    State(s): State<AppState>,
    site: Extension<SiteRequest>,
    headers: HeaderMap,
    q: Query<SessionQuery>,
) -> Result<Response, ApiError> {
    use axum::http::{HeaderValue, StatusCode, header};
    use clax_http::site_access as sa;
    let next = sa::safe_next(q.next.as_deref().unwrap_or("/"));
    let code = q.code.clone().unwrap_or_default();
    let secure = sa::secure(&s);
    let state = sa::state_in(&headers, secure);
    let aid = site.artifact.clone();
    let redeemed = s
        .store_call(move |st| st.site_code_redeem(&code, &aid, state.as_deref()))
        .await?;
    let Some((id, _, partitioned)) = redeemed else {
        return Ok(sa::signin(&s, &headers, &next));
    };
    let mut res = (StatusCode::FOUND, [(header::LOCATION, next)]).into_response();
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::SET_COOKIE,
        sa::site_cookie(secure, partitioned, &id),
    );
    if !partitioned {
        h.append(header::SET_COOKIE, sa::clear_state_cookie(secure, false));
    }
    Ok(res)
}

/// `/_clax/session`'s query.
#[derive(serde::Deserialize)]
struct SessionQuery {
    code: Option<String>,
    next: Option<String>,
}

/// `/_clax/session/start?next=<path>`: the framed handshake's first step.
/// Sets the browser's state ([`clax_http::site_access::state_for`]) in the
/// partitioned state cookie and answers a page that
/// hands the state to the shell framing it (`postMessage` of `{type:
/// "clax:site-state", state, next}` to the shell's origins alone,
/// [`clax_http::site_access::shell_origins`]: the public origin, or a local
/// daemon's loopback names), which asks `POST
/// /api/artifacts/<id>/site-code` for a code bound to it. The page runs
/// only its own script (a nonce) and only the shell may frame it.
async fn session_start(
    State(s): State<AppState>,
    headers: HeaderMap,
    q: Query<StartQuery>,
) -> Response {
    use axum::http::{HeaderValue, header};
    use clax_http::site_access as sa;
    let next = sa::safe_next(q.next.as_deref().unwrap_or("/"));
    let state = sa::state_for(&headers, sa::secure(&s));
    let nonce = sa::new_state();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let shells = sa::shell_origins(&s, host);
    let message = serde_json::json!({"type": "clax:site-state", "state": state, "next": next});
    // Inside a script element: no `<` may close it. A post to an origin
    // that is not the parent's is dropped.
    let script = |v: &serde_json::Value| v.to_string().replace('<', "\\u003c");
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><script nonce=\"{nonce}\">for(const o of {})parent.postMessage({},o);</script>",
        script(&serde_json::json!(shells)),
        script(&message),
    );
    let mut res = axum::response::Html(html).into_response();
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; frame-ancestors {}",
        shells.join(" ")
    )) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(
        header::SET_COOKIE,
        sa::state_cookie(sa::secure(&s), true, &state),
    );
    res
}

/// `/_clax/session/start`'s query.
#[derive(serde::Deserialize)]
struct StartQuery {
    next: Option<String>,
}

/// A live page's site is 404 to a caller that may not see live pages.
async fn hide_live_site(
    State(s): State<AppState>,
    site: Extension<SiteRequest>,
    req: Request,
    next: Next,
) -> Response {
    if s.live_ids.contains(site.artifact.as_str())
        && !sees_live_pages(req.headers(), req.extensions(), &s.token)
    {
        return ApiError::from(CoreError::NotFound).into_response();
    }
    next.run(req).await
}

/// A path parameter that does not parse names nothing on a site: 404.
fn param<T>(p: Result<Path<T>, PathRejection>) -> Result<T, ApiError> {
    p.map(|Path(v)| v).map_err(|_| ApiError::not_found())
}

/// A version segment in its one form, `n.to_string()` (no sign, no leading
/// zero); any other names nothing: 404.
fn version(seg: &str) -> Result<u32, ApiError> {
    seg.parse::<u32>()
        .ok()
        .filter(|n| n.to_string() == seg)
        .ok_or_else(ApiError::not_found)
}

/// The artifact's current version: 404 when it does not exist (or has none).
async fn current_of(s: &AppState, id: &ArtifactId) -> Result<u32, ApiError> {
    let id = id.clone();
    let n = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            Ok(a.current_version)
        })
        .await?;
    if n == 0 {
        return Err(ApiError::not_found());
    }
    Ok(n)
}

/// `/`: the current version's `index.html`.
async fn current_index(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let n = current_of(&s, &site.artifact).await?;
    content::serve_version(&s, site.artifact.clone(), n, INDEX.into(), true, &req).await
}

/// `/<path>`: a file of the current version; `/index.html` is `/`.
async fn current_file(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let rel = param(p)?;
    serve_current(&s, &site, rel, &req).await
}

async fn serve_current(
    s: &AppState,
    site: &Extension<SiteRequest>,
    rel: String,
    req: &HeaderMap,
) -> Result<Response, ApiError> {
    if rel == INDEX {
        return Ok(Redirect::permanent("./").into_response());
    }
    let n = current_of(s, &site.artifact).await?;
    content::serve_version(s, site.artifact.clone(), n, rel, true, req).await
}

/// `/_clax/v/<n>`: the version's root, with its slash.
async fn version_slash(
    d: DeniedToGrants,
    p: Result<Path<String>, PathRejection>,
) -> Result<Redirect, ApiError> {
    d.refuse_agents()?;
    let n = version(&param(p)?)?;
    Ok(Redirect::permanent(&format!("{n}/")))
}

/// `/_clax/v/<n>/`: version `n`'s `index.html`.
async fn version_index(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let n = version(&param(p)?)?;
    content::serve_version(&s, site.artifact.clone(), n, INDEX.into(), false, &req).await
}

/// `/_clax/v/<n>/<path>`: a file of version `n`; `index.html` redirects to
/// the version's root.
async fn version_file(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let (n, rel) = param(p)?;
    let n = version(&n)?;
    if rel == INDEX {
        return Ok(Redirect::permanent("./").into_response());
    }
    content::serve_version(&s, site.artifact.clone(), n, rel, false, &req).await
}

/// `/_clax/draft/<agent>/<token>`: the draft's root, with its slash.
async fn draft_slash(
    d: DeniedToGrants,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Redirect, ApiError> {
    d.refuse_agents()?;
    let (_, token) = param(p)?;
    if !clax_core::drafts::is_token(&token) {
        return Err(ApiError::not_found());
    }
    Ok(Redirect::permanent(&format!("{token}/")))
}

/// `/_clax/draft/<agent>/<token>/`: the draft's `index.html`.
async fn draft_index(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let (agent, token) = param(p)?;
    drafts::serve(
        &s,
        site.artifact.as_str(),
        &agent,
        &token,
        INDEX.into(),
        &req,
    )
    .await
}

/// `/_clax/draft/<agent>/<token>/<path>`: a file of the draft revision.
async fn draft_file(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<(String, String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let (agent, token, rel) = param(p)?;
    drafts::serve(&s, site.artifact.as_str(), &agent, &token, rel, &req).await
}

/// `/_clax/blob/<asset ID>`: the asset's bytes, when it is this artifact's.
async fn blob(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    site: Extension<SiteRequest>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let asset_id = param(p)?;
    assets::serve_blob(&s, sees, asset_id, Some(site.artifact.clone())).await
}

/// `/_blob/<asset ID>`: on a legacy artifact's site, its asset's bytes, as
/// [`blob`]; on any other, the current version's file `_blob/<asset ID>`.
async fn legacy_blob(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let asset_id = param(p)?;
    let id = site.artifact.clone();
    let legacy = s
        .store_call(move |st| Ok(st.get_artifact(&id)?.is_some_and(|a| a.legacy_paths)))
        .await?;
    if legacy {
        return assets::serve_blob(&s, sees, asset_id, Some(site.artifact.clone())).await;
    }
    serve_current(&s, &site, format!("_blob/{asset_id}"), &req).await
}
