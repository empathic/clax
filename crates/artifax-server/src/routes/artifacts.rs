//! REST routes for artifacts and versions.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use artifax_core::model::{Artifact, Session};
use artifax_core::publish::{PublishRequest, require_title, validate};
use artifax_core::{ArtifactId, CoreError, Event, MetaPatch, Store};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

/// Request body cap for the publish routes. A fully base64-encoded 64 MiB
/// payload is about 85 MiB on the wire, so the cap sits above that;
/// `validate` still enforces the 64 MiB decoded limit.
pub const PUBLISH_BODY_LIMIT: usize = 96 * 1024 * 1024;

pub fn parse_id(raw: &str) -> Result<ArtifactId, ApiError> {
    ArtifactId::parse(raw).map_err(ApiError::from)
}

pub(crate) fn body<T>(r: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    r.map(|Json(v)| v).map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "request body exceeds the publish limit",
            )
        } else {
            ApiError::bad_request("invalid_json", e.body_text())
        }
    })
}

pub(crate) fn path<T>(r: Result<Path<T>, PathRejection>) -> Result<T, ApiError> {
    r.map(|Path(v)| v)
        .map_err(|e| ApiError::bad_request("invalid_path_param", e.body_text()))
}

/// Header naming the session a publish is attributed to.
pub const SESSION_HEADER: &str = "x-artifax-session";

/// The session named by `X-Artifax-Session`, checked to exist and be live.
///
/// # Errors
/// `unknown_session` when the header is not valid text, or names a session that
/// does not exist or has ended.
fn publishing_session(st: &Store, header: &Option<String>) -> Result<Option<String>, CoreError> {
    let Some(id) = header else {
        return Ok(None);
    };
    match st.get_session(id)? {
        Some(sess) if sess.ended_at.is_none() => Ok(Some(sess.id)),
        _ => Err(CoreError::invalid(
            "unknown_session",
            "X-Artifax-Session names no live session",
        )),
    }
}

/// The raw `X-Artifax-Session` value; a value that is not UTF-8 is
/// `unknown_session`.
fn session_header(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    headers
        .get(SESSION_HEADER)
        .map(|v| {
            v.to_str().map(str::to_string).map_err(|_| {
                ApiError::bad_request("unknown_session", "X-Artifax-Session is not valid text")
            })
        })
        .transpose()
}

/// `a` as JSON with `owner_live` (its owner session exists and has not ended)
/// and `owner_harness` (the owner's harness, when it exists). The owner
/// session itself is not exposed: these routes need no token.
fn with_owner(a: &Artifact, owner: Option<&Session>) -> Value {
    let mut v = serde_json::to_value(a).expect("serialisable artifact");
    v["owner_live"] = json!(owner.is_some_and(|o| o.ended_at.is_none()));
    v["owner_harness"] = json!(owner.map(|o| &o.harness));
    v
}

/// Each live artifact, with the owner fields of [`with_owner`].
pub async fn list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let artifacts = s
        .store_call(|st| {
            let mut owners = std::collections::HashMap::new();
            let mut out = Vec::new();
            for a in st.list_artifacts()? {
                let owner = match &a.owner_session_id {
                    Some(sid) => {
                        if !owners.contains_key(sid) {
                            owners.insert(sid.clone(), st.get_session(sid)?);
                        }
                        owners[sid].clone()
                    }
                    None => None,
                };
                out.push(with_owner(&a, owner.as_ref()));
            }
            Ok(out)
        })
        .await?;
    Ok(Json(json!({"artifacts": artifacts})))
}

/// Creates an artifact. The body must carry a non-blank `title`.
pub async fn create(
    State(s): State<AppState>,
    _t: RequireToken,
    headers: HeaderMap,
    req: Result<Json<PublishRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let p = validate(body(req)?)?;
    require_title(p.title.as_deref())?;
    let session = session_header(&headers)?;
    let events = s.events.clone();
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session)?;
            let (artifact, version) = st.create_artifact(p, session.as_deref())?;
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
            });
            Ok((artifact, version))
        })
        .await?;
    let url = format!("/a/{}", artifact.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"artifact": artifact, "version": version, "url": url})),
    ))
}

/// The artifact (with the owner fields of [`with_owner`]) and its versions.
pub async fn get(
    State(s): State<AppState>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let (artifact, versions, owner) = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = st.list_versions(&id)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            Ok((a, v, owner))
        })
        .await?;
    Ok(Json(
        json!({"artifact": with_owner(&artifact, owner.as_ref()), "versions": versions}),
    ))
}

/// Capabilities are set through publish until the runtime bridge honours them.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    title: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    pinned: Option<bool>,
}

pub async fn patch(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let artifact = s
        .store_call(move |st| {
            st.update_meta(
                &id,
                MetaPatch {
                    title: b.title,
                    description: b.description,
                    icon: b.icon,
                    pinned: b.pinned,
                },
            )
        })
        .await?;
    Ok(Json(json!({"artifact": artifact})))
}

pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let events = s.events.clone();
    let cache = s.wrap_cache.clone();
    s.store_call(move |st| {
        st.delete_artifact(&id)?;
        cache.remove_artifact(id.as_str());
        events.publish(Event::ArtifactDeleted {
            artifact_id: id.as_str().to_string(),
        });
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_versions(
    State(s): State<AppState>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let versions = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.list_versions(&id)
        })
        .await?;
    Ok(Json(json!({"versions": versions})))
}

pub async fn publish(
    State(s): State<AppState>,
    _t: RequireToken,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<PublishRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = parse_id(&path(aid)?)?;
    let p = validate(body(req)?)?;
    let session = session_header(&headers)?;
    let events = s.events.clone();
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session)?;
            let (artifact, version) = st.publish_version(&id, p, session.as_deref())?;
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
            });
            Ok((artifact, version))
        })
        .await?;
    let url = format!("/a/{}", artifact.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"artifact": artifact, "version": version, "url": url})),
    ))
}

pub async fn get_version(
    State(s): State<AppState>,
    params: Result<Path<(String, u32)>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, n) = path(params)?;
    let id = parse_id(&aid)?;
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.get_version(&id, n)?.ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(Json(json!({"version": v})))
}

pub async fn files(
    State(s): State<AppState>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let v = s
        .store_call(move |st| {
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.get_version(&id, a.current_version)?
                .ok_or(CoreError::NotFound)
        })
        .await?;
    Ok(Json(json!({"files": v.files, "version": v.n})))
}
