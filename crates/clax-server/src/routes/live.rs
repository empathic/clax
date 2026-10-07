//! `/api/live/*` (spec 2026-10-05-chrome-overlay-design §9.2): look up a
//! page URL's live page, and post a comment on a live page with its
//! screenshot and snapshot. Viewer routes: no token, and only from the
//! shell's origin or a script ([`SameOrigin`]).

use super::assets::multipart_error;
use super::threads::{announce_new_thread, mentions_agent, publish_thread};
use crate::auth::has_token;
use crate::error::ApiError;
use crate::feedback::{thread_view, thread_views};
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::{SameOrigin, author};
use axum::Json;
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use clax_core::live::{PageKey, PageUrl, PathPattern, parse_page_url};
use clax_core::model::Artifact;
use clax_core::store::live::LivePage;
use clax_core::store::site::{
    KIND_MERGE, KIND_MOVE, KIND_UNMERGE, LiveRule, MAX_REFILE, MoveBy, Refile, Refiled,
};
use clax_core::store::threads::{NewThread, clip_problem};
use clax_core::{Anchor, ArtifactId, CoreError, Event, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

/// The request cap of `POST /api/live/threads`: an 8 MiB snapshot, a 5 MiB
/// clip, and the rest.
pub const LIVE_THREAD_LIMIT: usize = 24 * 1024 * 1024;
/// The largest snapshot accepted, in bytes.
pub const MAX_SNAPSHOT: usize = 8 * 1024 * 1024;
/// The longest page title kept, in characters.
pub const MAX_TITLE: usize = 200;

/// Host names that always name this machine.
const LOCAL_NAMES: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// `raw` split into its live-page key and route (spec §7), refusing the
/// daemon's own pages with `own_origin`: a URL on the daemon's port whose
/// host is a local name (`localhost`, any `*.localhost`), a loopback or
/// unspecified IP address (`127.0.0.0/8`, `::1`, `0.0.0.0`, `::`), or the
/// host the daemon is reached at.
///
/// # Errors
/// `parse_page_url`'s, and `own_origin`.
pub(crate) fn page_url(s: &AppState, raw: &str) -> Result<PageUrl, ApiError> {
    let p = parse_page_url(raw)?;
    let origin = url::Url::parse(&p.key.origin)
        .map_err(|e| ApiError::bad_request("invalid_url", e.to_string()))?;
    let host = origin.host_str().unwrap_or("");
    let own: Vec<url::Url> = [&s.self_base, &s.browser_base]
        .into_iter()
        .filter_map(|b| url::Url::parse(b).ok())
        .collect();
    let ip = match origin.host() {
        Some(url::Host::Ipv4(a)) => Some(std::net::IpAddr::V4(a)),
        Some(url::Host::Ipv6(a)) => Some(std::net::IpAddr::V6(a)),
        _ => None,
    };
    let ours = LOCAL_NAMES.contains(&host)
        || host.ends_with(".localhost")
        || ip.is_some_and(|ip| {
            let ip = ip.to_canonical();
            ip.is_loopback() || ip.is_unspecified()
        })
        || own.iter().any(|u| u.host_str() == Some(host));
    let on_our_port = own
        .iter()
        .any(|u| u.port_or_known_default() == origin.port_or_known_default());
    if ours && on_our_port {
        return Err(ApiError::bad_request(
            "own_origin",
            "Clax's own pages have their own comment mode",
        ));
    }
    Ok(p)
}

/// A title as a page gave it, fit to store (spec §8.3): whitespace
/// collapsed, other control characters dropped, cut to [`MAX_TITLE`]
/// characters; `fallback` when nothing is left.
pub(crate) fn clean_title(t: &str, fallback: &str) -> String {
    let spaced: String = t
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some(' ')
            } else if c.is_control() {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let t: String = clax_core::anchor::collapse(&spaced)
        .chars()
        .take(MAX_TITLE)
        .collect();
    let t = t.trim_end().to_string();
    if t.is_empty() {
        fallback.to_string()
    } else {
        t
    }
}

/// The view of a live page the routes answer with: its key, `page_url`,
/// the artifact's title and current version, `url`, its Clax view, and
/// `merged` and `pattern`: whether it is the canonical page of the merge
/// rule `rule` (when `rule`'s pattern is its path), and that pattern.
pub(crate) fn page_view(
    s: &AppState,
    p: &LivePage,
    a: &Artifact,
    rule: Option<&LiveRule>,
) -> Value {
    page_json(s, p, &a.title, a.current_version, rule)
}

/// [`page_view`] from the page's title and current version.
fn page_json(
    s: &AppState,
    p: &LivePage,
    title: &str,
    current_version: u32,
    rule: Option<&LiveRule>,
) -> Value {
    let pattern = rule.map(|r| &r.pattern).filter(|pat| **pat == p.path);
    json!({
        "artifact_id": p.artifact_id,
        "origin": p.origin,
        "path": p.path,
        "page_url": format!("{}{}", p.origin, p.path),
        "title": title,
        "current_version": current_version,
        "url": format!("{}/a/{}", s.browser_base.trim_end_matches('/'), p.artifact_id),
        "merged": pattern.is_some(),
        "pattern": pattern,
    })
}

/// A merge rule as the routes answer with it: its fields and `page_url`,
/// the URL of its canonical page (the origin followed by the pattern).
fn rule_view(r: &LiveRule) -> Value {
    json!({
        "id": r.id,
        "origin": r.origin,
        "pattern": r.pattern,
        "page_url": format!("{}{}", r.origin, r.pattern),
        "created_at": r.created_at,
    })
}

#[derive(Deserialize)]
pub struct PageQuery {
    url: String,
}

/// `GET /api/live/pages?url=`: `{page, route, rule}`, the live page the
/// URL names ([`page_view`], or null; the canonical page when a merge rule
/// maps the URL's path), the URL's route (or null), and that rule
/// ([`rule_view`], or null). Never creates a page.
pub async fn page(
    State(s): State<AppState>,
    _o: SameOrigin,
    q: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = page_url(&s, &q.url)?;
    let key = pu.key.clone();
    let live_ids = s.live_ids.clone();
    let (found, rule) = s
        .store_call(move |st| {
            // The extension looks a tab's URL up: its origin is in use.
            touch(&live_ids, st, &key.origin);
            let r = st.resolve_live_key(&key)?;
            let Some(p) = st.find_live_page(&r.key)? else {
                return Ok((None, r.rule));
            };
            let Some(a) = st.get_artifact(&ArtifactId::parse(&p.artifact_id)?)? else {
                return Ok((None, r.rule));
            };
            Ok((Some((p, a)), r.rule))
        })
        .await?;
    Ok(Json(json!({
        "page": found.map(|(p, a)| page_view(&s, &p, &a, rule.as_ref())),
        "route": pu.route,
        "rule": rule.as_ref().map(rule_view),
    })))
}

/// Records that Clax used `origin` (spec §7.2: a joined site is named after
/// its most recently used origin); a failure to record it is only logged.
fn touch(ids: &crate::live::LiveIds, st: &Store, origin: &str) {
    if let Err(e) = ids.touch(st, origin) {
        tracing::warn!(error = %e, origin, "could not record a joined origin's use");
    }
}

/// The fields of a `POST /api/live/threads`.
#[derive(Default)]
struct Fields {
    url: Option<String>,
    title: Option<String>,
    anchor: Option<String>,
    body: Option<String>,
    clip: Option<Vec<u8>>,
    snapshot: Option<Vec<u8>>,
    pending: Option<Vec<String>>,
    pick_id: Option<String>,
}

/// Whether `s` is a pick ID: 32 lowercase hex digits.
fn is_pick_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `pending` as a multipart field gives it: a JSON array of thread IDs.
fn parse_pending(text: &str) -> Result<Vec<String>, ApiError> {
    serde_json::from_str(text)
        .map_err(|_| ApiError::bad_request("invalid_args", "pending is a JSON array of thread IDs"))
}

/// Reads a `POST /api/live/threads`; any other field than its own, or one
/// given twice, is 400 `invalid_args`.
async fn read_fields(mut mp: Multipart) -> Result<Fields, ApiError> {
    let mut f = Fields::default();
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        let name = field.name().unwrap_or("").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e.status(), e.body_text()))?;
        let text = || {
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                ApiError::bad_request("invalid_args", format!("field '{name}' is not UTF-8"))
            })
        };
        let twice = match name.as_str() {
            "url" => f.url.replace(text()?).is_some(),
            "title" => f.title.replace(text()?).is_some(),
            "anchor" => f.anchor.replace(text()?).is_some(),
            "body" => f.body.replace(text()?).is_some(),
            "pending" => f.pending.replace(parse_pending(&text()?)?).is_some(),
            "pick_id" => {
                let pick = text()?;
                if !is_pick_id(&pick) {
                    return Err(ApiError::bad_request(
                        "invalid_args",
                        "pick_id is 32 lowercase hex digits",
                    ));
                }
                f.pick_id.replace(pick).is_some()
            }
            "clip" => f.clip.replace(bytes.to_vec()).is_some(),
            "snapshot" => {
                if bytes.len() > MAX_SNAPSHOT {
                    return Err(ApiError::bad_request(
                        "snapshot_too_large",
                        "a snapshot is at most 8 MiB",
                    ));
                }
                f.snapshot.replace(bytes.to_vec()).is_some()
            }
            _ => {
                return Err(ApiError::bad_request(
                    "invalid_args",
                    format!(
                        "unknown field '{name}': a comment takes url, title, anchor, body, pending, pick_id, clip and snapshot"
                    ),
                ));
            }
        };
        if twice {
            return Err(ApiError::bad_request(
                "invalid_args",
                format!("field '{name}' is given twice"),
            ));
        }
    }
    Ok(f)
}

