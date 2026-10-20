//! Draft artifacts (spec 2026-10-12-flutter-app-round2-design §5.1), all
//! behind the token: `POST /api/drafts` makes one, `GET /api/drafts` lists
//! them, `DELETE /api/drafts/<id>` removes one for good. A draft is
//! published (`POST /api/artifacts/<id>/versions`, with no session),
//! rendered and read like any artifact, and hidden from every listing.

use super::artifacts::{JsonBytes, parse_body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_core::publish::{FileInput, PublishRequest, validate};
use clax_http::auth::RequireToken;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::drafts::DRAFT_MAX_AGE;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One file of a draft: its text, or a publish's file object.
#[derive(Deserialize)]
#[serde(untagged)]
enum DraftFile {
    Text(String),
    File(FileInput),
}

/// The body of `POST /api/drafts`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    /// The app the draft changes; absent or null for a new app.
    #[serde(default)]
    of: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    capabilities: Option<Value>,
    #[serde(default)]
    files: BTreeMap<String, DraftFile>,
    /// Whether the draft's fetches use `of`'s network grants (default
    /// true); false keeps it offline.
    #[serde(default)]
    inherit_net: Option<bool>,
}

/// `POST /api/drafts` `{of, title, capabilities, files, inherit_net?}`: a
/// draft of the app `of` (or of a new app) whose version 1 holds `files`
/// (each a string, or a publish's `{content, encoding, content_type}`),
/// with a copy of `of`'s documents. Its fetches use `of`'s network grants
/// unless `inherit_net` is false. 201 `{draft: {id, of, version, docs_copied,
/// docs_truncated}}`; 404 when `of` is missing, 400 `invalid_args` when it
/// is a draft, and a publish's validation errors.
pub async fn create(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    audit: AuditCtx,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (of, p, inherit_net) = parse_body(req, "the publish limit", |b: CreateBody| {
        let of = b.of.as_deref().map(parse_id).transpose()?;
        let inherit_net = b.inherit_net.unwrap_or(true);
        let files = b
            .files
            .into_iter()
            .map(|(k, f)| {
                let f = match f {
                    DraftFile::Text(content) => FileInput {
                        content,
                        encoding: Default::default(),
                        content_type: None,
                    },
                    DraftFile::File(f) => f,
                };
                (k, Some(f))
            })
            .collect();
        let p = validate(PublishRequest {
            title: b.title,
            capabilities: b.capabilities,
            files,
            ..PublishRequest::default()
        })?;
        Ok((of, p, inherit_net))
    })
    .await?;
    let d = s
        .store_call(move |st| st.create_draft_with(&audit, of.as_ref(), p, inherit_net))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"draft": {
            "id": d.artifact.id,
            "of": d.of,
            "version": d.version.n,
            "docs_copied": d.docs_copied,
            "docs_truncated": d.docs_truncated,
        }})),
    ))
}

/// `GET /api/drafts`: `{drafts: [{id, of, created_at}]}`, oldest first.
pub async fn list(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
) -> Result<Json<Value>, ApiError> {
    let drafts = s.store_call(|st| st.list_drafts()).await?;
    Ok(Json(json!({ "drafts": drafts })))
}

/// `DELETE /api/drafts/<id>`: removes the draft, its versions' files and
/// its documents for good. `{deleted: true}`; 404 when `id` is no draft.
pub async fn delete(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    audit: AuditCtx,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let cache = s.wrap_cache.clone();
    s.store_call(move |st| {
        if !st.delete_draft(&audit, &id)? {
            return Err(CoreError::NotFound);
        }
        cache.remove_artifact(id.as_str());
        Ok(())
    })
    .await?;
    Ok(Json(json!({"deleted": true})))
}

/// The body of `POST /api/drafts/<id>/renders`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderBody {
    digest: String,
    errors: Vec<Value>,
    failed_tests: u32,
}

/// `POST /api/drafts/<id>/renders` `{digest, errors, failed_tests}`:
/// records the app's render of the draft's files whose digest is `digest`
/// (`clax_core::digest::files_digest`: the files but `icon.svg`), clean
/// when `errors` is empty and `failed_tests` is 0; a later render of the
/// same digest replaces it. A job's publish step naming the draft is
/// refused 409 `not_rendered_clean` without a clean one (spec
/// 2026-10-19-in-process-core-design §8.2). `{render: {draft_id, digest,
/// clean}}`; 404 when `id` is no draft; 400 `invalid_args` for a digest
/// that is not `sha256:` and 64 lower-case hex digits.
pub async fn render(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let id = parse_id(&path(p)?)?;
    let (digest, clean) = parse_body(req, "the request body limit", |b: RenderBody| {
        let hex = b.digest.strip_prefix("sha256:").unwrap_or_default();
        if hex.len() != 64 || !hex.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(ApiError::bad_request(
                "invalid_args",
                "digest is sha256: and 64 lower-case hex digits",
            ));
        }
        Ok((b.digest, b.errors.is_empty() && b.failed_tests == 0))
    })
    .await?;
    let draft_id = id.as_str().to_string();
    let answer = json!({"render": {"draft_id": draft_id, "digest": digest, "clean": clean}});
    s.store_call(move |st| st.record_render(&id, &digest, clean))
        .await?;
    Ok(Json(answer))
}

/// Deletes the drafts older than [`DRAFT_MAX_AGE`] at `now`, and their
/// wrapped pages from `cache`; how many it deleted.
///
/// # Errors
/// The store's.
pub fn sweep(
    st: &Store,
    cache: &clax_http::wrap_cache::WrapCache,
    now: chrono::DateTime<chrono::Utc>,
) -> clax_core::Result<usize> {
    let gone = st.sweep_drafts(&AuditCtx::DAEMON, now, DRAFT_MAX_AGE)?;
    for id in &gone {
        cache.remove_artifact(id);
    }
    Ok(gone.len())
}

/// 403 `draft`: what a draft refuses (threads, sessions, watches).
pub(crate) fn refused(what: &str) -> ApiError {
    ApiError::forbidden("draft", format!("a draft takes no {what}"))
}
