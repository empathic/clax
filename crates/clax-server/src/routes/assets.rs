//! Asset upload, listing, deletion, and the asset byte routes: `/_blob/<id>`
//! on the public host, and on a site host (`super::site`) its own artifact's.

use crate::routes::artifacts::path;
use axum::Json;
use axum::body::Body;
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::PathRejection;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use clax_core::audit::AuditCtx;
use clax_core::{ArtifactId, CoreError};
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
                let legacy = st.get_artifact(&id)?.is_some_and(|a| a.legacy_paths);
                let vis = vf.build(st)?;
                Ok((
                    vis.filter_assets(std::slice::from_ref(&asset)).remove(0),
                    legacy,
                ))
            })
            .await?;
        let (asset, legacy) = asset;
        let url = clax_http::hosted::asset_url(asset["id"].as_str().unwrap_or_default(), legacy);
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
    let (mut assets, legacy) = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            Ok((vis.filter_assets(&st.list_assets(&id)?), a.legacy_paths))
        })
        .await?;
    clax_http::hosted::stamp_asset_urls(&mut assets, legacy);
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

/// `/_blob/<asset ID>` and `/_clax/blob/<asset ID>` on the public host
/// (and a hosted daemon's retired content origin), an asset's addresses of
/// before each artifact had a site: 301 to the asset's address on its
/// artifact's site (`/_blob/<id>` for a legacy artifact, else
/// `/_clax/blob/<id>`), whose access check decides; no bytes are served
/// here. An unknown asset, or a live page's to a caller that may not see
/// live pages, is 404.
pub async fn blob(
    State(s): State<AppState>,
    d: DeniedToGrants,
    sees: SeesLive,
    headers: axum::http::HeaderMap,
    asset_id: Result<Path<String>, PathRejection>,
) -> Result<Response, ApiError> {
    // An asset's bytes serve browsers (spec §7.5): agents read through the routes the filter covers.
    d.refuse_agents()?;
    let asset_id = path(asset_id)?;
    let live_ids = s.live_ids.clone();
    let (aid, asset_id, legacy) = s
        .store_call(move |st| {
            let asset = st.get_asset(&asset_id)?.ok_or(CoreError::NotFound)?;
            if sees.check(&live_ids, &asset.artifact_id).is_err() {
                return Err(CoreError::NotFound);
            }
            let aid = ArtifactId::parse(&asset.artifact_id)?;
            let a = st.get_artifact(&aid)?.ok_or(CoreError::NotFound)?;
            Ok((aid, asset.id, a.legacy_paths))
        })
        .await?;
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let to = format!(
        "{}{}",
        clax_http::site_access::site_origin(&s, &aid, host),
        clax_http::hosted::asset_url(&asset_id, legacy)
    );
    Ok((
        StatusCode::MOVED_PERMANENTLY,
        [
            (header::LOCATION, to.as_str()),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
    )
        .into_response())
}

/// The asset's bytes; with `owner` (a site host's artifact), only when the
/// asset is that artifact's. An asset of a live page answers 404 to a
/// caller that may not see live pages ([`SeesLive`]), as a missing one,
/// or another artifact's, does, checked on the asset's row before any of
/// its bytes are read.
pub(crate) async fn serve_blob(
    s: &AppState,
    sees: SeesLive,
    asset_id: String,
    owner: Option<ArtifactId>,
) -> Result<Response, ApiError> {
    let live_ids = s.live_ids.clone();
    let (asset, bytes) = s
        .store_call_async(move |st| async move {
            let asset = st.get_asset(&asset_id)?.ok_or(CoreError::NotFound)?;
            if owner.is_some_and(|o| o.as_str() != asset.artifact_id) {
                return Err(CoreError::NotFound);
            }
            if sees.check(&live_ids, &asset.artifact_id).is_err() {
                return Err(CoreError::NotFound);
            }
            let bytes = st
                .asset_bytes_async(&asset)
                .await?
                .ok_or(CoreError::NotFound)?;
            Ok((asset, bytes))
        })
        .await?;
    let len = bytes.len().to_string();
    (
        [
            (header::CONTENT_TYPE, asset.content_type.as_str()),
            (header::CONTENT_LENGTH, len.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Body::from(bytes),
    )
        .into_response()
}
