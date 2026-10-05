//! `/api/live/*` (spec 2026-10-05-chrome-overlay-design §9.2): look up a
//! page URL's live page, and post a comment on a live page with its
//! screenshot and snapshot. Viewer routes: no token, and only from the
//! shell's origin or a script ([`SameOrigin`]).

use super::assets::multipart_error;
use super::threads::{create_thread_now, publish_thread};
use crate::auth::has_token;
use crate::error::ApiError;
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::{SameOrigin, author};
use axum::Json;
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use clax_core::live::{PageUrl, parse_page_url};
use clax_core::model::Artifact;
use clax_core::store::live::LivePage;
use clax_core::store::threads::{NewThread, clip_problem};
use clax_core::{Anchor, ArtifactId, CoreError, Event};
use serde::Deserialize;
use serde_json::{Value, json};

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
/// the artifact's title and current version, and `url`, its Clax view.
pub(crate) fn page_view(s: &AppState, p: &LivePage, a: &Artifact) -> Value {
    json!({
        "artifact_id": p.artifact_id,
        "origin": p.origin,
        "path": p.path,
        "page_url": format!("{}{}", p.origin, p.path),
        "title": a.title,
        "current_version": a.current_version,
        "url": format!("{}/a/{}", s.browser_base.trim_end_matches('/'), p.artifact_id),
    })
}

#[derive(Deserialize)]
pub struct PageQuery {
    url: String,
}

/// `GET /api/live/pages?url=`: `{page, route}`, the live page the URL names
/// ([`page_view`], or null) and the URL's route (or null). Never creates a page.
pub async fn page(
    State(s): State<AppState>,
    _o: SameOrigin,
    q: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = page_url(&s, &q.url)?;
    let key = pu.key.clone();
    let found = s
        .store_call(move |st| {
            let Some(p) = st.find_live_page(&key)? else {
                return Ok(None);
            };
            let Some(a) = st.get_artifact(&ArtifactId::parse(&p.artifact_id)?)? else {
                return Ok(None);
            };
            Ok(Some((p, a)))
        })
        .await?;
    Ok(Json(json!({
        "page": found.map(|(p, a)| page_view(&s, &p, &a)),
        "route": pu.route,
    })))
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
}

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
        match name.as_str() {
            "url" => f.url = Some(text()?),
            "title" => f.title = Some(text()?),
            "anchor" => f.anchor = Some(text()?),
            "body" => f.body = Some(text()?),
            "clip" => f.clip = Some(bytes.to_vec()),
            "snapshot" => {
                if bytes.len() > MAX_SNAPSHOT {
                    return Err(ApiError::bad_request(
                        "snapshot_too_large",
                        "a snapshot is at most 8 MiB",
                    ));
                }
                f.snapshot = Some(bytes.to_vec());
            }
            _ => {}
        }
    }
    Ok(f)
}

/// `POST /api/live/threads` (multipart `url`, `title`, `anchor`, `body`,
/// optional `clip`, `snapshot`): finds or creates the live page `url` names,
/// stores `snapshot` as its next version when it differs from the current
/// one (linking the page's pending addresses to that version), and creates
/// the thread on that version as the request's viewer, with the anchor's
/// `route` from `url` and `clip` as its screenshot; an `@agent` mention
/// sends it. Answers `201 {thread, page, version,
/// clip_error?}`; a clip failing `clip_problem` is dropped and reported.
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
    let title = clean_title(f.title.as_deref().unwrap_or(""), &pu.key.page_url());
    let clip_error = f.clip.as_deref().and_then(clip_problem);
    let clip = if clip_error.is_some() { None } else { f.clip };
    let body = f.body.unwrap_or_default();
    let with_path = has_token(&headers, &s.token);
    let ctx = s.feedback_ctx();
    let events = s.events.clone();
    let live_ids = s.live_ids.clone();
    let key = pu.key;
    let (view, page, artifact, version) = s
        .store_call(move |st| {
            let e = live_ids.ensure_page(st, &key, &title, Some(&snapshot))?;
            let id = ArtifactId::parse(&e.artifact.id)?;
            if e.new_version {
                events.publish(Event::Version {
                    artifact_id: e.artifact.id.clone(),
                    n: e.version.n,
                    by_page: false,
                    title: Some(e.artifact.title.clone()),
                    at: Some(e.version.created_at.clone()),
                });
                link_pending(st, &ctx, &id, e.version.n)?;
            }
            let (author_name, author_public_id) = author(st, &who)?;
            let view = create_thread_now(
                st,
                &ctx,
                &id,
                NewThread {
                    author_public_id,
                    version_n: e.version.n,
                    anchor,
                    author_name,
                    body,
                    clip,
                    via_page: false,
                },
                with_path,
            )?;
            let page = st.live_page_of(&id)?.ok_or(CoreError::NotFound)?;
            Ok((view, page, e.artifact, e.version.n))
        })
        .await?;
    let mut out = json!({
        "thread": view,
        "page": page_view(&s, &page, &artifact),
        "version": version,
    });
    if let Some(e) = clip_error {
        out["clip_error"] = json!(e);
    }
    Ok((StatusCode::CREATED, Json(out)))
}

/// Links the pending addresses of the live page `id` to its new version `n`
/// and publishes each linked thread; returns their IDs.
fn link_pending(
    st: &clax_core::Store,
    ctx: &crate::feedback::FeedbackCtx,
    id: &ArtifactId,
    n: u32,
) -> clax_core::Result<Vec<String>> {
    let linked = st.link_pending(id, n)?;
    for tid in &linked {
        if let Some(t) = st.get_thread(tid)? {
            publish_thread(ctx, st, &t)?;
        }
    }
    Ok(linked)
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
            "pending" => {
                let ids: Vec<String> = serde_json::from_str(&text()?).map_err(|_| {
                    ApiError::bad_request("invalid_args", "pending is a JSON array of thread IDs")
                })?;
                pending.replace(ids).is_some()
            }
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
            let Some(p) = st.find_live_page(&key)? else {
                return Ok(None);
            };
            let id = ArtifactId::parse(&p.artifact_id)?;
            let Some((v, linked)) = st.snapshot_pending(&id, &title, &f.snapshot, &f.pending)?
            else {
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
            Ok(Some((p, a, v.n, linked)))
        })
        .await?;
    let Some((page, artifact, n, linked)) = done else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "nothing_pending",
            "none of the named threads has an address waiting for a snapshot",
        ));
    };
    Ok(Json(json!({
        "page": page_view(&s, &page, &artifact),
        "version": n,
        "linked": linked,
    })))
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