/// What `POST /api/live/threads` answers with.
struct Answer {
    /// The request made the thread (`false`: a repeat found it).
    made: bool,
    view: Value,
    page: LivePage,
    artifact: Artifact,
    version: u32,
    /// The merge rule whose canonical page it is.
    rule: Option<LiveRule>,
}

/// A repeat of a comment: the thread pick `pick` made on the live page
/// `key`, when it did ([`Store::picked_thread`]).
fn replay(
    st: &Store,
    ctx: &crate::feedback::FeedbackCtx,
    key: &PageKey,
    pick: &str,
    with_path: bool,
    rule: Option<&LiveRule>,
) -> clax_core::Result<Option<Answer>> {
    let Some((aid, tid)) = st.picked_thread(key, pick)? else {
        return Ok(None);
    };
    let id = ArtifactId::parse(&aid)?;
    let (Some(thread), Some(page), Some(artifact)) = (
        st.get_thread(&tid)?,
        st.live_page_of(&id)?,
        st.get_artifact(&id)?,
    ) else {
        return Ok(None);
    };
    let version = thread.version_n;
    let view = thread_view(st, &thread, ctx.codex_push(), with_path)?;
    Ok(Some(Answer {
        made: false,
        view,
        page,
        artifact,
        version,
        rule: rule.cloned(),
    }))
}

