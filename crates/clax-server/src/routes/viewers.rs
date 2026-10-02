//! `GET/PUT /api/viewers/me`: the browser viewer behind the `clax_viewer`
//! cookie; `GET/PUT /api/viewers/me/seen`: its version seen marks;
//! `GET /api/viewers/me/attention` and `PUT /api/viewers/me/looked`: its
//! attention per artifact and its looked-at marks on threads;
//! `PUT /api/viewers/me/presence`: its presence on an artifact;
//! `GET /api/viewers`: other viewers by public ID or name. All refuse
//! a request with a foreign `Origin` ([`SameOrigin`]).

use super::artifacts::{body, parse_id};
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::{SameOrigin, ViewerCookie, set_cookie};
use axum::Json;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use clax_core::{CoreError, new_ulid};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameBody {
    display_name: String,
}

async fn respond(
    s: AppState,
    cookie: Option<String>,
    name: Option<String>,
) -> Result<Response, ApiError> {
    let (id, fresh) = match cookie {
        Some(id) => (id, false),
        None => (new_ulid(), true),
    };
    let id2 = id.clone();
    let viewer = s
        .store_call(move |st| st.upsert_viewer(&id2, name.as_deref()))
        .await?;
    let mut res = Json(json!({"viewer": viewer})).into_response();
    if fresh {
        res.headers_mut().insert(SET_COOKIE, set_cookie(&id));
    }
    Ok(res)
}

/// The viewer, created (and its cookie set) on first contact.
pub async fn me(
    State(s): State<AppState>,
    _o: SameOrigin,
    ViewerCookie(c): ViewerCookie,
) -> Result<Response, ApiError> {
    respond(s, c, None).await
}

/// Sets the display name; an empty name clears it.
pub async fn set_me(
    State(s): State<AppState>,
    _o: SameOrigin,
    ViewerCookie(c): ViewerCookie,
    req: Result<Json<NameBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let b = body(req)?;
    respond(s, c, Some(b.display_name)).await
}

/// Most public IDs one lookup takes.
pub const MAX_LOOKUP_IDS: usize = 64;
/// Most viewers a name search returns.
pub const MAX_SEARCH: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupQuery {
    ids: Option<String>,
    q: Option<String>,
}

