//! The audit view (spec `docs/superpowers/specs/2026-10-21-claxbot-design.md`
//! §9.2, §10): the recorded events, filtered and paged newest first.
//!
//! - `GET /api/audit?q=&actor=&group=&artifact=&from=&to=&cursor=` →
//!   `{events: [{seq, at, actor: {id, name}, on_behalf_of: {id, name}|null,
//!   kind, summary, artifact_id, run}], next_cursor}`. `actor` is
//!   `claxbot` or a person key; `group` one of `views`, `changes`, `bot`,
//!   `credits`, `admin`; `from` and `to` RFC 3339 (or `YYYY-MM-DD`);
//!   `cursor` the page before's `next_cursor`.
//! - `GET /api/audit/<seq>` → the event with its stored `body`.
//!
//! Who may: admins read every event, on a hosted daemon every event but
//! those naming an artifact out of their own reach (left out of the list,
//! 404 by `seq`; spec 2026-10-22 §3); the owner's connected people the
//! events their scope holds ([`clax_store::audit_query`]); agents get 403
//! `not_for_agents`, anyone else 403 `forbidden`, no credential 401.
//! Opening or filtering the view is recorded as `view.audit {filters}`, at
//! most once in five minutes per viewer and filter; on a hosted daemon its
//! search words (`q`) are shown to, and searched for, its viewer alone.

use super::artifacts::path;
use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, RawQuery, State};
use clax_core::audit::{AuditKind, KindGroup};
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::DeniedToGrants;
use clax_http::auth::Caller;
use clax_http::bot::{ACTOR_ID, HANDLE, Names, may_use, person_of, record_view, role_of};
use clax_http::error::ApiError;
use clax_http::hosted::MemberRights;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::audit_query::{ActorFilter, AuditEvent, AuditQuery, AuditReader, acted_as};
use serde_json::{Value, json};

/// Who reads, and as whom: the caller, their reader and, for an admin on
/// a hosted daemon, their scope, which events naming an artifact out of
/// it are left out by ([`out_of_reach`]).
async fn reader(
    s: &AppState,
    d: &DeniedToGrants,
) -> Result<(Caller, AuditReader, Option<MemberRights>), ApiError> {
    d.refuse_agents()?;
    let caller = d.caller(s).await?;
    let person = may_use(&caller)?;
    let (c, hosted) = (caller.clone(), s.hosted.is_some());
    let (admin, scope) = s
        .store_call(move |st| {
            let admin = role_of(st, &c)?.is_admin();
            let scope = if admin && hosted {
                super::bot::scope_of(st, &c)?
            } else {
                None
            };
            Ok((admin, scope))
        })
        .await?;
    Ok((
        caller,
        if admin {
            AuditReader::Admin
        } else {
            AuditReader::Person(person)
        },
        scope,
    ))
}

/// Whether event `e` names an artifact (either of its two) out of
/// `scope`'s reach, as its listings decide it ([`MemberRights::lists`]);
/// never without a scope.
fn out_of_reach(st: &Store, scope: Option<&MemberRights>, e: &AuditEvent) -> bool {
    let Some(m) = scope else {
        return false;
    };
    [e.ids.artifact.as_deref(), e.ids.artifact2.as_deref()]
        .into_iter()
        .flatten()
        .any(|a| !m.lists(st, a))
}

/// Event `e` without its search words (`q` of a `view.audit`'s
/// `filters`) unless `reader` (a person key) recorded it: they can quote
/// a page's text, and are their viewer's alone. The other filters stay.
fn without_others_words(e: &mut AuditEvent, reader: Option<&str>) {
    if e.kind != AuditKind::ViewAudit.as_str() || reader.is_some_and(|p| acted_as(&e.actor, p)) {
        return;
    }
    let Some(filters) = e.body.get("filters").and_then(Value::as_str) else {
        return;
    };
    let kept = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(url::form_urlencoded::parse(filters.as_bytes()).filter(|(k, _)| k != "q"))
        .finish();
    e.body["filters"] = Value::String(kept);
}

/// A time of a filter.
fn time(k: &str, v: &str) -> Result<chrono::DateTime<chrono::Utc>, ApiError> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(v) {
        return Ok(t.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc())
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_parameter",
                format!("{k} is an RFC 3339 time or a YYYY-MM-DD date"),
            )
        })
}