/// `POST /api/live/threads` (multipart `url`, `title`, `anchor`, `body`,
/// `pending`, optional `clip`, `snapshot`): finds or creates the live page
/// `url` names (a merge rule's canonical page when one maps its path; the
/// thread then keeps that path as its `live_path`), stores `snapshot` as
/// its next version when it differs from
/// the current one (linking to that version, in its transaction, the
/// threads of `pending` still pending on the page; other addresses stay
/// pending), and creates the thread on that version as the request's
/// viewer, with the anchor's `route` from `url` and `clip` as its
/// screenshot; an `@agent` mention sends it. Answers `201 {thread, page,
/// version, clip_error?}`; a clip failing `clip_problem` is dropped and
/// reported.
///
/// With `pick_id` (32 lowercase hex digits), a repeat of the request within
/// [`PICK_TTL`](clax_core::store::live::PICK_TTL) whose pick already made a thread on the page writes and
/// sends nothing and answers `200 {thread, page, version}` with that thread
/// as it is now (`version` is the one it was made on).
pub async fn thread(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    who: Identity,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let f = read_fields(mp).await?;
    let pu = page_url(
        &s,
        f.url
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("invalid_args", "field 'url' is required"))?,
    )?;
    let mut anchor: Anchor =
        serde_json::from_str(f.anchor.as_deref().ok_or_else(|| {
            ApiError::bad_request("invalid_anchor", "field 'anchor' is required")
        })?)
        .map_err(|e| ApiError::bad_request("invalid_anchor", e.to_string()))?;
    anchor.route = pu.route.clone();
    let snapshot = f
        .snapshot
        .ok_or_else(|| ApiError::bad_request("invalid_args", "field 'snapshot' is required"))?;
    let pending = f
        .pending
        .ok_or_else(|| ApiError::bad_request("invalid_args", "field 'pending' is required"))?;
    let title = clean_title(f.title.as_deref().unwrap_or(""), &pu.key.page_url());
    let clip_error = f.clip.as_deref().and_then(clip_problem);
    let clip = if clip_error.is_some() { None } else { f.clip };
    let body = f.body.unwrap_or_default();
    let mention = mentions_agent(&body);
    let pick = f.pick_id;
    let with_path = has_token(&headers, &s.token);
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let live_ids = s.live_ids.clone();
    let key = pu.key;
    let a = s
        .store_call(move |st| {
            touch(&live_ids, st, &key.origin);
            let path = key.path.clone();
            let resolved = st.resolve_live_key(&key)?;
            let rule = resolved.rule.as_ref();
            let key = resolved.key.clone();
            if let Some(pick) = &pick
                && let Some(r) = replay(st, &ctx, &key, pick, with_path, rule)?
            {
                return Ok(r);
            }
            // A snapshot of this path settles only addresses of threads
            // made at it (a merged page holds others).
            let pending = st.pending_at_path(&pending, &key.origin, &path)?;
            let e = live_ids.ensure_page(st, &key, &title, Some(&snapshot), &pending)?;
            let id = ArtifactId::parse(&e.artifact.id)?;
            if e.new_version {
                events.publish(Event::Version {
                    artifact_id: e.artifact.id.clone(),
                    n: e.version.n,
                    by_page: false,
                    title: Some(e.artifact.title.clone()),
                    at: Some(e.version.created_at.clone()),
                });
            }
            for tid in &e.linked {
                if let Some(t) = st.get_thread(tid)? {
                    publish_thread(&ctx, st, &t)?;
                }
            }
            let (author_name, author_public_id) = author(st, &who)?;
            let t = NewThread {
                author_public_id,
                version_n: e.version.n,
                anchor,
                author_name,
                body,
                clip,
                via_page: false,
            };
            let made =
                st.create_live_thread(&id, t, pick.as_deref(), resolved.live_path.as_deref())?;
            let thread = match (made, &pick) {
                (Some(thread), _) => thread,
                (None, Some(pick)) => {
                    return replay(st, &ctx, &key, pick, with_path, rule)?
                        .ok_or(CoreError::NotFound);
                }
                (None, None) => return Err(CoreError::NotFound),
            };
            let view = announce_new_thread(st, &ctx, thread, mention, with_path)?;
            let page = st.live_page_of(&id)?.ok_or(CoreError::NotFound)?;
            Ok(Answer {
                made: true,
                view,
                page,
                artifact: e.artifact,
                version: e.version.n,
                rule: resolved.rule.clone(),
            })
        })
        .await?;
    let mut out = json!({
        "thread": a.view,
        "page": page_view(&s, &a.page, &a.artifact, a.rule.as_ref()),
        "version": a.version,
    });
    if !a.made {
        return Ok((StatusCode::OK, Json(out)));
    }
    if let Some(e) = clip_error {
        out["clip_error"] = json!(e);
    }
    Ok((StatusCode::CREATED, Json(out)))
}

