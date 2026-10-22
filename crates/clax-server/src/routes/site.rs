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
//! - `/_clax/…`: the UI's files and the bridge (`crate::shell::static_file`);
//!   any other `/_clax/` path is 404.
//! - On a legacy artifact (`Artifact::legacy_paths`) `/_blob/<asset ID>` is
//!   its assets' too, as its pages name them; on any other, `_blob/` is the
//!   artifact's own path.
//!
//! A live page's site answers 404 to a caller that may not see live pages
//! (`clax_http::live_access::sees_live_pages`), as a missing one does.

use super::{assets, content, drafts, health};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Extension, Path, Request, State};
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
    let site = Router::new()
        .route("/", get(current_index))
        .route("/_clax/v/{n}", get(version_slash))
        .route("/_clax/v/{n}/", get(version_index))
        .route("/_clax/v/{n}/{*path}", get(version_file))
        .route("/_clax/draft/{agent}/{token}", get(draft_slash))
        .route("/_clax/draft/{agent}/{token}/", get(draft_index))
        .route("/_clax/draft/{agent}/{token}/{*path}", get(draft_file))
        .route("/_clax/blob/{asset_id}", get(blob))
        .route("/_clax/{*path}", get(crate::shell::static_file))
        .route("/_blob/{asset_id}", get(legacy_blob))
        .route("/{*path}", get(current_file))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            hide_live_site,
        ));
    Router::new()
        // The shell probes it from its own origin (`probeOrigin` in
        // web/shell/src/origin.ts), and a deploy from anywhere.
        .route(
            "/_clax/healthz",
            get(health::healthz).layer(
                tower_http::cors::CorsLayer::new()
                    .allow_origin(tower_http::cors::Any)
                    .allow_methods(tower_http::cors::Any),
            ),
        )
        .merge(site)
        .fallback(|| async { ApiError::not_found() })
        .layer(clax_http::http_cache::compression())
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
    let origin = Some(site.clone());
    content::serve_version(
        &s,
        site.artifact.clone(),
        n,
        INDEX.into(),
        true,
        &origin,
        &req,
    )
    .await
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
    let origin = Some(site.clone());
    content::serve_version(s, site.artifact.clone(), n, rel, true, &origin, req).await
}

/// `/_clax/v/<n>`: the version's root, with its slash.
async fn version_slash(
    d: DeniedToGrants,
    p: Result<Path<u32>, PathRejection>,
) -> Result<Redirect, ApiError> {
    d.refuse_agents()?;
    let n = param(p)?;
    Ok(Redirect::permanent(&format!("{n}/")))
}

/// `/_clax/v/<n>/`: version `n`'s `index.html`.
async fn version_index(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<u32>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let n = param(p)?;
    let origin = Some(site.clone());
    content::serve_version(
        &s,
        site.artifact.clone(),
        n,
        INDEX.into(),
        false,
        &origin,
        &req,
    )
    .await
}

/// `/_clax/v/<n>/<path>`: a file of version `n`; `index.html` redirects to
/// the version's root.
async fn version_file(
    State(s): State<AppState>,
    d: DeniedToGrants,
    site: Extension<SiteRequest>,
    req: HeaderMap,
    p: Result<Path<(u32, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let (n, rel) = param(p)?;
    if rel == INDEX {
        return Ok(Redirect::permanent("./").into_response());
    }
    let origin = Some(site.clone());
    content::serve_version(&s, site.artifact.clone(), n, rel, false, &origin, &req).await
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
    let origin = Some(site.clone());
    drafts::serve(
        &s,
        site.artifact.as_str(),
        &agent,
        &token,
        INDEX.into(),
        &origin,
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
    let origin = Some(site.clone());
    drafts::serve(
        &s,
        site.artifact.as_str(),
        &agent,
        &token,
        rel,
        &origin,
        &req,
    )
    .await
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
