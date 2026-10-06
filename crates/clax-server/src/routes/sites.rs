//! `/api/live/sites/*` (spec 2026-10-05-chrome-overlay-design §7.2, owner
//! decisions 2026-10-06): joined sites. Clax suggests that an origin is the
//! same app as another of its host family, and the owner confirms; the
//! owner joins and splits origins. Owner-only, under `/api/live/` (so hidden
//! from the LAN, L10), and admitted by the extension gateway's allowlist.

use super::live::{
    RefileCtx, clean_title, mover, origin_of, owner_reads, owner_writes, page_url, target_page,
};
use crate::error::ApiError;
use crate::identity::Identity;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use clax_core::store::joined::JoinedSite;
use clax_core::store::site::{KIND_JOIN, KIND_MERGE, MAX_REFILE, MoveBy, Refile};
use clax_core::{Event, Store};
use serde::Deserialize;
use serde_json::{Value, json};

/// A site as the routes answer with it: its key, its name (its most
/// recently used origin), whether it joins several origins, and its
/// origins (`{origin, joined_at, last_used_at}`, the most recently used
/// first).
pub(crate) fn site_view(site: &JoinedSite) -> Value {
    json!({
        "key": site.key,
        "name": site.newest(),
        "joined": site.joined(),
        "origins": site.origins,
    })
}

/// `GET /api/live/sites` (the owner): every site with live pages, the most
/// recently active first, as `{sites: [{site, pages, threads,
/// last_activity}]}` ([`site_view`]); one entry per joined site.
pub async fn list(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    owner_reads(&who)?;
    let sites = s.store_call(|st| st.live_sites()).await?;
    let out: Vec<Value> = sites
        .iter()
        .map(|x| {
            json!({
                "site": site_view(&x.site),
                "pages": x.pages,
                "threads": x.threads,
                "last_activity": x.last_activity,
            })
        })
        .collect();
    Ok(Json(json!({ "sites": out })))
}

#[derive(Deserialize)]
pub struct SuggestQuery {
    url: String,
    #[serde(default)]
    title: Option<String>,
}