/// The query `raw` asks for. An unknown or repeated parameter is refused.
///
/// # Errors
/// 400 `invalid_parameter`.
pub fn audit_query(raw: &str) -> Result<AuditQuery, ApiError> {
    let mut q = AuditQuery::default();
    let mut seen: Vec<String> = Vec::new();
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        let (k, v) = (k.into_owned(), v.into_owned());
        if seen.contains(&k) {
            return Err(ApiError::bad_request(
                "invalid_parameter",
                format!("{k} may be given once"),
            ));
        }
        seen.push(k.clone());
        if v.is_empty() {
            continue;
        }
        match k.as_str() {
            "q" => q.q = Some(v),
            "actor" => {
                q.actor = Some(if v == HANDLE || v == ACTOR_ID {
                    ActorFilter::Bot
                } else {
                    ActorFilter::Person(v)
                });
            }
            "group" => {
                q.group = Some(KindGroup::parse(&v).ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_parameter",
                        "group is views, changes, bot, credits or admin",
                    )
                })?);
            }
            "artifact" => q.artifact = Some(v),
            "from" => q.from = Some(time(&k, &v)?),
            "to" => q.to = Some(time(&k, &v)?),
            "cursor" => {
                q.cursor =
                    Some(v.parse().map_err(|_| {
                        ApiError::bad_request("invalid_parameter", "cursor is a seq")
                    })?);
            }
            "limit" => {
                q.limit = v.parse().map_err(|_| {
                    ApiError::bad_request("invalid_parameter", "limit is a whole number")
                })?;
            }
            _ => {
                return Err(ApiError::bad_request(
                    "invalid_parameter",
                    format!("no parameter is named {k}"),
                ));
            }
        }
    }
    Ok(q)
}

/// Event `e` as the view lists it (with its `body` when `body`).
fn event_view(names: &mut Names<'_>, e: &AuditEvent, body: bool) -> clax_core::Result<Value> {
    let on_behalf_of = match &e.on_behalf_of {
        Some(p) => names.person(p)?,
        None => Value::Null,
    };
    let mut v = json!({
        "seq": e.seq, "at": e.at, "actor": names.actor(&e.actor)?,
        "on_behalf_of": on_behalf_of, "kind": e.kind, "summary": e.summary,
        "artifact_id": e.artifact_id, "run": e.run,
    });
    if body {
        v["body"] = e.body.clone();
    }
    Ok(v)
}

/// Whether `reader` may see events naming a live page (either of their
/// two artifacts): only the owner reads those, so another reader's page
/// leaves them out.
fn hide_live(
    st: &Store,
    caller: &Caller,
    live: &clax_realtime::live::LiveIds,
    e: &AuditEvent,
) -> bool {
    let _ = st;
    !caller.is_owner()
        && [e.ids.artifact.as_deref(), e.ids.artifact2.as_deref()]
            .into_iter()
            .flatten()
            .any(|a| live.contains(a))
}

/// `GET /api/audit` (see the module documentation).
pub async fn list(
    State(s): State<AppState>,
    d: DeniedToGrants,
    who: Identity,
    RawQuery(raw): RawQuery,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, reader, scope) = reader(&s, &d).await?;
    let raw = raw.unwrap_or_default();
    let mut q = audit_query(&raw)?;
    let (bot, live, hosted) = (s.bot.clone(), s.live_ids.clone(), s.hosted.is_some());
    let v = s
        .store_call(move |st| {
            let person = person_of(&caller);
            if hosted {
                // Before the words, the limit and the cursor apply: live
                // pages for all but the owner, pages out of an admin's
                // reach, and others' search words.
                let known = st.artifact_ids_known()?;
                if !caller.is_owner() {
                    q.hidden = known.iter().filter(|a| live.contains(a)).cloned().collect();
                }
                if let Some(m) = &scope {
                    q.within = Some(known.into_iter().filter(|a| m.lists(st, a)).collect());
                }
                q.searcher.clone_from(&person);
            }
            let page = st.query_audit(&reader, &q)?;
            let mut names = Names::new(st);
            let events = page
                .events
                .iter()
                .filter(|e| !hide_live(st, &caller, &live, e))
                .map(|e| event_view(&mut names, e, false))
                .collect::<clax_core::Result<Vec<_>>>()?;
            // On a hosted daemon a view filtered by an artifact its viewer
            // reaches names it, so only admins who reach it read it.
            let mut rec =
                clax_http::bot::view(st, AuditKind::ViewAudit).with("filters", raw.as_str());
            if hosted && let Some(a) = &q.artifact {
                let reach = match &scope {
                    Some(m) => Some(m.clone()),
                    None => super::bot::scope_of(st, &caller)?,
                };
                if ArtifactId::parse(a).is_ok() && reach.is_some_and(|m| m.lists(st, a)) {
                    rec.ids.artifact = Some(a.clone());
                }
            }
            record_view(st, &bot, &who, &caller, &raw, rec)?;
            Ok(json!({"events": events, "next_cursor": page.next_cursor}))
        })
        .await?;
    Ok(Json(v))
}

/// `GET /api/audit/<seq>` (see the module documentation).
pub async fn get(
    State(s): State<AppState>,
    d: DeniedToGrants,
    p: Result<Path<i64>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let (caller, reader, scope) = reader(&s, &d).await?;
    let seq = path(p)?;
    let (live, hosted) = (s.live_ids.clone(), s.hosted.is_some());
    let v = s
        .store_call(move |st| {
            let mut e = st
                .audit_event(&reader, seq)?
                .filter(|e| !hide_live(st, &caller, &live, e))
                .filter(|e| !out_of_reach(st, scope.as_ref(), e))
                .ok_or(CoreError::NotFound)?;
            if hosted {
                without_others_words(&mut e, person_of(&caller).as_deref());
            }
            event_view(&mut Names::new(st), &e, true)
        })
        .await?;
    Ok(Json(v))
}
