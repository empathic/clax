//! Working routes (spec §6, §10 "Working").

use super::artifacts::{body, path};
use crate::agent::TokenOrAgent;
use crate::audit::DeferredAudit;
#[cfg(debug_assertions)]
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::headers::parse_id;
use crate::state::AppState;
use crate::working::settle;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::model::Session;
use clax_core::working::{Actor, End, Ended, MAX_WORKING_THREADS, SetWorking, clean_message};
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

/// Records each of `ended` (work its agent said it finished) as a
/// `finished` inbox item. The records have ended either way; an item that
/// could not be written is logged.
fn finished(st: &Store, ended: &[Ended], harness: &str) {
    for e in ended {
        if let Err(err) = st.note_finished(e, harness) {
            tracing::warn!(error = %err, "finished work not recorded in the inbox");
        }
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
    let statuses = st.thread_statuses(aid, ids)?;
    for tid in ids {
        if !clax_core::is_ulid(tid) {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("'{tid}' is not a thread ID"),
            ));
        }
        match statuses.get(tid).map(String::as_str) {
            Some("open") => {}
            Some(_) => {
                return Err(CoreError::invalid(
                    "thread_not_open",
                    format!("thread {tid} is resolved"),
                ));
            }
            None => {
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
/// record; given fields replace. `{working, message_truncated}`. A new
/// record records `working.start` as the session's agent.
pub async fn put(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::WorkingPut>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<SetBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, _) = path(p)?;
    let id = a.artifact()?;
    let b = body(req)?;
    let (message, truncated) = match b.message.as_deref() {
        Some(m) => clean_message(m),
        None => (None, false),
    };
    let (w, events) = (s.working.clone(), s.events.clone());
    let view = s
        .store_call(move |st| {
            let sess = live(st, &sid)?;
            let audit = audit.for_session(st, &sid)?;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            if let Some(t) = &b.thread_ids {
                check_threads(st, &id, t)?;
            }
            let who = Actor {
                session_id: sess.id,
                harness: sess.harness,
                agent: sess.agent_handle,
            };
            let (view, changed) = w.set(
                &who,
                id.as_str(),
                SetWorking {
                    thread_ids: b.thread_ids,
                    message,
                },
            );
            settle(st, &audit, &events, &w, &changed);
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
/// what remains, or null). A record this ends is finished work: a
/// `finished` inbox item; it records `working.stop` as the session's agent.
pub async fn delete(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::WorkingDelete>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<ClearQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, _) = path(p)?;
    let id = a.artifact()?;
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
            let sess = live(st, &sid)?;
            let audit = audit.for_session(st, &sid)?;
            let (changed, ended) = w.clear(&sid, id.as_str(), threads.as_deref(), End::Done);
            settle(st, &audit, &events, &w, &changed);
            finished(st, &ended, &sess.harness);
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
    _a: TokenOrAgent<crate::agent::WorkingForSession>,
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

/// `GET /api/working` (token): `{working}`, every live record with its
/// session and artifact (the working roster), newest `started_at` first.
/// Live pages are left out for a caller that may not see them
/// ([`crate::live_access::sees_live_pages`]); for a person's agent, so is every
/// artifact it may not read, and the session of another credential is not
/// named (spec §6.7). Each message is filtered for an agent channel's
/// reader.
pub async fn roster(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<crate::agent::WorkingRoster>,
    extensions: axum::http::Extensions,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let local = crate::live_access::sees_live_pages(&headers, &extensions, &s.token);
    let reads = a.read_rights();
    let grant = a.grant().map(str::to_string);
    let vf = crate::views::thread::VisFor::of(&a.caller, &s);
    let records = s.working.roster();
    let list = s
        .store_call(move |st| {
            let mut kept = Vec::new();
            let mut grants = std::collections::HashMap::new();
            for r in records {
                let Some(art) = st.get_artifact(&ArtifactId::parse(&r.artifact_id)?)? else {
                    continue;
                };
                if (!local && art.kind == clax_core::live::KIND_LIVE) || !reads.allows(&art) {
                    continue;
                }
                kept.push(r);
            }
            let vis = vf.build(st)?;
            let mut views = vis.filter_roster(st, &kept)?;
            if reads.is_person_agent() {
                for (r, v) in kept.iter().zip(&mut views) {
                    if !grants.contains_key(&r.session_id) {
                        let g = st.get_session(&r.session_id)?.and_then(|x| x.grant_id);
                        grants.insert(r.session_id.clone(), g);
                    }
                    if grants[&r.session_id] != grant {
                        v["session_id"] = Value::Null;
                    }
                }
            }
            Ok(views)
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `GET /api/artifacts/<aid>/working` (no token): `{working}`, never naming
/// a session; each message filtered for an agent channel's reader.
pub async fn for_artifact(
    State(s): State<AppState>,
    a: crate::agent::Agent<crate::agent::WorkingForArtifact>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let w = s.working.clone();
    let vf = crate::views::thread::VisFor::of(&a.caller, &s);
    let list = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let vis = vf.build(st)?;
            vis.filter_working(st, id.as_str(), &w.for_artifact(id.as_str()))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `POST /api/sessions/<sid>/working/renew` (W): `{renewed}`.
pub async fn renew(
    State(s): State<AppState>,
    _a: TokenOrAgent<crate::agent::WorkingRenew>,
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
/// Each record it ends is finished work: a `finished` inbox item, and
/// records `working.stop` (reason `explicit`) as the session's agent.
pub async fn end(
    State(s): State<AppState>,
    _a: TokenOrAgent<crate::agent::WorkingEnd>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let (w, events) = (s.working.clone(), s.events.clone());
    let n = s
        .store_call(move |st| {
            let sess = live(st, &sid)?;
            let audit = audit.for_session(st, &sid)?;
            let (changed, ended) = w.end_session(&sid, End::TurnEnd);
            settle(st, &audit, &events, &w, &changed);
            finished(st, &ended, &sess.harness);
            Ok(changed.artifacts.len())
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
    _d: crate::agent::DeniedToGrants,
    _t: RequireToken,
    req: Result<Json<SkewBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    s.working.skew(b.secs);
    crate::working::sweep_and_announce(&s.store, &s.working, &s.events).await;
    Ok(Json(json!({"now": s.working.now().to_rfc3339()})))
}
