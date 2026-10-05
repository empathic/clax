//! `GET/PUT /api/viewers/me`: the viewer the request speaks for
//! ([`Identity`]: the owner for an owner credential, else the browser viewer
//! behind the `clax_viewer` cookie); `GET/PUT /api/viewers/me/seen`: its
//! version seen marks;
//! `GET /api/viewers/me/attention` and `PUT /api/viewers/me/looked`: its
//! attention per artifact and its looked-at marks on threads;
//! `PUT /api/viewers/me/presence`: its presence on an artifact;
//! `GET /api/viewers`: other viewers by public ID or name. All refuse
//! a request with a foreign `Origin` ([`SameOrigin`]).

use super::artifacts::{body, parse_id};
use crate::error::ApiError;
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::{SameOrigin, set_cookie};
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

async fn respond(s: AppState, who: Identity, name: Option<String>) -> Result<Response, ApiError> {
    let fresh = (!who.is_owner() && who.cookie.is_none()).then(new_ulid);
    let minted = fresh.clone();
    let (before, viewer) = s
        .store_call(move |st| {
            if who.is_owner() {
                // The token alone reads the owner without making one.
                let before = who.viewer(st)?;
                let after = match &name {
                    Some(n) => Some(st.set_owner_name(n, who.owner_browser())?),
                    None => before.clone(),
                };
                return Ok((before.and_then(|v| v.display_name), after));
            }
            let id = match minted {
                Some(m) => {
                    st.mint_viewer(&m, who.local)?;
                    m
                }
                None => who.cookie.clone().unwrap_or_default(),
            };
            let before = st.get_viewer(&id)?.and_then(|v| v.display_name);
            Ok((before, Some(st.upsert_viewer(&id, name.as_deref())?)))
        })
        .await?;
    if let Some(v) = &viewer
        && before != v.display_name
    {
        for aid in s.presence.rename(&v.public_id, v.display_name.as_deref()) {
            crate::presence::announce(&s.events, &s.presence, &aid);
        }
    }
    let mut res = Json(json!({"viewer": viewer})).into_response();
    if let Some(id) = fresh {
        res.headers_mut().insert(SET_COOKIE, set_cookie(&id));
    }
    Ok(res)
}

/// The viewer the request speaks for ([`Identity`]): the owner for an owner
/// credential (the token, or the owner cookie of the owner's browsers; `null`
/// for the token alone before the owner exists), else the cookie's viewer,
/// created (and its cookie set, noting whether it was minted on this
/// machine) on first contact.
pub async fn me(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
) -> Result<Response, ApiError> {
    respond(s, who, None).await
}

/// Sets the display name of the viewer [`me`] answers (for the owner, the
/// owner's one name, in every browser and the CLI); an empty name clears it.
/// A change reaches everyone at once through the presence of every artifact
/// that lists the viewer.
pub async fn set_me(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    req: Result<Json<NameBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let b = body(req)?;
    respond(s, who, Some(b.display_name)).await
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
    who: Identity,
    sees: crate::live::SeesLive,
    q: Result<Query<SeenQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let id = parse_id(&q.artifact)?;
    sees.check(&s.live_ids, id.as_str())?;
    let n = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            match who.viewer(st)? {
                Some(v) => st.seen(&v.id, &id),
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
    who: Identity,
    sees: crate::live::SeesLive,
    req: Result<Json<SeenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    sees.check(&s.live_ids, id.as_str())?;
    if !who.is_owner() && who.cookie.is_none() {
        return Err(ApiError::bad_request(
            "no_viewer",
            "open /api/viewers/me first",
        ));
    }
    let n = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = who
                .ensure_viewer(st)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
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

#[derive(Deserialize)]
pub struct AttentionQuery {
    artifact: Option<String>,
}

/// `GET /api/viewers/me/attention`: this viewer's attention on every live
/// artifact, without looked-at times; `{artifacts: {}}` without a cookie.
/// Live pages are left out for a request that may not see them (and a
/// `?artifact=` naming one is 404), as on every viewer route here.
/// With `?artifact=<aid>`, on that artifact alone: `{artifacts: {<aid>:
/// ...}}`, or `{artifacts: {}}` when it is not live (400 for a malformed ID).
pub async fn attention(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    sees: crate::live::SeesLive,
    q: Result<Query<AttentionQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let one = q.artifact.as_deref().map(parse_id).transpose()?;
    if let Some(id) = &one {
        sees.check(&s.live_ids, id.as_str())?;
    }
    let hidden = (!sees.0).then(|| s.live_ids.clone());
    let mut out = s
        .store_call(move |st| {
            Ok(match (who.viewer(st)?, one) {
                (None, _) => json!({}),
                (Some(v), None) => json!(st.attention_all(&v.id)?),
                (Some(v), Some(id)) => match st.attention_one(&v.id, &id)? {
                    Some(a) => Value::Object([(id.to_string(), json!(a))].into_iter().collect()),
                    None => json!({}),
                },
            })
        })
        .await?;
    if let (Some(live), Some(map)) = (hidden, out.as_object_mut()) {
        map.retain(|aid, _| !live.contains(aid));
    }
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
    who: Identity,
    sees: crate::live::SeesLive,
    req: Result<Json<LookedBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    sees.check(&s.live_ids, id.as_str())?;
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
    let looked = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = who
                .viewer(st)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
            st.mark_looked(&v.id, &id, &b.thread_ids)
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
    /// Which of the viewer's open views reports (any short string); reports
    /// from several tabs or browsers of one viewer combine.
    #[serde(default)]
    tab: Option<String>,
}

/// `PUT /api/viewers/me/presence`: reports this viewer here or away on an
/// artifact, with where they look when they share it (spec §10, "Presence");
/// `{people}`, the artifact's presence. Announces a `presence` event when the
/// report changed what others see. 400 `no_viewer` without a viewer cookie;
/// 429 `limit_reached` when the artifact already lists
/// [`clax_core::presence::MAX_PEOPLE`] others and none of them is gone.
pub async fn set_presence(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    sees: crate::live::SeesLive,
    req: Result<Json<PresenceBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    use clax_core::presence::State as P;
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    sees.check(&s.live_ids, id.as_str())?;
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            who.viewer(st)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))
        })
        .await?;
    let state = match b.state {
        ReportedState::Here => P::Here,
        ReportedState::Away => P::Away,
    };
    let changed = s
        .presence
        .report_tab(
            &b.artifact_id,
            &v.public_id,
            v.display_name.as_deref(),
            state,
            b.r#where.as_deref(),
            b.tab.as_deref().unwrap_or(""),
        )
        .ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "limit_reached",
                format!(
                    "this artifact already lists {} people",
                    clax_core::presence::MAX_PEOPLE
                ),
            )
        })?;
    if changed {
        crate::presence::announce(&s.events, &s.presence, &b.artifact_id);
    }
    Ok(Json(
        json!({"people": s.presence.for_artifact(&b.artifact_id)}),
    ))
}