/// The fields of a `POST /api/live/snapshots`.
struct SnapshotFields {
    url: String,
    title: Option<String>,
    pending: Vec<String>,
    snapshot: Vec<u8>,
}

/// Reads a `POST /api/live/snapshots`: `url`, `pending` (a JSON array of
/// thread IDs) and `snapshot` are required, `title` is optional; any other
/// field, or one given twice, is 400 `invalid_args`.
async fn read_snapshot_fields(mut mp: Multipart) -> Result<SnapshotFields, ApiError> {
    let (mut url, mut title, mut pending, mut snapshot) = (None, None, None, None);
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| multipart_error(e.status(), e.body_text()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if !matches!(name.as_str(), "url" | "title" | "pending" | "snapshot") {
            return Err(ApiError::bad_request(
                "invalid_args",
                format!(
                    "unknown field '{name}': a snapshot takes url, title, pending and snapshot"
                ),
            ));
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e.status(), e.body_text()))?;
        if name == "snapshot" && bytes.len() > MAX_SNAPSHOT {
            return Err(ApiError::bad_request(
                "snapshot_too_large",
                "a snapshot is at most 8 MiB",
            ));
        }
        let text = || {
            String::from_utf8(bytes.to_vec()).map_err(|_| {
                ApiError::bad_request("invalid_args", format!("field '{name}' is not UTF-8"))
            })
        };
        let twice = match name.as_str() {
            "url" => url.replace(text()?).is_some(),
            "title" => title.replace(text()?).is_some(),
            "pending" => pending.replace(parse_pending(&text()?)?).is_some(),
            _ => snapshot.replace(bytes.to_vec()).is_some(),
        };
        if twice {
            return Err(ApiError::bad_request(
                "invalid_args",
                format!("field '{name}' is given twice"),
            ));
        }
    }
    let required =
        |f: &str| ApiError::bad_request("invalid_args", format!("field '{f}' is required"));
    Ok(SnapshotFields {
        url: url.ok_or_else(|| required("url"))?,
        title,
        pending: pending.ok_or_else(|| required("pending"))?,
        snapshot: snapshot.ok_or_else(|| required("snapshot"))?,
    })
}

/// `POST /api/live/snapshots` (multipart `url`, `title`, `pending`,
/// `snapshot`): the extension's snapshot of a page with pending addresses
/// (spec L11). `pending` names the threads the extension saw pending when it
/// serialized the page; of those, the ones still pending on the page are
/// linked to the snapshot, stored as a new version even when identical to
/// the current one, and any other pending address waits for a later
/// snapshot. The check, the version and the links are one transaction.
/// Answers `{page, version, linked}` (the linked thread IDs, oldest address
/// first); 409 `nothing_pending`, writing nothing, when none of `pending` is
/// pending or the page does not exist; this route never creates a page.
pub async fn snapshot(
    State(s): State<AppState>,
    _o: SameOrigin,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<Json<Value>, ApiError> {
    let mp = mp.map_err(|e| multipart_error(e.status(), e.body_text()))?;
    let f = read_snapshot_fields(mp).await?;
    let pu = page_url(&s, &f.url)?;
    let title = clean_title(f.title.as_deref().unwrap_or(""), &pu.key.page_url());
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let key = pu.key;
    let done = s
        .store_call(move |st| {
            let path = key.path.clone();
            let resolved = st.resolve_live_key(&key)?;
            let Some(p) = st.find_live_page(&resolved.key)? else {
                return Ok(None);
            };
            let id = ArtifactId::parse(&p.artifact_id)?;
            // Only addresses of threads made at this path (see `thread`).
            let pending = st.pending_at_path(&f.pending, &p.origin, &path)?;
            let Some((v, linked)) = st.snapshot_pending(&id, &title, &f.snapshot, &pending)? else {
                return Ok(None);
            };
            let a = st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            events.publish(Event::Version {
                artifact_id: a.id.clone(),
                n: v.n,
                by_page: false,
                title: Some(a.title.clone()),
                at: Some(v.created_at.clone()),
            });
            for tid in &linked {
                if let Some(t) = st.get_thread(tid)? {
                    publish_thread(&ctx, st, &t)?;
                }
            }
            Ok(Some((p, a, v.n, linked, resolved.rule)))
        })
        .await?;
    let Some((page, artifact, n, linked, rule)) = done else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "nothing_pending",
            "none of the named threads has an address waiting for a snapshot",
        ));
    };
    Ok(Json(json!({
        "page": page_view(&s, &page, &artifact, rule.as_ref()),
        "version": n,
        "linked": linked,
    })))
}

