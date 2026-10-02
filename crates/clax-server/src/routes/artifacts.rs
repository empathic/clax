//! REST routes for artifacts and versions.

use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use clax_core::model::{Artifact, Session};
use clax_core::publish::{PublishRequest, require_title, validate};
use clax_core::{ArtifactId, CoreError, Event, MetaPatch, Store};
use serde::Deserialize;
use serde_json::{Value, json};

/// Request body cap for the publish routes. A fully base64-encoded 64 MiB
/// payload is about 85 MiB on the wire, so the cap sits above that;
/// `validate` still enforces the 64 MiB decoded limit.
pub const PUBLISH_BODY_LIMIT: usize = 96 * 1024 * 1024;

pub fn parse_id(raw: &str) -> Result<ArtifactId, ApiError> {
    ArtifactId::parse(raw).map_err(ApiError::from)
}

/// The JSON body of a route with axum's default body limit; see [`body_within`].
pub(crate) fn body<T>(r: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body_within(r, "the request body limit")
}

/// The JSON body, or 413 `body_too_large` naming `limit` (the route's own
/// cap, such as "the publish limit"), or 400 `invalid_json`.
pub(crate) fn body_within<T>(
    r: Result<Json<T>, JsonRejection>,
    limit: &str,
) -> Result<T, ApiError> {
    r.map(|Json(v)| v).map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                format!("request body exceeds {limit}"),
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

/// Header a shell sets when the page itself publishes (`artifact.publish`):
/// the version event then carries `by_page`.
pub const VIA_HEADER: &str = "x-clax-via";

/// Header naming the session a publish is attributed to.
pub const SESSION_HEADER: &str = "x-clax-session";

/// The session named by `X-Clax-Session`, checked to exist and be live.
///
/// # Errors
/// `unknown_session` when the header is not valid text, or names a session that
/// does not exist or has ended.
pub(crate) fn publishing_session(
    st: &Store,
    header: &Option<String>,
) -> Result<Option<String>, CoreError> {
    let Some(id) = header else {
        return Ok(None);
    };
    match st.get_session(id)? {
        Some(sess) if sess.ended_at.is_none() => Ok(Some(sess.id)),
        _ => Err(CoreError::invalid(
            "unknown_session",
            "X-Clax-Session names no live session",
        )),
    }
}

/// The raw `X-Clax-Session` value; a value that is not UTF-8 is
/// `unknown_session`.
pub(crate) fn session_header(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    headers
        .get(SESSION_HEADER)
        .map(|v| {
            v.to_str().map(str::to_string).map_err(|_| {
                ApiError::bad_request("unknown_session", "X-Clax-Session is not valid text")
            })
        })
        .transpose()
}

/// `a` as JSON with `owner_live` (its owner session exists and has not ended)
/// and `owner_harness` (the owner's harness, when it exists). The owner
/// session itself is not exposed: these routes need no token. `working` is
/// the artifact's working list (spec §10 "Working"), which never names a session.
pub(crate) fn with_owner(
    a: &Artifact,
    owner: Option<&Session>,
    working: &[clax_core::working::WorkingView],
) -> Value {
    let mut v = serde_json::to_value(a).expect("serialisable artifact");
    v["owner_live"] = json!(owner.is_some_and(|o| o.ended_at.is_none()));
    v["owner_harness"] = json!(owner.map(|o| &o.harness));
    v["working"] = json!(working);
    v
}

/// Each live artifact, with the owner fields of [`with_owner`].
pub async fn list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let all = s.working.all();
    let artifacts = s
        .store_call(move |st| {
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
                let working = all.get(&a.id).map(Vec::as_slice).unwrap_or(&[]);
                out.push(with_owner(&a, owner.as_ref(), working));
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
    let p = validate(body_within(req, "the publish limit")?)?;
    require_title(p.title.as_deref())?;
    let session = session_header(&headers)?;
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session)?;
            let (artifact, version) = st.create_artifact(p, session.as_deref())?;
            if let Some(sid) = &session {
                let aid = ArtifactId::parse(&artifact.id)?;
                st.ensure_watch(sid, &aid)?;
                let touched = st.retarget_untargeted(&aid, sid)?;
                crate::feedback::apply(&ctx, st, &touched);
            }
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page: false,
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
    let working = s.working.for_artifact(&artifact.id);
    Ok(Json(
        json!({"artifact": with_owner(&artifact, owner.as_ref(), &working), "versions": versions}),
    ))
}

/// Metadata edits. `capabilities` replaces the whole declaration (omitted
/// keeps, `{}` clears) and is validated like a publish's.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    title: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    pinned: Option<bool>,
    capabilities: Option<Value>,
}

pub async fn patch(
    State(s): State<AppState>,
    _t: RequireToken,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<PatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    if let Some(c) = &b.capabilities {
        clax_core::capabilities::validate(c)?;
    }
    let artifact = s
        .store_call(move |st| {
            st.update_meta(
                &id,
                MetaPatch {
                    title: b.title,
                    description: b.description,
                    icon: b.icon,
                    pinned: b.pinned,
                    capabilities: b.capabilities,
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
    let working = s.working.clone();
    s.store_call(move |st| {
        st.delete_artifact(&id)?;
        crate::working::announce(&events, &working, &working.artifact_gone(id.as_str()));
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
    let p = validate(body_within(req, "the publish limit")?)?;
    let session = session_header(&headers)?;
    let by_page = headers.get(VIA_HEADER).and_then(|v| v.to_str().ok()) == Some("page");
    let events = s.events.clone();
    let ctx = s.feedback_ctx();
    let (artifact, version) = s
        .store_call(move |st| {
            let session = publishing_session(st, &session)?;
            let (artifact, version) = st.publish_version(&id, p, session.as_deref())?;
            if let Some(sid) = &session {
                let aid = ArtifactId::parse(&artifact.id)?;
                st.ensure_watch(sid, &aid)?;
                let changed = ctx.working.clear(sid, artifact.id.as_str(), None);
                crate::working::announce(&ctx.events, &ctx.working, &changed);
                let touched = st.retarget_untargeted(&aid, sid)?;
                crate::feedback::apply(&ctx, st, &touched);
            }
            events.publish(Event::Version {
                artifact_id: artifact.id.clone(),
                n: version.n,
                by_page,
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