/// `GET /api/live/sites/suggest?url=&title=` (the owner): the sites the
/// URL's origin may be the same app as, when it joined none (at most
/// three, the most recently active first): `{origin, site, suggestions:
/// [{origin, site, reason, path}]}`. A suggestion is a site with an origin
/// of the same host family (loopback names: `localhost`, `*.localhost`,
/// `127.0.0.0/8`, `[::1]`) with a live page of the URL's path, or titled
/// as the page is, or of a path the origin's own pages have (`reason`:
/// `path` or `title`), that the owner has not answered never (or not now,
/// within a day) for. Never writes.
pub async fn suggest(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    q: Result<Query<SuggestQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_reads(&who)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let pu = page_url(&s, &q.url)?;
    let title = clean_title(q.title.as_deref().unwrap_or(""), "");
    let origin = pu.key.origin.clone();
    let o = origin.clone();
    let (site, found) = s
        .store_call(move |st| {
            Ok((
                st.joined_site(&o)?,
                st.join_suggestions(&o, &pu.key.path, &title)?,
            ))
        })
        .await?;
    let suggestions: Vec<Value> = found
        .iter()
        .map(|x| {
            json!({
                "origin": x.origin,
                "site": site_view(&x.site),
                "reason": x.reason,
                "path": x.path,
            })
        })
        .collect();
    Ok(Json(json!({
        "origin": origin,
        "site": site_view(&site),
        "suggestions": suggestions,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinBody {
    origin: String,
    with: String,
}

/// One batch of a join's work: the threads of the pages it left pending
/// re-filed onto the site's pages of their paths ([`KIND_JOIN`]), the
/// emptied pages deleted; once none is left, the site's merge rules applied
/// across it ([`KIND_MERGE`]). Returns the re-filed thread IDs and how many
/// threads are left for the next request.
fn join_batch(
    st: &Store,
    rc: &RefileCtx,
    s: &AppState,
    who: &Identity,
    key: &str,
) -> clax_core::Result<(Vec<String>, usize)> {
    let (moves, mut remaining) = st.join_candidates(key, MAX_REFILE)?;
    let mut moved = Vec::new();
    if !moves.is_empty() {
        let how = MoveBy {
            by: mover(st, who)?,
            kind: KIND_JOIN,
            rule_id: None,
        };
        let done = rc.refile(st, &moves, &how, &[])?;
        remaining += done.deferred;
        moved.extend(done.moved.into_iter().map(|m| m.thread_id));
    }
    let (empty, rekeyed) = st.settle_joined_pages(key)?;
    s.live_ids.rekey(&rekeyed, key);
    for aid in &empty {
        rc.discard_if_empty(st, aid)?;
    }
    if !moves.is_empty() {
        return Ok((moved, remaining));
    }
    // The site's rules apply across it: a joined origin's pages they map.
    for rule in st.live_rules(key)? {
        let (refiles, rest) = st.merge_candidates(&rule, MAX_REFILE)?;
        if refiles.is_empty() {
            continue;
        }
        if !moved.is_empty() {
            remaining += refiles.len() + rest;
            continue;
        }
        let canonical = clax_core::live::PageKey {
            origin: key.to_string(),
            path: rule.pattern.clone(),
        };
        let title = canonical.page_url();
        let (to, made) = target_page(st, &rc.live_ids, &rc.feedback.events, &canonical, &title)?;
        let how = MoveBy {
            by: mover(st, who)?,
            kind: KIND_MERGE,
            rule_id: Some(rule.id.clone()),
        };
        let moves: Vec<(String, Refile)> = refiles.into_iter().map(|r| (to.clone(), r)).collect();
        let fresh: Vec<String> = made.then(|| to.clone()).into_iter().collect();
        let done = rc.refile(st, &moves, &how, &fresh)?;
        remaining += rest + done.deferred;
        moved.extend(done.moved.into_iter().map(|m| m.thread_id));
    }
    Ok((moved, remaining))
}

/// `POST /api/live/sites/join` `{origin, with}` (the owner: the token or the
/// extension; each an origin or a page URL): joins `origin` (with the site
/// it is in) to the site of `with`, which must have live pages; the
/// daemon's own origin is refused (`own_origin`). From then on both are one
/// site: lookups by path from any of its origins find the site's pages, its
/// threads list together, and a watch on any of its origins covers them
/// all, origins joined later included. A page of `origin` whose path the
/// site has a page of is merged into it as moves are, at most
/// [`MAX_REFILE`] threads a request (each batch one transaction), and
/// deleted once empty; then the site's merge rules apply across it. Answers
/// `{site, joined, moved, remaining}` (`joined`: this call joined them;
/// `moved`: the re-filed thread IDs; `remaining`: threads left: repeat the
/// request, which is idempotent, until it is 0). 400 `same_origin`,
/// `unknown_site`, `too_many_origins`, `too_many_rules`.
pub async fn join(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    body: Result<Json<JoinBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_writes(&who)?;
    let b = super::artifacts::body(body)?;
    let origin = origin_of(&s, &b.origin)?;
    let with = origin_of(&s, &b.with)?;
    let rc = RefileCtx::of(&s);
    let st2 = s.clone();
    let (site, joined, moved, remaining) = s
        .store_call(move |st| {
            let j = st.join_origins(&origin, &with)?;
            if j.changed {
                st2.live_ids.rekey(&j.rekeyed, &j.site.key);
                st2.live_ids.reload_sites(st)?;
                st2.events.publish(Event::Site {
                    site: j.site.key.clone(),
                    origins: j.site.origin_names(),
                    left: Vec::new(),
                });
            }
            let (moved, remaining) = join_batch(st, &rc, &st2, &who, &j.site.key)?;
            Ok((st.joined_site(&j.site.key)?, j.changed, moved, remaining))
        })
        .await?;
    Ok(Json(json!({
        "site": site_view(&site),
        "joined": joined,
        "moved": moved,
        "remaining": remaining,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitBody {
    origin: String,
}

/// `POST /api/live/sites/split` `{origin}` (the owner): splits `origin` off
/// its site. From then on it is a site of its own again: its new pages and
/// threads are its own. What the site holds (its pages, threads and rules,
/// whichever origin they were made on) stays with the site; when `origin`
/// was the site's key, the key moves to its most recently used origin left.
/// A site left with one origin is that origin's own. Clax does not suggest
/// the pair again. Answers `{split, origin, site}` (`split`: false, and
/// nothing written, when `origin` joined no site; `site`: what is left).
/// 400 `joining` while the site's key has a join to finish.
pub async fn split(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    body: Result<Json<SplitBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_writes(&who)?;
    let b = super::artifacts::body(body)?;
    let origin = origin_of(&s, &b.origin)?;
    let o = origin.clone();
    let st2 = s.clone();
    let (split, site) = s
        .store_call(move |st| {
            let Some(sp) = st.split_origin(&o)? else {
                return Ok((false, st.joined_site(&o)?));
            };
            st2.live_ids.rekey(&sp.rekeyed, &sp.site.key);
            st2.live_ids.reload_sites(st)?;
            st2.events.publish(Event::Site {
                site: sp.site.key.clone(),
                origins: sp.site.origin_names(),
                left: vec![o.clone()],
            });
            Ok((true, sp.site))
        })
        .await?;
    Ok(Json(json!({
        "split": split,
        "origin": origin,
        "site": site_view(&site),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBody {
    origin: String,
    with: String,
    answer: String,
}

/// `POST /api/live/sites/answer` `{origin, with, answer}` (the owner): the
/// owner's answer to the suggestion that `origin` is the same app as
/// `with`: `never` (never suggested again) or `later` ("Not now": not
/// suggested for a day). Answers `{answer}`. 400 `invalid_answer`,
/// `same_origin`.
pub async fn answer(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    body: Result<Json<AnswerBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    owner_writes(&who)?;
    let b = super::artifacts::body(body)?;
    let origin = origin_of(&s, &b.origin)?;
    let with = origin_of(&s, &b.with)?;
    let a = b.answer.clone();
    s.store_call(move |st| st.answer_join(&origin, &with, &a))
        .await?;
    Ok(Json(json!({ "answer": b.answer })))
}
