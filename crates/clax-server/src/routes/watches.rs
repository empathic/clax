//! Watches (spec §6): `PUT`/`DELETE /api/sessions/<sid>/watches/<aid>` (W),
//! the listing, and scope watches on page URLs (`/live-watches`, spec
//! 2026-10-05 L2).

use super::artifacts::{body, path};
use super::live::{page_url, page_view};
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use clax_core::{ArtifactId, CoreError, Event};
use clax_http::agent::{DeniedToGrants, TokenOrAgent};
use clax_http::audit::DeferredAudit;
use clax_http::auth::RequireToken;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_realtime::feedback::apply;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WatchBody {
    replies_armed: Option<bool>,
    /// Another Clax's sync session: the local agent it relays for here
    /// (`null` clears it).
    #[serde(rename = "as", default, deserialize_with = "some_or_null")]
    relay: Option<Option<RelayAs>>,
}

/// A local agent another Clax relays for: its name and harness there.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayAs {
    name: String,
    harness: String,
}

/// `Some(None)` for an explicit `null`, so it differs from an absent field.
fn some_or_null<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<RelayAs>>, D::Error> {
    Option::<RelayAs>::deserialize(d).map(Some)
}

/// `invalid_args` unless `r` names an agent as agent names go (1 to 32 of
/// a-z, 0-9, `.`, `_` and `-`, a letter or digit at each end) of one of
/// the harnesses.
fn checked_relay(r: &RelayAs) -> Result<(), ApiError> {
    let n = r.name.as_bytes();
    let ok_name = (1..=32).contains(&n.len())
        && n.iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(b))
        && n[0].is_ascii_alphanumeric()
        && n[n.len() - 1].is_ascii_alphanumeric();
    if ok_name && super::sessions::HARNESSES.contains(&r.harness.as_str()) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "invalid_args",
            "as names an agent (1 to 32 of a-z, 0-9, '.', '_' and '-') and a harness",
        ))
    }
}

/// Watches the artifact (replies armed unless `replies_armed: false`) and
/// hands the session any feedback on it that had no live target, unless it
/// is a person's agent's: the owner's agents' untargeted feedback stays
/// where it is, as a person's agent's publish leaves it (a person's agent
/// receives a thread when it is sent to it, spec 2026-10-08 §10). A new
/// watch, or a change of its arming, records `watch.start` as the session's
/// agent (audit spec §6.4).
pub async fn put(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::WatchesPut>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    let (sid, _) = path(p)?;
    let id = a.artifact()?;
    let retarget = a.grant().is_none();
    let b: WatchBody = if raw.is_empty() {
        WatchBody::default()
    } else {
        serde_json::from_slice(&raw)
            .map_err(|e| ApiError::bad_request("invalid_json", e.to_string()))?
    };
    if let Some(Some(r)) = &b.relay {
        checked_relay(r)?;
    }
    let ctx = s.feedback_ctx();
    let watch = s
        .store_call(move |st| {
            let audit = audit.for_session(st, &sid)?;
            // Only another Clax's sync session (the by-hand harness of a
            // device sign-in's token) names whom it relays for.
            if b.relay.is_some() {
                let session = st.get_session(&sid)?;
                let grant = session
                    .as_ref()
                    .filter(|x| x.harness == super::sessions::BY_HAND_HARNESS)
                    .and_then(|x| x.grant_id.clone());
                let relays = match grant {
                    Some(g) => st.is_device_grant(&g)?,
                    None => false,
                };
                if !relays {
                    return Err(CoreError::invalid(
                        "invalid_args",
                        "only another Clax's sync session names an agent it relays for",
                    ));
                }
            }
            let w = st.watch(&audit, &sid, &id, b.replies_armed.unwrap_or(true))?;
            if let Some(relay) = &b.relay {
                let relay = relay
                    .as_ref()
                    .map(|r| (r.name.as_str(), r.harness.as_str()));
                st.set_watch_relay(&sid, &id, relay)?;
            }
            // The artifact's Send menu and roster show the watcher (and the
            // agent it relays for), and a linked artifact's sync names it.
            ctx.events.publish(Event::Agents {
                artifacts: vec![id.as_str().to_string()],
            });
            if retarget {
                let touched = st.retarget_untargeted(&id, &sid)?;
                apply(&ctx, st, &touched);
            }
            Ok(w)
        })
        .await?;
    Ok(Json(json!({"watch": watch})))
}

