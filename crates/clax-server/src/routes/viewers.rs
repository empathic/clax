//! `GET/PUT /api/viewers/me`: the viewer the request speaks for
//! ([`Identity`]: the owner for an owner credential, else the browser viewer
//! behind the `clax_viewer` cookie); `GET/PUT /api/viewers/me/seen`: its
//! version seen marks;
//! `GET /api/viewers/me/attention` and `PUT /api/viewers/me/looked`: its
//! attention per artifact and its looked-at marks on threads;
//! `PUT /api/viewers/me/presence`: its presence on an artifact;
//! `GET /api/viewers`: other viewers by public ID or name. All refuse
//! a request with a foreign `Origin` ([`SameOrigin`]).

use super::artifacts::body;
use axum::Json;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::header;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Response};
use clax_core::CoreError;
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::{SameOrigin, set_cookie};
use clax_store::viewers::is_legacy_cookie;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameBody {
    display_name: String,
}

/// Whether the request's caller may start an artifact from a brief
/// (`POST /api/artifacts:new`, [`super::artifacts::brief_refusal`]): the
/// gallery offers New only then.
async fn may_create(s: &AppState, who: &Identity) -> Result<bool, ApiError> {
    let who = who.clone();
    s.store_call(move |st| {
        let Ok(c) = who.caller(st, None) else {
            return Ok(false);
        };
        Ok(super::artifacts::brief_refusal(st, &c)?.is_none())
    })
    .await
}