/// 403 `forbidden` unless the request holds the token or comes through the
/// extension gateway: who may move threads and change merge rules.
pub(crate) fn owner_writes(who: &Identity) -> Result<(), ApiError> {
    if who.token || who.extension {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "forbidden",
            "only the owner (the token or the Clax extension) moves threads and changes rules",
        ))
    }
}

/// 403 `forbidden` unless the request holds an owner credential.
pub(crate) fn owner_reads(who: &Identity) -> Result<(), ApiError> {
    if who.is_owner() {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "forbidden",
            "only the owner lists a site's threads and rules",
        ))
    }
}

/// The origin a `site` or `rules` request names: the origin of `raw`, a
/// page URL or an origin, refusing the daemon's own ([`page_url`]).
pub(crate) fn origin_of(s: &AppState, raw: &str) -> Result<String, ApiError> {
    Ok(page_url(s, raw)?.key.origin)
}

#[derive(Deserialize)]
pub struct SiteQuery {
    origin: String,
}

/// A thread view's last activity: its newest comment, creation or resolve.
fn last_activity(t: &Value) -> String {
    let mut at = [&t["created_at"], &t["resolved_at"]]
        .into_iter()
        .filter_map(Value::as_str)
        .max()
        .unwrap_or("")
        .to_string();
    for c in t["comments"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if let Some(c) = c["created_at"].as_str()
            && c > at.as_str()
        {
            at = c.to_string();
        }
    }
    at
}

/// The status a site summary counts a thread view under: `resolved`,
/// `addressed` (open, and an agent addressed it: a version or a pending
/// address), or `open`.
fn summary_status(t: &Value) -> &'static str {
    if t["status"] == "resolved" {
        "resolved"
    } else if t["addressed_in"].as_array().is_some_and(|a| !a.is_empty())
        || !t["addressed_pending"].is_null()
    {
        "addressed"
    } else {
        "open"
    }
}

/// `GET /api/live/site?origin=<origin or page URL>` (the owner): every live
/// page of the origin's site (spec §7.2: of every origin joined to it) that
/// has threads, newest activity first, as `{origin, site, rules, pages:
/// [{page, summary: {open, addressed, resolved, last_activity}, threads}]}`.
/// `site` is the origin's site ([`super::sites::site_view`]) with `joining`:
/// how many threads a join not finished has still to re-file; their pages
/// are listed too, their page views marked `pending: true`. `page` is [`page_view`]; `threads` are
/// thread views (resolved ones too), newest activity first; `rules` the
/// origin's merge rules ([`rule_view`]), oldest first.
pub async fn site(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    who: Identity,
    q: Result<Query<SiteQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_reads(&who)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let origin = origin_of(&s, &q.origin)?;
    let with_path = has_token(&headers, &s.token);
    let codex = s.feedback_ctx().codex_push();
    let o = origin.clone();
    let (pages, rules, joined) = s
        .store_call(move |st| {
            let joined = st.joined_site(&o)?;
            let pages = st.site_pages(&o)?;
            let all: Vec<_> = pages.iter().flat_map(|p| p.threads.clone()).collect();
            let mut views = thread_views(st, &all, codex, with_path)?.into_iter();
            let mut out = Vec::new();
            for p in pages {
                let threads: Vec<Value> = views.by_ref().take(p.threads.len()).collect();
                out.push((p.page, p.title, p.current_version, threads, p.pending));
            }
            Ok((out, st.live_rules(&o)?, joined))
        })
        .await?;
    let mut groups: Vec<(String, Value)> = pages
        .into_iter()
        .filter(|(_, _, _, threads, _)| !threads.is_empty())
        .map(|(page, title, current, mut threads, pending)| {
            threads.sort_by_cached_key(|t| std::cmp::Reverse(last_activity(t)));
            let count = |st: &str| threads.iter().filter(|t| summary_status(t) == st).count();
            let last = threads.first().map(last_activity).unwrap_or_default();
            let rule = rules.iter().find(|r| r.pattern == page.path);
            let mut view = page_json(&s, &page, &title, current, rule);
            if pending {
                view["pending"] = json!(true);
            }
            let group = json!({
                "page": view,
                "summary": {
                    "open": count("open"),
                    "addressed": count("addressed"),
                    "resolved": count("resolved"),
                    "last_activity": last,
                },
                "threads": threads,
            });
            (last, group)
        })
        .collect();
    groups.sort_by(|a, b| b.0.cmp(&a.0));
    let mut site = super::sites::site_view(&joined);
    site["joining"] = json!(pages_pending_threads(&groups));
    Ok(Json(json!({
        "origin": origin,
        "site": site,
        "rules": rules.iter().map(rule_view).collect::<Vec<_>>(),
        "pages": groups.into_iter().map(|(_, g)| g).collect::<Vec<_>>(),
    })))
}

