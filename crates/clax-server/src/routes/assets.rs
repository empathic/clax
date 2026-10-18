//! Asset upload, listing, deletion, and the /_blob/<id> byte route.

use crate::routes::artifacts::path;
use axum::Json;
use axum::body::Body;
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::PathRejection;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_http::agent::{Agent, AssetsUpload, DeniedToGrants, TokenOrAgent};
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::live_access::SeesLive;
use clax_http::state::AppState;
use serde_json::{Value, json};

pub(crate) fn multipart_error(status: StatusCode, message: String) -> ApiError {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "request body exceeds the upload limit",
        )
    } else {
        ApiError::bad_request("invalid_multipart", message)
    }
}

/// Stores the multipart field `file` as an asset of the artifact. A live
/// page keeps no files besides its snapshots (spec 2026-10-05 §8.3): 400
/// `live_page`. A page's upload (`x-clax-via: page`, which the shell's
/// `assets` capability sends) also carries every author of the page (spec
/// 2026-10-08-multi-agent-multi-person §8.2); the uploader is a person's
/// agent's person, else the owner. A person's agent uploads from a
/// session and never as a page (spec §6.7); the answer is filtered for an
/// agent channel's reader.
pub async fn upload(
    State(s): State<AppState>,
    a: TokenOrAgent<AssetsUpload>,
    headers: axum::http::HeaderMap,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let aid = path(aid)?;
    let id = parse_id(&aid)?;
    if s.live_ids.contains(id.as_str()) {
        return Err(ApiError::bad_request(
            "live_page",
            "a live page keeps only its snapshots; it takes no assets",
        ));
    }
    clax_http::agent::person_agent_session(&a.0.caller)?;
    let via_page = clax_http::agent::page_marker(&a.0.caller, &headers)?;
    let vf = clax_http::views::thread::VisFor::of(&a.0.caller, &s);
    let mut mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let content_type = field
            .content_type()
            .map(|s| s.to_string())
            .or_else(|| field.file_name().map(clax_core::publish::content_type_for))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e.status(), e.body_text()))?;
        let person = match &a.0.caller {
            clax_http::auth::Caller::PersonAgent { person, .. } => person.clone(),
            _ => None,
        };
        let vf = vf.clone();
        let asset = s
            .store_call_async(move |st| async move {
                let st = &*st;
                let asset = st
                    .add_asset_async(
                        &audit,
                        &id,
                        &content_type,
                        &bytes,
                        person.as_deref(),
                        via_page,
                    )
                    .await?;
                let vis = vf.build(st)?;
                Ok(vis.filter_assets(std::slice::from_ref(&asset)).remove(0))
            })
            .await?;
        // A hosted daemon's keyed URL; a local daemon's names the artifact,
        // whose pages load only its own assets.
        let url = if s.hosted.is_some() {
            clax_http::hosted::asset_url(&s, asset["id"].as_str().unwrap_or_default())
        } else {
            format!(
                "/_blob/{}/{}",
                asset["artifact_id"].as_str().unwrap_or_default(),
                asset["id"].as_str().unwrap_or_default()
            )
        };
        return Ok((
            StatusCode::CREATED,
            Json(json!({"asset": asset, "url": url})),
        ));
    }
    Err(ApiError::bad_request(
        "missing_file",
        "multipart field 'file' is required",
    ))
}

/// The artifact's assets, each an agent channel's reader may not see with
/// its name and what its bytes decide withheld (spec §8.3).
pub async fn list(
    State(s): State<AppState>,
    a: Agent<clax_http::agent::AssetsList>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let aid = path(aid)?;
    let id = parse_id(&aid)?;
    let vf = clax_http::views::thread::VisFor::of(&a.caller, &s);
    let assets = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            Ok(vis.filter_assets(&st.list_assets(&id)?))
        })
        .await?;
    let mut assets = assets;
    clax_http::hosted::stamp_asset_urls(&s, &mut assets);
    Ok(Json(json!({"assets": assets})))
}