/// `GET /api/viewers?ids=<comma-separated public IDs>` (at most
/// [`MAX_LOOKUP_IDS`]) names the viewers a page refers to; unknown IDs are
/// left out. `GET /api/viewers?q=<text>` (token only) returns up to
/// [`MAX_SEARCH`] named viewers whose name contains the text, ignoring case.
/// Both answer `{viewers: [{id, display_name}]}` with public IDs, never
/// cookies.
pub async fn lookup(
    State(s): State<AppState>,
    _o: SameOrigin,
    headers: HeaderMap,
    q: Result<Query<LookupQuery>, QueryRejection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let viewers = match (q.ids, q.q) {
        (Some(ids), None) => {
            let ids: Vec<String> = ids
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if ids.len() > MAX_LOOKUP_IDS {
                return Err(ApiError::bad_request(
                    "invalid_argument",
                    format!("at most {MAX_LOOKUP_IDS} IDs per lookup"),
                ));
            }
            if ids.iter().any(|i| !clax_core::is_public_id(i)) {
                return Err(ApiError::bad_request(
                    "invalid_argument",
                    "ids takes viewer public IDs",
                ));
            }
            s.store_call(move |st| st.viewers_by_public_ids(&ids))
                .await?
        }
        (None, Some(text)) => {
            if !crate::auth::has_token(&headers, &s.token) {
                return Err(ApiError::unauthorized());
            }
            let text = text.trim().to_string();
            // No stored name is longer than MAX_NAME_CHARS, so a longer query matches none.
            if text.is_empty() || text.chars().count() > clax_core::store::viewers::MAX_NAME_CHARS {
                Vec::new()
            } else {
                s.store_call(move |st| st.search_viewers(&text, MAX_SEARCH))
                    .await?
            }
        }
        _ => {
            return Err(ApiError::bad_request(
                "invalid_argument",
                "pass exactly one of ids and q",
            ));
        }
    };
    Ok(Json(json!({
        "viewers": viewers
            .iter()
            .map(|v| json!({"id": v.public_id, "display_name": v.display_name}))
            .collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
pub struct SeenQuery {
    artifact: String,
}

/// `GET /api/viewers/me/seen?artifact=<aid>`: `{seen}`, the highest version
/// this viewer has viewed unpinned; null for none or no cookie.
pub async fn seen(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    q: Result<Query<SeenQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let id = parse_id(&q.artifact)?;
    let n = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            match viewer.0.as_deref() {
                Some(cookie) => match st.get_viewer(cookie)? {
                    Some(v) => st.seen(&v.id, &id),
                    None => Ok(None),
                },
                None => Ok(None),
            }
        })
        .await?;
    Ok(Json(json!({"seen": n})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeenBody {
    artifact_id: String,
    version: u32,
}

/// `PUT /api/viewers/me/seen`: raises the mark (never lowers it); `{seen}`.
/// 400 `no_viewer` without a viewer cookie.
pub async fn set_seen(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    req: Result<Json<SeenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    let cookie = viewer
        .0
        .ok_or_else(|| ApiError::bad_request("no_viewer", "open /api/viewers/me first"))?;
    let n = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = st.upsert_viewer(&cookie, None)?;
            st.mark_seen(&v.id, &id, b.version)
        })
        .await?;
    Ok(Json(json!({"seen": n})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookedBody {
    artifact_id: String,
    thread_ids: Vec<String>,
}

/// `GET /api/viewers/me/attention`: this viewer's attention on every live
/// artifact, without looked-at times; `{artifacts: {}}` without a cookie.
pub async fn attention(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
) -> Result<Response, ApiError> {
    let out = match viewer.0 {
        Some(v) => {
            s.store_call(move |st| {
                Ok(match st.get_viewer(&v)? {
                    Some(_) => json!(st.attention_all(&v)?),
                    None => json!({}),
                })
            })
            .await?
        }
        None => json!({}),
    };
    Ok((
        [
            (header::CACHE_CONTROL, "private, no-cache"),
            (header::VARY, "Cookie"),
        ],
        Json(json!({"artifacts": out})),
    )
        .into_response())
}

/// `PUT /api/viewers/me/looked`: records that this viewer looked at the
/// threads now (spec §10, "Participants and attention"); `{looked}`, the
/// viewer's marks on the artifact. 400 `no_viewer` without a viewer cookie.
pub async fn set_looked(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    req: Result<Json<LookedBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    let max = clax_core::store::attention::MAX_LOOKED;
    if b.thread_ids.is_empty()
        || b.thread_ids.len() > max
        || !b.thread_ids.iter().all(|t| clax_core::is_ulid(t))
    {
        return Err(ApiError::bad_request(
            "invalid_args",
            format!("thread_ids: 1 to {max} thread IDs"),
        ));
    }
    let Some(vid) = viewer.0 else {
        return Err(ApiError::bad_request(
            "no_viewer",
            "open /api/viewers/me first",
        ));
    };
    let looked = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.get_viewer(&vid)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
            st.mark_looked(&vid, &id, &b.thread_ids)
        })
        .await?;
    Ok(Json(json!({"looked": looked})))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    Here,
    Away,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceBody {
    artifact_id: String,
    state: ReportedState,
    #[serde(default)]
    r#where: Option<String>,
}

/// `PUT /api/viewers/me/presence`: reports this viewer here or away on an
/// artifact, with where they look when they share it (spec §10, "Presence");
/// `{people}`, the artifact's presence. Announces a `presence` event when the
/// report changed what others see. 400 `no_viewer` without a viewer cookie.
pub async fn set_presence(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    req: Result<Json<PresenceBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    use clax_core::presence::State as P;
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    let Some(vid) = viewer.0 else {
        return Err(ApiError::bad_request(
            "no_viewer",
            "open /api/viewers/me first",
        ));
    };
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            st.get_viewer(&vid)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))
        })
        .await?;
    let state = match b.state {
        ReportedState::Here => P::Here,
        ReportedState::Away => P::Away,
    };
    let changed = s.presence.report(
        &b.artifact_id,
        &v.public_id,
        v.display_name.as_deref(),
        state,
        b.r#where.as_deref(),
    );
    if changed {
        crate::presence::announce(&s.events, &s.presence, &b.artifact_id);
    }
    Ok(Json(
        json!({"people": s.presence.for_artifact(&b.artifact_id)}),
    ))
}