async fn respond(s: AppState, who: Identity, name: Option<String>) -> Result<Response, ApiError> {
    let may_create = may_create(&s, &who).await?;
    // A hosted session's viewer is signed in already: it is answered as it
    // is, and no `clax_viewer` cookie is minted (the hosted gate renames a
    // guest itself).
    if let Some(v) = &who.hosted_viewer
        && name.is_none()
    {
        return Ok(Json(json!({"viewer": v, "may_create": may_create})).into_response());
    }
    let owner = who.is_owner();
    let (before, viewer, fresh) = s
        .store_call(move |st| {
            if who.is_owner() {
                // The token alone reads the owner without making one.
                let before = who.viewer(st)?;
                let after = match &name {
                    Some(n) => Some(st.set_owner_name(n, who.owner_browser())?),
                    None => before.clone(),
                };
                return Ok((before.and_then(|v| v.display_name), after, None));
            }
            // A browser without a cookie, or with one that names no viewer
            // (a value the daemon never minted, say one another local
            // server planted), gets a new viewer and a fresh secret: a
            // cookie is never adopted. One holding a legacy cookie (its
            // viewer's row ID, from before migration 24) gets a fresh
            // secret for the same viewer, also while that cookie's
            // replacement is recent (spec §21.4).
            let mint = |st: &clax_store::Store| -> clax_core::Result<clax_core::secret::Secret> {
                let c = clax_core::secret::viewer_cookie();
                st.mint_viewer(c.expose(), who.local)?;
                Ok(c)
            };
            let fresh = match who.cookie.as_deref() {
                None => Some(mint(st)?),
                Some(c) if is_legacy_cookie(c) => match st.reissue_viewer_cookie(c)? {
                    Some((_, secret)) => Some(secret),
                    None => Some(mint(st)?),
                },
                Some(c) if st.get_viewer(c)?.is_some() => None,
                Some(_) => Some(mint(st)?),
            };
            let cookie = fresh
                .as_ref()
                .map(|f| f.expose().to_string())
                .or_else(|| who.cookie.clone())
                .unwrap_or_default();
            let before = st.get_viewer(&cookie)?.and_then(|v| v.display_name);
            Ok((
                before,
                Some(st.upsert_viewer(&cookie, name.as_deref())?),
                fresh,
            ))
        })
        .await?;
    if let Some(v) = &viewer
        && before != v.display_name
    {
        for aid in s.presence.rename(&v.public_id, v.display_name.as_deref()) {
            clax_realtime::presence::announce(&s.events, &s.presence, &aid);
        }
    }
    // `owner` marks the owner's browser (its agents are "Your agents", §10).
    let mut viewer = json!(viewer);
    if owner && viewer.is_object() {
        viewer["owner"] = json!(true);
    }
    let mut res = Json(json!({"viewer": viewer, "may_create": may_create})).into_response();
    if let Some(c) = fresh {
        res.headers_mut().insert(SET_COOKIE, set_cookie(&c));
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
    _d: clax_http::agent::DeniedToGrants,
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
    _d: clax_http::agent::DeniedToGrants,
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
/// cookies; to the owner's agents, a viewer who is not connected is named
/// `<name> (not connected)`.
pub async fn lookup(
    State(s): State<AppState>,
    d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
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
            if !d.holds_token() {
                return Err(ApiError::unauthorized());
            }
            let text = text.trim().to_string();
            // No stored name is longer than MAX_NAME_CHARS, so a longer query matches none.
            if text.is_empty() || text.chars().count() > clax_store::viewers::MAX_NAME_CHARS {
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
    let mut viewers = json!(
        viewers
            .iter()
            .map(|v| json!({"id": v.public_id, "display_name": v.display_name}))
            .collect::<Vec<_>>()
    );
    // The owner's agents read a viewer who is not connected as `<name> (not
    // connected)` (spec 2026-10-08 §9.6).
    let vf = clax_http::views::thread::VisFor::of(&d.caller(&s).await?, &s);
    let viewers = s
        .store_call(move |st| {
            vf.build(st)?.mark_viewers(&mut viewers, "id");
            Ok(viewers)
        })
        .await?;
    Ok(Json(json!({ "viewers": viewers })))
}

#[derive(Deserialize)]
pub struct SeenQuery {
    artifact: String,
}

/// `GET /api/viewers/me/seen?artifact=<aid>`: `{seen}`, the highest version
/// this viewer has viewed unpinned; null for none or no cookie.
pub async fn seen(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    sees: clax_http::live_access::SeesLive,
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
    _d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    sees: clax_http::live_access::SeesLive,
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
    _d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    sees: clax_http::live_access::SeesLive,
    q: Result<Query<AttentionQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let one = q.artifact.as_deref().map(parse_id).transpose()?;
    if let Some(id) = &one {
        sees.check(&s.live_ids, id.as_str())?;
    }
    let hidden = (!sees.may_see).then(|| s.live_ids.clone());
    // Every artifact's attention costs what the viewer's threads do, so it
    // waits in the bulk lane; one artifact's is an interactive call.
    let whole = one.is_none();
    let job = move |st: &clax_store::Store| {
        Ok(match (who.viewer(st)?, one) {
            (None, _) => json!({}),
            (Some(v), None) => json!(st.attention_all(&v.id)?),
            (Some(v), Some(id)) => match st.attention_one(&v.id, &id)? {
                Some(a) => Value::Object([(id.to_string(), json!(a))].into_iter().collect()),
                None => json!({}),
            },
        })
    };
    let mut out = if whole {
        s.store_call_bulk(job).await?
    } else {
        s.store_call(job).await?
    };
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
    _d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    sees: clax_http::live_access::SeesLive,
    req: Result<Json<LookedBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    sees.check(&s.live_ids, id.as_str())?;
    let max = clax_store::attention::MAX_LOOKED;
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
    /// The tab has focus (kept for the viewer's own agents, never shown).
    #[serde(default)]
    focused: bool,
    /// How long before this report the viewer last gave input in the tab.
    #[serde(default)]
    idle_ms: Option<u64>,
}

/// `PUT /api/viewers/me/presence`: reports this viewer here or away on an
/// artifact, with where they look when they share it (spec §10, "Presence");
/// the artifact's presence. Announces a `presence` event when the
/// report changed what others see. A `here` report is input on the viewer's
/// live seat there ([`clax_http::seats`]). A spectator's report (a person beyond
/// the artifact's live seats, spec 2026-10-11-clax-dev-hosted-design §16.3)
/// is not listed (one listed before is dropped): it counts them as viewing
/// instead. `{people, viewing}`:
/// `viewing` counts the people seated and the spectators seen in the last
/// 15 s. 400 `no_viewer` without a viewer cookie;
/// 429 `limit_reached` when the artifact already lists
/// [`clax_core::presence::MAX_PEOPLE`] others and none of them is gone.
/// `focused` and `idle_ms` are kept for the viewer's own agents' suggestion
/// of where to ask (spec 2026-10-16-ask-where-attention-is-design §1) and
/// never shown to anyone; a spectator's report records neither.
pub async fn set_presence(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    sees: clax_http::live_access::SeesLive,
    req: Result<Json<PresenceBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    use clax_core::presence::State as P;
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    sees.check(&s.live_ids, id.as_str())?;
    let (v, claim) = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let v = who
                .viewer(st)?
                .ok_or_else(|| CoreError::invalid("no_viewer", "open /api/viewers/me first"))?;
            let claim = match who.caller(st, None) {
                Ok(c) => clax_http::seats::claim_for(st, &c, id.as_str())?,
                Err(_) => None,
            };
            Ok((v, claim))
        })
        .await?;
    let state = match b.state {
        ReportedState::Here => P::Here,
        ReportedState::Away => P::Away,
    };
    let aid = b.artifact_id.as_str();
    let presence_answer = |s: &AppState| {
        Json(json!({
            "people": s.presence.for_artifact(aid),
            "viewing": s.seats.counts(aid).viewing,
        }))
    };
    if let Some(c) = &claim {
        if !s.seats.may_hold(aid, c) {
            // A spectator is counted, never listed (spec §16.3): a listing
            // from before (unnamed, or seated) goes.
            s.seats.see_spectator(aid, &c.key);
            if s.presence.leave(aid, &v.public_id) {
                clax_realtime::presence::announce(&s.events, &s.presence, aid);
            }
            return Ok(presence_answer(&s));
        }
        // A page in use keeps its viewer's live seat (spec §16.1).
        if state == P::Here {
            s.seats.touch(aid, &c.key);
        }
    }
    let changed = s
        .presence
        .report_engaged(
            &b.artifact_id,
            &v.public_id,
            v.display_name.as_deref(),
            state,
            b.r#where.as_deref(),
            b.tab.as_deref().unwrap_or(""),
            b.focused,
            b.idle_ms,
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
        clax_realtime::presence::announce(&s.events, &s.presence, aid);
    }
    Ok(presence_answer(&s))
}
