//! `GET /api/artifacts/<aid>/snapshot[?tab=<ID>]`: the spectator snapshot
//! (spec 2026-10-11-clax-dev-hosted-design §16.2). A person beyond the
//! artifact's live seats holds no live connection ([`crate::seats`]); their
//! page polls this every 5 s while visible instead.
//!
//! The answer is the artifact as any person reads it, with nothing of the
//! caller's own (no attention, no looked or seen marks of theirs, no session
//! IDs): `{artifact, versions, threads, presence, viewing, seats_free,
//! guest_seats_free, built_at}`, where `threads` lists every thread,
//! resolved ones included, `viewing` counts the people seated and the
//! spectators seen in the last 15 s, `seats_free` the people seats free for
//! a connected person and `guest_seats_free` those free for a guest, and
//! `built_at` is the daemon's time of the build in ms. Every caller gets
//! the same bytes, so a build is shared for 5 s per artifact and head
//! version: `x-clax-cache` says `hit` or `miss`, and a new version is a
//! miss at once.
//!
//! People only: agents are never spectators and read through the filtered
//! routes (403 `not_for_agents` for the owner's, 403 `forbidden` for a
//! person's). 404 for a missing artifact or a live page the request may not
//! see. A caller without a seat is noted as viewing: a person or a named
//! guest by their seat key, anyone else by `tab` (an ID its page picks,
//! at most 32 letters, digits, `-` or `_`), counted per client address.

use super::artifacts::{live_part, parse_id, path, strip_sessions, with_owner};
use crate::error::ApiError;
use crate::feedback::VisFor;
use crate::identity::Identity;
use crate::state::AppState;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct SnapshotQuery {
    tab: Option<String>,
}

/// Threads read per page while building.
const PAGE: usize = 200;

fn tab_ok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 32
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub async fn snapshot(
    State(s): State<AppState>,
    d: crate::agent::DeniedToGrants,
    who: Identity,
    sees: crate::live::SeesLive,
    extensions: axum::http::Extensions,
    aid: Result<Path<String>, PathRejection>,
    q: Result<Query<SnapshotQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    d.refuse_agents()?;
    let id = parse_id(&path(aid)?)?;
    sees.check(&s.live_ids, id.as_str())?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let lookup = id.clone();
    let (head, key) = s
        .store_call(move |st| {
            let a = st.get_artifact(&lookup)?.ok_or(CoreError::NotFound)?;
            let key = who
                .caller(st, None)
                .ok()
                .and_then(|c| crate::seats::key_for(&c));
            Ok((a.current_version, key))
        })
        .await?;
    let spectator = key.or_else(|| {
        let client =
            crate::identity::peer_of(&extensions).map_or_else(String::new, |p| p.ip().to_string());
        q.tab
            .as_deref()
            .filter(|t| tab_ok(t))
            .map(|t| format!("tab:{client}:{t}"))
    });
    if let Some(k) = spectator
        && !s.seats.is_seated(id.as_str(), &k)
    {
        s.seats.see_spectator(id.as_str(), &k);
    }
    if let Some(body) = s.seats.shared_snapshot(id.as_str(), head) {
        return Ok(respond(body, "hit"));
    }
    // Requests that miss together share one build.
    let lock = s.seats.build_lock(id.as_str());
    let _building = lock.lock().await;
    if let Some(body) = s.seats.shared_snapshot(id.as_str(), head) {
        return Ok(respond(body, "hit"));
    }
    let working = s.working.for_artifact(id.as_str());
    let presence = s.presence.for_artifact(id.as_str());
    let vf = VisFor::of(&crate::auth::Caller::Anonymous, &s);
    let codex = s.feedback_ctx().codex_push();
    let build = id.clone();
    let mut v = s
        .store_call(move |st| {
            let id = build;
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let owner = match &a.owner_session_id {
                Some(sid) => st.get_session(sid)?,
                None => None,
            };
            let artifact = with_owner(
                &a,
                owner.as_ref(),
                &working,
                &st.participants(&id)?,
                live_part(st, &a)?.as_ref(),
            );
            let vis = vf.build(st)?;
            let mut threads = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let (ts, next) = st.list_threads(&id, true, cursor.as_deref(), PAGE)?;
                threads.extend(vis.filter_threads(st, &ts, codex, false)?);
                match next {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            Ok(json!({
                "artifact": artifact,
                "versions": st.list_versions(&id)?,
                "threads": threads,
                "presence": presence,
            }))
        })
        .await?;
    strip_sessions(&mut v);
    let counts = s.seats.counts(id.as_str());
    v["viewing"] = json!(counts.viewing);
    v["seats_free"] = json!(counts.seats_free);
    v["guest_seats_free"] = json!(counts.guest_seats_free);
    v["built_at"] = json!(s.seats.now_ms());
    let body = Arc::new(v.to_string().into_bytes());
    s.seats.keep_snapshot(id.as_str(), head, body.clone());
    Ok(respond(body, "miss"))
}

fn respond(body: Arc<Vec<u8>>, cache: &'static str) -> Response {
    let mut res = (*body).clone().into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert("x-clax-cache", HeaderValue::from_static(cache));
    res
}