/// Deletes an asset. A person's agent deletes from a session (400
/// `no_session`), as it uploads.
pub async fn delete(
    State(s): State<AppState>,
    a: TokenOrAgent<clax_http::agent::AssetsDelete>,
    audit: AuditCtx,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    clax_http::agent::person_agent_session(&a.0.caller)?;
    let (aid, asset_id) = path(params)?;
    let id = parse_id(&aid)?;
    s.store_call_async(move |st| async move {
        match st.get_asset(&asset_id)? {
            Some(a) if a.artifact_id == id.as_str() => {
                st.delete_asset_async(&audit, &asset_id).await
            }
            _ => Err(CoreError::NotFound),
        }
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `/_blob/<asset ID>`: the asset's bytes, for the owner's surfaces and
/// links opened on their own. An app page's policy names only its own
/// artifact's prefix ([`scoped_blob`]), so a page cannot load this. An asset
/// of a live page answers 404 to a caller that may not see live pages
/// ([`SeesLive`]), as a missing one does, checked on the asset's row before
/// any of its bytes are read. Sent with `Referrer-Policy: no-referrer`: an
/// asset opened as a document names no asset ID to what it loads.
pub async fn blob(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    asset_id: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    // An asset's bytes serve browsers (spec §7.5): agents read through the routes the filter covers.
    d.refuse_agents()?;
    let asset_id = path(asset_id)?;
    let live_ids = s.live_ids.clone();
    let seen = move |aid: &str| sees.check(&live_ids, aid).is_ok();
    let (asset, bytes) = find_blob(&s, asset_id, None, seen).await?;
    Ok(send_blob(&asset, bytes))
}

/// `/_blob/<artifact ID>/<asset ID>`: an asset's bytes under the artifact
/// that owns it, the URL pages are given. Another artifact's asset answers
/// 404, as a missing one does, so the artifact's content policy, which
/// names only this prefix, keeps a page to its own assets. On an artifact's
/// origin `/_blob/<asset ID>` is this route for that artifact
/// (`host::rewrite_artifact_host`).
pub async fn scoped_blob(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Result<Response, ApiError> {
    // As `blob`: an asset's bytes serve browsers (spec §7.5).
    d.refuse_agents()?;
    let (aid, asset_id) = path(params)?;
    let id = parse_id(&aid)?;
    let live_ids = s.live_ids.clone();
    let seen = move |aid: &str| sees.check(&live_ids, aid).is_ok();
    let (asset, bytes) = find_blob(&s, asset_id, Some(id), seen).await?;
    Ok(send_blob(&asset, bytes))
}

/// The asset and its bytes; `NotFound` when it is missing, of an artifact
/// `seen` refuses (a live page the caller may not see), or, with `owner`,
/// another artifact's. The row is checked before any bytes are read.
async fn find_blob(
    s: &AppState,
    asset_id: String,
    owner: Option<clax_core::ArtifactId>,
    seen: impl Fn(&str) -> bool + Send + 'static,
) -> Result<(clax_core::model::Asset, Vec<u8>), ApiError> {
    s.store_call_async(move |st| async move {
        let asset = st.get_asset(&asset_id)?.ok_or(CoreError::NotFound)?;
        if owner.is_some_and(|o| o.as_str() != asset.artifact_id) || !seen(&asset.artifact_id) {
            return Err(CoreError::NotFound);
        }
        let bytes = st
            .asset_bytes_async(&asset)
            .await?
            .ok_or(CoreError::NotFound)?;
        Ok((asset, bytes))
    })
    .await
}

fn send_blob(asset: &clax_core::model::Asset, bytes: Vec<u8>) -> Response {
    let len = bytes.len().to_string();
    (
        [
            (header::CONTENT_TYPE, asset.content_type.as_str()),
            (header::CONTENT_LENGTH, len.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Body::from(bytes),
    )
        .into_response()
}
