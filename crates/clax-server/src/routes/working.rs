//! Working routes (spec §6, §10 "Working").

use super::artifacts::{body, parse_id, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use crate::working::announce;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::model::Session;
use clax_core::working::{Actor, MAX_WORKING_THREADS, SetWorking, clean_message};
use clax_core::{ArtifactId, CoreError, Store};
use serde::Deserialize;
use serde_json::{Value, json};

/// The live session `sid`: 404 when unknown, 400 `unknown_session` when ended.
fn live(st: &Store, sid: &str) -> clax_core::Result<Session> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(s) if s.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "the session has ended",
        )),
        Some(s) => Ok(s),
    }
}

/// Checks `ids` (at most [`MAX_WORKING_THREADS`] ULIDs) are open threads of `aid`.
pub(crate) fn check_threads(st: &Store, aid: &ArtifactId, ids: &[String]) -> clax_core::Result<()> {
    if ids.len() > MAX_WORKING_THREADS {
        return Err(CoreError::invalid(
            "invalid_args",
            format!("at most {MAX_WORKING_THREADS} thread_ids"),
        ));
    }
    for tid in ids {
        if !clax_core::is_ulid(tid) {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("'{tid}' is not a thread ID"),
            ));
        }
        match st.get_thread(tid)? {
            Some(t) if t.artifact_id == aid.as_str() => {
                if t.status != "open" {
                    return Err(CoreError::invalid(
                        "thread_not_open",
                        format!("thread {tid} is resolved"),
                    ));
                }
            }
            _ => {
                return Err(CoreError::invalid(
                    "unknown_thread",
                    format!("{tid} is not a thread of {aid}"),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SetBody {
    #[serde(default)]
    thread_ids: Option<Vec<String>>,
    #[serde(default)]
    message: Option<String>,
}

/// `PUT /api/sessions/<sid>/working/<aid>` (W): creates or updates the
/// record; given fields replace. `{working, message_truncated}`.
pub async fn put(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<SetBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let (message, truncated) = match b.message.as_deref() {
        Some(m) => clean_message(m),
        None => (None, false),
    };
    let (w, events) = (s.working.clone(), s.events.clone());
    let view = s
        .store_call(move |st| {
            let sess = live(st, &sid)?;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            if let Some(t) = &b.thread_ids {
                check_threads(st, &id, t)?;
            }
            let who = Actor {
                session_id: sess.id,
                harness: sess.harness,
            };
            let (view, changed) = w.set(
                &who,
                id.as_str(),
                SetWorking {
                    thread_ids: b.thread_ids,
                    message,
                },
            );
            announce(&events, &w, &changed);
            Ok(view)
        })
        .await?;
    Ok(Json(
        json!({"working": view, "message_truncated": truncated}),
    ))
}

#[derive(Deserialize)]
pub struct ClearQuery {
    thread_ids: Option<String>,
}

/// `DELETE /api/sessions/<sid>/working/<aid>` (W): the record, or with
/// `?thread_ids=a,b` only those threads. `{cleared, working}` (`working` is
/// what remains, or null).
pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<ClearQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let threads: Option<Vec<String>> = q.thread_ids.map(|t| {
        t.split(',')
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    });
    let (w, events) = (s.working.clone(), s.events.clone());
    let (cleared, left) = s
        .store_call(move |st| {
            live(st, &sid)?;
            let changed = w.clear(&sid, id.as_str(), threads.as_deref());
            announce(&events, &w, &changed);
            let left = w
                .for_session(&sid)
                .into_iter()
                .find(|r| r.artifact_id == id.as_str());
            Ok((!changed.is_empty(), left))
        })
        .await?;
    Ok(Json(json!({"cleared": cleared, "working": left})))
}

/// `GET /api/sessions/<sid>/working` (token): `{working}` with session fields.
pub async fn for_session(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let w = s.working.clone();
    let list = s
        .store_call(move |st| {
            st.get_session(&sid)?.ok_or(CoreError::NotFound)?;
            Ok(w.for_session(&sid))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `GET /api/artifacts/<aid>/working` (no token): `{working}`, never naming a session.
pub async fn for_artifact(
    State(s): State<AppState>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let w = s.working.clone();
    let list = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            Ok(w.for_artifact(id.as_str()))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `POST /api/sessions/<sid>/working/renew` (W): `{renewed}`.
pub async fn renew(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let w = s.working.clone();
    let n = s
        .store_call(move |st| {
            live(st, &sid)?;
            Ok(w.renew(&sid))
        })
        .await?;
    Ok(Json(json!({"renewed": n})))
}

/// `POST /api/sessions/<sid>/working/end` (W; the turn ended): `{cleared}`.
pub async fn end(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let (w, events) = (s.working.clone(), s.events.clone());
    let n = s
        .store_call(move |st| {
            live(st, &sid)?;
            let changed = w.end_session(&sid);
            announce(&events, &w, &changed);
            Ok(changed.0.len())
        })
        .await?;
    Ok(Json(json!({"cleared": n})))
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
pub struct SkewBody {
    secs: i64,
}

/// Debug builds only: moves the working clock forward and sweeps, so browser
/// tests can observe expiry without waiting. `{now}`.
#[cfg(debug_assertions)]
pub async fn skew(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<SkewBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    s.working.skew(b.secs);
    crate::working::sweep_and_announce(&s.working, &s.events);
    Ok(Json(json!({"now": s.working.now().to_rfc3339()})))
}