/// Stops watching the artifact; a watch that existed records `watch.stop`
/// as the session's agent.
pub async fn delete(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::WatchesDelete>,
    audit: DeferredAudit,
    p: Result<Path<(String, String)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let (sid, _) = path(p)?;
    let id = a.artifact()?;
    let aid = id.as_str().to_string();
    let removed = s
        .store_call(move |st| {
            let audit = audit.for_session(st, &sid)?;
            st.unwatch(&audit, &sid, &id)
        })
        .await?;
    if removed {
        s.events.publish(Event::Agents {
            artifacts: vec![aid],
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The session's watches. Token-gated like every `/api/sessions*` read. A
/// watch of a followed remote artifact names it as there: `artifact_id`
/// its ID there, with its `url` there and `remote: {origin, name}`, never
/// the local mirror.
pub async fn list(
    State(s): State<AppState>,
    _a: TokenOrAgent<clax_http::agent::WatchesList>,
    sid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let watches = s
        .store_call(move |st| {
            st.list_watches(&sid)?
                .into_iter()
                .map(|w| {
                    let mut v = json!(w);
                    if let Some(m) = st.mirror_of(&w.artifact_id)? {
                        v["artifact_id"] = json!(m.remote_artifact_id);
                        v["url"] = json!(m.url);
                        v["remote"] = json!({"origin": m.origin, "name": m.name});
                    }
                    Ok(v)
                })
                .collect::<clax_core::Result<Vec<Value>>>()
        })
        .await?;
    Ok(Json(json!({"watches": watches})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveWatchBody {
    url: String,
    replies_armed: Option<bool>,
}

/// The pages a scope watch on `origin` + `path` covers, as the tools show it.
fn scope_label(origin: &str, path: &str) -> String {
    if path.ends_with('/') {
        format!("{origin}{path}*")
    } else {
        format!("{origin}{path} and {origin}{path}/*")
    }
}

/// `PUT /api/sessions/<sid>/live-watches` (W): a scope watch on the page
/// `url` names (spec 2026-10-05 L2, §9.3). The page is created (with its
/// placeholder) when it does not exist; the session watches every live page
/// the scope covers and is handed their comments that were waiting with no
/// live target. The scope covers the same paths on every origin of the
/// URL's site, origins joined later included (spec §7.2). Answers
/// `{live_watch: {origin, path, scope, replies_armed}, page, site,
/// covered}`; `site` names the site and its origins. The page it makes, the
/// scope watch and the page watches it starts are recorded as the session's
/// agent (audit spec §6.3, §6.4).
pub async fn live_put(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    b: Result<Json<LiveWatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let b = body(b)?;
    let pu = page_url(&s, &b.url)?;
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let live_ids = s.live_ids.clone();
    let (w, page, artifact, covered, site) = s
        .store_call_async(move |st| async move {
            let st = &*st;
            // The page is made before the scope watch is written, so a
            // failure leaves no watch behind; an unknown session makes no page.
            if st.get_session(&sid)?.is_none_or(|x| x.ended_at.is_some()) {
                return Err(CoreError::invalid(
                    "unknown_session",
                    format!("no live session {sid}"),
                ));
            }
            let audit = audit.for_session(st, &sid)?;
            let page_url = pu.key.page_url();
            let title = page_url
                .split_once("://")
                .map_or(page_url.as_str(), |(_, rest)| rest);
            let e = live_ids
                .ensure_page(st, &audit, &pu.key, title, None, &[])
                .await?;
            if e.new_version {
                events.publish(Event::Version {
                    artifact_id: e.artifact.id.clone(),
                    n: e.version.n,
                    by_page: false,
                    title: Some(e.artifact.title.clone()),
                    at: Some(e.version.created_at.clone()),
                });
            }
            let (w, covered) =
                st.live_watch(&audit, &sid, &pu.key, b.replies_armed.unwrap_or(true))?;
            for aid in &covered {
                let touched = st.retarget_untargeted(&ArtifactId::parse(aid)?, &sid)?;
                apply(&ctx, st, &touched);
            }
            let id = ArtifactId::parse(&e.artifact.id)?;
            let page = st.live_page_of(&id)?.ok_or(CoreError::NotFound)?;
            let site = st.joined_site(&w.origin)?;
            Ok((w, page, e.artifact, covered, site))
        })
        .await?;
    Ok(Json(json!({
        "live_watch": {
            "origin": w.origin,
            "path": w.path,
            "scope": scope_label(&w.origin, &w.path),
            "replies_armed": w.replies_armed,
        },
        "page": page_view(&s, &page, &artifact, None),
        "site": super::sites::site_view(&site),
        "covered": covered,
    })))
}

#[derive(Deserialize)]
pub struct LiveUnwatchQuery {
    url: String,
}

/// `DELETE /api/sessions/<sid>/live-watches?url=` (W): removes the scope
/// watch on the page `url` names and the watches only it justified. The
/// live page itself stays. Answers `{page_url, removed: [artifact IDs]}`.
/// The watches it stops are recorded as the session's agent.
pub async fn live_delete(
    State(s): State<AppState>,
    _d: DeniedToGrants,
    _t: RequireToken,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    q: Result<Query<LiveUnwatchQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = page_url(&s, &q.url)?;
    let page = pu.key.page_url();
    let removed = s
        .store_call(move |st| {
            let audit = audit.for_session(st, &sid)?;
            st.live_unwatch(&audit, &sid, &pu.key)
        })
        .await?;
    Ok(Json(json!({"page_url": page, "removed": removed})))
}

/// What only the owner and the owner's agents do here.
const REMOTE_WHAT: &str = "watches an artifact on a connected Clax instance";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteWatchBody {
    /// The remote: its ID, name or origin.
    remote: String,
    /// The artifact's ID there.
    artifact_id: String,
    /// Its address there, as the resolver answered it. Taken and not
    /// used: the remote's own (`<origin>/a/<artifact_id>`) is the one kept.
    #[serde(default, rename = "url")]
    _url: Option<String>,
    #[serde(default)]
    replies_armed: Option<bool>,
}

/// 403 `forbidden` when an agent of the owner's names, in `X-Clax-Session`,
/// another session than the path's `sid`: an agent watches for its own
/// session, as the other routes under `/api/sessions/<sid>` hold it to.
fn own_session(asker: &super::remotes::Asker, sid: &str) -> Result<(), ApiError> {
    match asker {
        super::remotes::Asker::Agent(Some(own)) if own != sid => Err(ApiError::forbidden(
            "forbidden",
            "an agent watches for its own session (X-Clax-Session names another)",
        )),
        _ => Ok(()),
    }
}

/// A followed remote artifact as the remote-watch routes answer it.
fn remote_view(r: &clax_store::remotes::Remote, raid: &str) -> Value {
    json!({
        "artifact_id": raid,
        "url": format!("{}/a/{raid}", r.origin),
        "remote": {"origin": r.origin, "name": r.name},
    })
}

/// `PUT /api/sessions/<sid>/remote-watches {remote, artifact_id, url?,
/// replies_armed?}`: session `sid` watches artifact `artifact_id` of the
/// connected remote `remote` (spec 2026-10-29-remote-instances §4). The
/// artifact is followed (`Store::follow_remote`): its threads are kept in
/// step in a local mirror, which the session watches as it watches a local
/// artifact (replies armed unless `replies_armed: false`), so a comment
/// there sent to it reaches it by every route a local one takes; the
/// linked threads' sync opens a session there named as the session's agent
/// (`crate::linked`). Answers `{watch, mirror, artifact_id, url, remote:
/// {origin, name}}`, `url` the artifact's address there. The owner and the
/// owner's agents alone, on a local daemon (404 on a hosted one), an
/// agent for its own session alone (403 `forbidden` otherwise); errors
/// as `crate::routes::remote_artifacts`'s, the remote's own refusal of
/// the artifact passed on, and 400 `pushed_here` naming `artifact_id`
/// here for the public copy of an artifact this home pushed: its local
/// artifact is the one to watch. Records `watch.start` as the session's
/// agent.
pub async fn remote_put(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    b: Result<Json<RemoteWatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let b = body(b)?;
    super::remotes::local_only(&s)?;
    who.require_owner(REMOTE_WHAT)?;
    let r = super::remote_artifacts::reach(&s, &d, &b.remote).await?;
    own_session(&r.asker, &sid)?;
    let raid = super::remote_apps::remote_artifact(&b.artifact_id)?;
    let (rid, r2) = (r.remote.id.clone(), raid.clone());
    let pushed = s
        .store_call(move |st| {
            Ok(st
                .all_remote_links()?
                .into_iter()
                .find(|l| l.remote_id == rid && l.remote_artifact_id == r2 && !l.followed))
        })
        .await?;
    if let Some(l) = pushed {
        let mut e = ApiError::bad_request(
            "pushed_here",
            format!(
                "{} is the public copy of this Clax's artifact {}: watch {} instead",
                l.url, l.artifact_id, l.artifact_id
            ),
        );
        e.extra.insert("artifact_id".into(), json!(l.artifact_id));
        return Err(e);
    }
    // The artifact there, as the token's person's agents read it: that it
    // is there to follow, and its title.
    let c = crate::public::Client::new(&r.remote.origin, Some(r.token.clone()));
    let there = match clax_rt::sendable(c.get(&format!("/api/artifacts/{raid}"))).await {
        Ok(v) => v,
        Err(f) => return Err(super::remote_artifacts::failed(&s, &r.remote, f).await),
    };
    let title = there["artifact"]["title"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let url = format!("{}/a/{raid}", r.remote.origin);
    let armed = b.replies_armed.unwrap_or(true);
    let (rid, r2, ctx) = (r.remote.id.clone(), raid.clone(), s.feedback_ctx());
    let (mirror, watch) = s
        .store_call(move |st| {
            let audit = audit.for_session(st, &sid)?;
            let (m, w) = st.follow_and_watch(&audit, &sid, (&rid, &r2), &url, &title, armed)?;
            // The sync names the session's agent there.
            ctx.events.publish(Event::Agents {
                artifacts: vec![m.clone()],
            });
            Ok((m, w))
        })
        .await?;
    s.linked.links_changed();
    s.linked.kick(Some(&r.remote.id));
    let mut v = remote_view(&r.remote, &raid);
    v["watch"] = json!(watch);
    v["mirror"] = json!(mirror);
    Ok(Json(v))
}

/// `DELETE /api/sessions/<sid>/remote-watches {remote, artifact_id,
/// url?}`: session `sid` stops watching artifact `artifact_id` of remote
/// `remote`; its session there ends. Once no live session watches it, it
/// is no longer followed (`Store::unfollow_remote`), and its mirror is
/// dropped a week later. 204, whether or not it was watched; 404
/// `no_remote` for no such remote. The owner and the owner's agents alone,
/// on a local daemon, an agent for its own session alone. A watch that existed records `watch.stop` as the
/// session's agent.
pub async fn remote_delete(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
    b: Result<Json<RemoteWatchBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let sid = path(p)?;
    let b = body(b)?;
    super::remotes::local_only(&s)?;
    who.require_owner(REMOTE_WHAT)?;
    own_session(&super::remotes::asker(&s, &d).await?, &sid)?;
    let raid = super::remote_apps::remote_artifact(&b.artifact_id)?;
    let key = b.remote.trim().trim_end_matches('/').to_string();
    let remote = s
        .store_call(move |st| st.find_remote(&key))
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "no_remote",
                format!(
                    "no remote is named '{}': `clax remote list` names them",
                    b.remote
                ),
            )
        })?;
    let (rid, ctx) = (remote.id.clone(), s.feedback_ctx());
    s.store_call(move |st| {
        let Some(m) = st.remote_mirror(&rid, &raid)? else {
            return Ok(());
        };
        let audit = audit.for_session(st, &sid)?;
        st.unwatch(&audit, &sid, &ArtifactId::parse(&m)?)?;
        // No longer followed once no live session watches it.
        st.unfollow_remote(&m)?;
        ctx.events.publish(Event::Agents { artifacts: vec![m] });
        Ok(())
    })
    .await?;
    s.linked.links_changed();
    s.linked.kick(Some(&remote.id));
    Ok(StatusCode::NO_CONTENT)
}