/// How many threads of the listing's pending pages (spec §7.2: a join not
/// finished) are still to be re-filed.
fn pages_pending_threads(groups: &[(String, Value)]) -> usize {
    groups
        .iter()
        .filter(|(_, g)| g["page"]["pending"] == true)
        .map(|(_, g)| g["threads"].as_array().map_or(0, Vec::len))
        .sum()
}

/// What a re-filing route needs besides the store.
#[derive(Clone)]
pub(crate) struct RefileCtx {
    pub(crate) feedback: crate::feedback::FeedbackCtx,
    pub(crate) live_ids: std::sync::Arc<crate::live::LiveIds>,
    cache: std::sync::Arc<crate::wrap_cache::WrapCache>,
}

impl RefileCtx {
    pub(crate) fn of(s: &AppState) -> RefileCtx {
        RefileCtx {
            feedback: s.feedback_ctx(),
            live_ids: s.live_ids.clone(),
            cache: s.wrap_cache.clone(),
        }
    }

    /// [`Store::refile_threads`], announcing what it did; when it fails, the
    /// pages of `fresh` (made for it) that hold no thread are deleted again.
    pub(crate) fn refile(
        &self,
        st: &Store,
        moves: &[(String, Refile)],
        how: &MoveBy,
        fresh: &[String],
    ) -> clax_core::Result<Refiled> {
        match st.refile_threads(moves, how, fresh) {
            Ok(done) => {
                announce_refiled(st, &self.feedback, &done)?;
                Ok(done)
            }
            Err(e) => {
                for aid in fresh {
                    if let Err(e) = self.discard_if_empty(st, aid) {
                        tracing::warn!(error = %e, artifact_id = aid.as_str(), "could not delete an empty live page");
                    }
                }
                Err(e)
            }
        }
    }

    pub(crate) fn discard_if_empty(&self, st: &Store, aid: &str) -> clax_core::Result<()> {
        let id = ArtifactId::parse(aid)?;
        if !st.list_threads(&id, true, None, 1)?.0.is_empty() {
            return Ok(());
        }
        st.delete_artifact(&id)?;
        self.cache.remove_artifact(aid);
        self.feedback.events.publish(Event::ArtifactDeleted {
            artifact_id: aid.to_string(),
        });
        self.live_ids.remove(aid);
        Ok(())
    }
}

/// Publishes what a re-filing did: each version written, then for each
/// thread that changed page `thread_moved` on the page it left (and, for
/// clients that know only it, `thread_deleted` on that page's own topics),
/// and each re-filed thread's view on its page.
fn announce_refiled(
    st: &Store,
    ctx: &crate::feedback::FeedbackCtx,
    done: &Refiled,
) -> clax_core::Result<()> {
    let mut titles: HashMap<String, String> = HashMap::new();
    for v in &done.versions {
        if !titles.contains_key(&v.artifact_id) {
            let a = st
                .get_artifact(&ArtifactId::parse(&v.artifact_id)?)?
                .ok_or(CoreError::NotFound)?;
            titles.insert(v.artifact_id.clone(), a.title);
        }
        ctx.events.publish(Event::Version {
            artifact_id: v.artifact_id.clone(),
            n: v.n,
            by_page: false,
            title: titles.get(&v.artifact_id).cloned(),
            at: Some(v.created_at.clone()),
        });
    }
    for m in &done.moved {
        if m.from != m.to {
            ctx.events.publish(Event::ThreadMoved {
                artifact_id: m.from.clone(),
                thread_id: m.thread_id.clone(),
                to_artifact_id: m.to.clone(),
            });
            ctx.events.publish(Event::ThreadDeleted {
                artifact_id: m.from.clone(),
                thread_id: m.thread_id.clone(),
                moved: true,
            });
        }
        if let Some(t) = st.get_thread(&m.thread_id)? {
            publish_thread(ctx, st, &t)?;
        }
    }
    Ok(())
}

/// Finds or makes the live page `key` (titled `title` when made) and
/// announces a version it wrote; `fresh` when this call made it.
pub(crate) fn target_page(
    st: &Store,
    live_ids: &crate::live::LiveIds,
    events: &clax_core::EventBus,
    key: &PageKey,
    title: &str,
) -> clax_core::Result<(String, bool)> {
    let e = live_ids.ensure_page(st, key, title, None, &[])?;
    if e.new_version {
        events.publish(Event::Version {
            artifact_id: e.artifact.id.clone(),
            n: e.version.n,
            by_page: false,
            title: Some(e.artifact.title.clone()),
            at: Some(e.version.created_at.clone()),
        });
    }
    Ok((e.artifact.id, e.created))
}

/// `viewer:<public ID>` of the owner, who moves threads.
pub(crate) fn mover(st: &Store, who: &Identity) -> clax_core::Result<String> {
    let v = who.ensure_viewer(st)?.ok_or(CoreError::NotFound)?;
    Ok(format!("viewer:{}", v.public_id))
}

/// The live page `id` with its artifact, for an answer.
pub(crate) fn page_of(st: &Store, id: &str) -> clax_core::Result<(LivePage, Artifact)> {
    let id = ArtifactId::parse(id)?;
    Ok((
        st.live_page_of(&id)?.ok_or(CoreError::NotFound)?,
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveBody {
    page_url: String,
}

/// `POST /api/live/threads/<tid>/move` `{page_url}` (the owner: the token
/// or the extension): re-files the live page's thread `tid` under the live
/// page `page_url` names (its merge rule's canonical page when one maps it;
/// made when missing), at `page_url`'s route, keeping its comments, history,
/// clip, snapshot links and agents (spec 2026-10-05-chrome-overlay-design
/// §7.1). Answers `{thread, page, moved}`; `moved` is false, and nothing is
/// written, when the thread is already there. 400 `cross_origin` for a page
/// of another site (an origin not joined to the thread's: nothing written); 404 for a missing thread or one not
/// on a live page.
pub async fn move_thread(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    p: Result<axum::extract::Path<String>, axum::extract::rejection::PathRejection>,
    body: Result<Json<MoveBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_writes(&who)?;
    let tid = super::artifacts::path(p)?;
    let b = super::artifacts::body(body)?;
    let pu = page_url(&s, &b.page_url)?;
    let rc = RefileCtx::of(&s);
    let with_path = who.token;
    let (view, page, artifact, rule, moved) = s
        .store_call(move |st| {
            let t = st.get_thread(&tid)?.ok_or(CoreError::NotFound)?;
            let Some(origin) = rc.live_ids.origin_of(&t.artifact_id) else {
                return Err(CoreError::NotFound);
            };
            // Within its site, joined origins included (spec §7.2).
            if rc.live_ids.site_key(&origin) != rc.live_ids.site_key(&pu.key.origin) {
                return Err(CoreError::invalid(
                    "cross_origin",
                    "a thread moves only to a page of its own site",
                ));
            }
            let r = st.resolve_live_key(&pu.key)?;
            let title = clean_title("", &pu.key.page_url());
            let (to, made) = target_page(st, &rc.live_ids, &rc.feedback.events, &r.key, &title)?;
            let refile = Refile {
                thread_id: tid.clone(),
                live_path: r.live_path,
                route: pu.route,
            };
            let how = MoveBy {
                by: mover(st, &who)?,
                kind: KIND_MOVE,
                rule_id: None,
            };
            let fresh: Vec<String> = made.then(|| to.clone()).into_iter().collect();
            let done = rc.refile(st, &[(to.clone(), refile)], &how, &fresh)?;
            let t = st.get_thread(&tid)?.ok_or(CoreError::NotFound)?;
            let (page, artifact) = page_of(st, &to)?;
            let view = thread_view(st, &t, rc.feedback.codex_push(), with_path)?;
            Ok((view, page, artifact, r.rule, !done.moved.is_empty()))
        })
        .await?;
    Ok(Json(json!({
        "thread": view,
        "page": page_view(&s, &page, &artifact, rule.as_ref()),
        "moved": moved,
    })))
}

/// `GET /api/live/rules?origin=<origin or page URL>` (the owner):
/// `{origin, rules}`, the origin's merge rules in force ([`rule_view`]),
/// oldest first.
pub async fn rules(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    q: Result<Query<SiteQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_reads(&who)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let origin = origin_of(&s, &q.origin)?;
    let o = origin.clone();
    let rules = s.store_call(move |st| st.live_rules(&o)).await?;
    Ok(Json(json!({
        "origin": origin,
        "rules": rules.iter().map(rule_view).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleBody {
    origin: String,
    pattern: String,
}

/// `POST /api/live/rules` `{origin, pattern}` (the owner: the token or the
/// extension): adds a merge rule (spec 2026-10-05-chrome-overlay-design
/// §7.1) and applies it: the live pages of `origin` whose path `pattern`
/// matches are one page from now on, the canonical page whose path is
/// `pattern`, and the threads of the origin whose path the rule now wins
/// are re-filed there (made when missing), as a move would, at most
/// [`MAX_REFILE`] a request, all of them in one transaction. Answers `201
/// {rule, page, moved, remaining}` (`page`: the canonical page, or null
/// when it does not exist; `moved`: the re-filed thread IDs; `remaining`:
/// threads left for the next request); `200` for a rule the origin already
/// has, applied again (to the remaining threads). 400 `invalid_pattern`,
/// `own_origin`, `too_many_rules`.
pub async fn add_rule(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    body: Result<Json<RuleBody>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    owner_writes(&who)?;
    let b = super::artifacts::body(body)?;
    let pattern = PathPattern::parse(&b.pattern)?;
    let origin = origin_of(&s, &b.origin)?;
    let canonical = page_url(&s, &format!("{origin}{}", pattern.as_str()))?;
    if canonical.key.path != pattern.as_str() || canonical.route.is_some() {
        return Err(ApiError::bad_request(
            "invalid_pattern",
            "the pattern is not a path as the URL parser writes it",
        ));
    }
    let rc = RefileCtx::of(&s);
    let (rule, created, page, moved, remaining) = s
        .store_call(move |st| {
            let (rule, created) = st.add_live_rule(&origin, &pattern)?;
            let (refiles, remaining) = st.merge_candidates(&rule, MAX_REFILE)?;
            if refiles.is_empty() {
                let page = match st.find_live_page(&canonical.key)? {
                    Some(p) => Some(page_of(st, &p.artifact_id)?),
                    None => None,
                };
                return Ok((rule, created, page, Vec::new(), remaining));
            }
            let title = canonical.key.page_url();
            let (to, made) = target_page(
                st,
                &rc.live_ids,
                &rc.feedback.events,
                &canonical.key,
                &title,
            )?;
            let how = MoveBy {
                by: mover(st, &who)?,
                kind: KIND_MERGE,
                rule_id: Some(rule.id.clone()),
            };
            let moves: Vec<(String, Refile)> =
                refiles.into_iter().map(|r| (to.clone(), r)).collect();
            let fresh: Vec<String> = made.then(|| to.clone()).into_iter().collect();
            let done = rc.refile(st, &moves, &how, &fresh)?;
            let remaining = remaining + done.deferred;
            let moved = done.moved.into_iter().map(|m| m.thread_id).collect();
            Ok((rule, created, Some(page_of(st, &to)?), moved, remaining))
        })
        .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(json!({
            "rule": rule_view(&rule),
            "page": page.map(|(p, a)| page_view(&s, &p, &a, Some(&rule))),
            "moved": moved,
            "remaining": remaining,
        })),
    ))
}

/// `DELETE /api/live/rules/<id>` (the owner: the token or the extension):
/// deletes a merge rule and un-merges (spec §7.1, owner ruling
/// 2026-10-06): the rule maps nothing from now on, and each thread it
/// merged that has not moved since goes back to the page of the path it
/// was made at (made when missing; another rule's canonical page when one
/// maps that path), at most [`MAX_REFILE`] a request, in one transaction.
/// Answers `{rule, moved, remaining}`; while `remaining` is above 0 the
/// rule is kept as `deleting` and the request is repeated; then it is gone
/// (404 after that).
pub async fn delete_rule(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    p: Result<axum::extract::Path<String>, axum::extract::rejection::PathRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_writes(&who)?;
    let id = super::artifacts::path(p)?;
    let rc = RefileCtx::of(&s);
    let (rule, moved, remaining, deleting) = s
        .store_call(move |st| {
            let rule = st.mark_rule_deleted(&id)?.ok_or(CoreError::NotFound)?;
            let (back, remaining) = st.unmerge_candidates(&rule, MAX_REFILE)?;
            let mut moves = Vec::new();
            let mut fresh: Vec<String> = Vec::new();
            for (key, refile) in back {
                let title = clean_title("", &key.page_url());
                let made = target_page(st, &rc.live_ids, &rc.feedback.events, &key, &title);
                let (to, made) = match made {
                    Ok(m) => m,
                    Err(e) => {
                        for aid in &fresh {
                            let _ = rc.discard_if_empty(st, aid);
                        }
                        return Err(e);
                    }
                };
                if made {
                    fresh.push(to.clone());
                }
                moves.push((to, refile));
            }
            let how = MoveBy {
                by: mover(st, &who)?,
                kind: KIND_UNMERGE,
                rule_id: Some(rule.id.clone()),
            };
            let done = rc.refile(st, &moves, &how, &fresh)?;
            // Pages made for threads left for the next request stay only
            // once they hold one.
            for aid in &fresh {
                rc.discard_if_empty(st, aid)?;
            }
            let remaining = remaining + done.deferred;
            let deleting = remaining > 0;
            if !deleting {
                // Not when a re-add put it back in force meanwhile.
                st.drop_rule(&rule.id)?;
            }
            let moved: Vec<String> = done.moved.into_iter().map(|m| m.thread_id).collect();
            Ok((rule, moved, remaining, deleting))
        })
        .await?;
    let mut rule = rule_view(&rule);
    rule["deleting"] = json!(deleting);
    Ok(Json(
        json!({"rule": rule, "moved": moved, "remaining": remaining}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_control_characters_and_are_cut() {
        assert_eq!(clean_title("  A\u{0}b\n\tc  ", "f"), "Ab c");
        assert_eq!(clean_title("\u{1b}\u{7f} ", "f"), "f");
        assert_eq!(
            clean_title(&"é".repeat(300), "f").chars().count(),
            MAX_TITLE
        );
        assert_eq!(clean_title("a\u{2028}b", "f"), "a b");
    }
}
