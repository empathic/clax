//! The `db` capability's routes (spec §6 "Docs"). Every route refuses a
//! foreign `Origin` ([`SameOrigin`]) and acts as the [`CallerParts`] caller.
//! A caller without the token gets 403 `not_declared` when the artifact's
//! current declaration does not include `db` (a publish or a metadata edit
//! can drop it).
//! A document the caller may not read answers 404, like a missing one; a
//! write the rules refuse answers 404 too. Those 404s name the document's
//! `path` in the error; a missing artifact's 404 does not. Each change publishes the `doc`
//! SSE event, which carries the path and version but never the body.
//!
//! Agent channels (spec §7.5 docs row, §8.2, §8.3) get every document
//! through [`clax_http::views::thread::Vis`]: one whose authors their reader may not
//! see is its placeholder (`{doc_id, version, withheld: {authors}}`) when
//! read or written by its path, and in a listing is never matched,
//! ordered or listed, only counted in `withheld_count`. A write whose
//! outcome depends on its content (`str_replace`, and a merge: `PATCH`, a
//! batch `update`, `acquire` with data) is 403 `withheld` before anything
//! is evaluated; blind writes (`PUT`, `DELETE`, a batch `set` or `delete`,
//! `acquire` without data) stay allowed and add the writer to its
//! authors, so they never make it the writer's. A lease in force on it is
//! busy whatever its holder.
//!
//! Every write (`PUT`, `PATCH`, `DELETE`, a batch, `str_replace`,
//! `acquire`) by a person beyond the artifact's live seats is 403
//! `spectator` ([`clax_http::seats::require_seat`]; spec
//! 2026-10-11-clax-dev-hosted-design §7, §16.1), before the artifact's
//! declaration is read; agents and the owner are never refused it.
//!
//! The documents live in the artifact's own store, reached through
//! [`AppState::app_data`] ([`clax_http::app_data_host`]): each route first
//! decides, on the instance, the caller, the artifact's declared rules, a
//! page's authors and what the caller may see ([`Decided`]), then sends
//! the operation, then shows its answer as the caller may see it.
//!
//! The read routes also serve another app reading these documents
//! (`X-Clax-Reader`, [`clax_http::db_caller`]); the owner's grants for that and
//! the collection listing for the agent live here too.

use super::artifacts::{JsonBytes, body, parse_body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use clax_core::audit::AuditCtx;
use clax_core::db::{Caller, invalid_argument};
use clax_core::{ArtifactId, CoreError, Event, EventBus};
use clax_http::auth::RequireToken;
use clax_http::db_caller::CallerParts;
use clax_http::db_caller::{not_granted, reader_of};
use clax_http::error::ApiError;
use clax_http::headers::parse_id;
use clax_http::hosted::HostedRequest;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_http::views::thread::{Vis, VisFor};
use clax_store::Store;
use clax_store::app_data::{AppDataOp, AppDataReply, DocView, WireCaller, WriteCtx};
use clax_store::docs::{
    Acquire, BatchOp, BatchWrite, DocChange, DocQuery, Pin, StrReplace, parse_where,
};
use serde::Deserialize;
use serde_json::{Value, json};

fn announce(events: &EventBus, aid: &str, changes: impl IntoIterator<Item = DocChange>) {
    for c in changes {
        events.publish(Event::Doc {
            artifact_id: aid.to_string(),
            path: c.path,
            version: c.version,
            private_to: c.private_to,
            read_level: c.read_level,
            self_read: c.self_read,
        });
    }
}

type DocParams = Result<Path<(String, String)>, PathRejection>;

/// A write's answer, or its refusal: 403 `withheld` for a content-dependent
/// write (`str_replace`, a merge) on a document the caller's reader may not
/// see, naming its withheld authors (in a batch, with the operation's
/// index and path, the caller's own).
type Answer<T> = Result<T, ApiError>;

/// `e` as [`Answer`]'s refusal when it is a withheld document's, else as
/// the error it is.
fn refusal<T>(vis: &Vis, st: &Store, e: CoreError) -> clax_core::Result<Answer<T>> {
    let withheld = |authors: &[clax_core::visibility::Author]| -> clax_core::Result<ApiError> {
        let mut e = ApiError::forbidden(
            "withheld",
            "this document is withheld until your person approves its authors",
        );
        e.extra
            .insert("authors".into(), json!(vis.filter_authors(st, authors)?));
        Ok(e)
    };
    match e {
        CoreError::DocWithheld { authors, .. } => Ok(Err(withheld(&authors)?)),
        CoreError::InBatch { op, path, error } => match *error {
            CoreError::DocWithheld { authors, .. } => {
                let mut e = withheld(&authors)?;
                e.message = format!("batch write {op} ({path}): {}", e.message);
                e.extra.insert("op".into(), json!(op));
                e.extra.insert("path".into(), json!(path));
                Ok(Err(e))
            }
            error => Err(CoreError::InBatch {
                op,
                path,
                error: Box::new(error),
            }),
        },
        e => Err(e),
    }
}

/// What the instance decides for one call on an artifact's documents: the
/// caller, the artifact's declared capabilities (whose `db` rules its store
/// applies), the authors a page's write adds, and what the caller may see.
struct Decided {
    caller: Caller,
    caps: Value,
    page_authors: Vec<String>,
    vis: Vis,
}

impl Decided {
    /// Decided on `st` for `who` at artifact `id`.
    fn of(
        st: &Store,
        who: &CallerParts,
        id: &ArtifactId,
        vf: &VisFor,
    ) -> clax_core::Result<Decided> {
        let vis = vf.build(st)?;
        let caller = who.resolve_for(st, id)?;
        let caps = st.doc_capabilities(id)?;
        let page_authors = if caller.via_page {
            st.doc_page_authors(id)?
        } else {
            Vec::new()
        };
        Ok(Decided {
            caller,
            caps,
            page_authors,
            vis,
        })
    }

    fn caller(&self) -> WireCaller {
        WireCaller::from(&self.caller)
    }

    /// A write under `audit`, adding the page's authors.
    fn ctx(&self, audit: &AuditCtx) -> WriteCtx {
        WriteCtx {
            page_authors: self.page_authors.clone(),
            ..WriteCtx::of(audit)
        }
    }

    /// The caller's reader's view of artifact `id`'s documents, when it may
    /// withhold any.
    fn view(&self, id: &ArtifactId) -> Option<DocView> {
        self.vis.view().and_then(|v| v.doc_view(id.as_str()))
    }
}

/// What `who` may do at artifact `id`, decided on the instance.
async fn decide(
    s: &AppState,
    who: CallerParts,
    id: &ArtifactId,
    vf: VisFor,
) -> Result<Decided, ApiError> {
    let id = id.clone();
    s.store_call(move |st| Decided::of(st, &who, &id, &vf))
        .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBody {
    data: Value,
    #[serde(default)]
    if_version: Option<u64>,
    #[serde(default)]
    lww: bool,
}

/// The document: `{doc: {path, collection, id, data, version, updated_at}}`,
/// or for an agent whose reader may not see it its placeholder; 404 with
/// `path` when it is missing or the caller may not read it.
pub async fn get(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsGet>,
    _o: SameOrigin,
    who: CallerParts,
    p: DocParams,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Get {
        caps: d.caps.clone(),
        path: doc.clone(),
        caller: d.caller(),
    };
    let Some((got, authors)) = s.app_data_call(&id, op, |r| r?.into_doc()).await? else {
        return Err(CoreError::DocNotFound { path: doc }.into());
    };
    let shown = s
        .store_call(move |st| d.vis.filter_doc(st, id.as_str(), &got, &authors))
        .await?;
    Ok(Json(json!({"doc": shown})))
}

/// Replaces or creates the document: `{doc, created}`.
pub async fn put(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsPut>,
    _o: SameOrigin,
    who: CallerParts,
    audit: AuditCtx,
    p: DocParams,
    req: Result<Json<WriteBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let b = body(req)?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Set {
        ctx: d.ctx(&audit),
        caps: d.caps.clone(),
        path: doc,
        data: b.data,
        pin: Pin {
            if_version: b.if_version,
            lww: b.lww,
        },
        caller: d.caller(),
    };
    let events = s.events.clone();
    let aid = id.to_string();
    let w = s
        .app_data_call(&id, op, move |r| {
            let w = r?.into_written()?;
            announce(&events, &aid, w.change.clone());
            Ok(w)
        })
        .await?;
    let created = w.created;
    let doc = s
        .store_call(move |st| d.vis.filter_written(st, id.as_str(), &w))
        .await?;
    Ok(Json(json!({"doc": doc, "created": created})))
}

/// Merges into the existing document: `{doc}`. For an agent whose reader
/// may not see the document, 403 `withheld` before the pin or the merge.
pub async fn patch(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsPatch>,
    _o: SameOrigin,
    who: CallerParts,
    audit: AuditCtx,
    p: DocParams,
    req: Result<Json<WriteBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let b = body(req)?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Update {
        ctx: d.ctx(&audit),
        caps: d.caps.clone(),
        path: doc,
        patch: b.data,
        pin: Pin {
            if_version: b.if_version,
            lww: b.lww,
        },
        caller: d.caller(),
        view: d.view(&id),
    };
    let events = s.events.clone();
    let aid = id.to_string();
    let w = s
        .app_data_call(&id, op, move |r| {
            Ok(r.and_then(AppDataReply::into_written)
                .inspect(|w| announce(&events, &aid, w.change.clone())))
        })
        .await?;
    let doc = s
        .store_call(move |st| match w {
            Ok(w) => Ok(Ok(d.vis.filter_written(st, id.as_str(), &w)?)),
            Err(e) => refusal(&d.vis, st, e),
        })
        .await??;
    Ok(Json(json!({"doc": doc})))
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    if_version: Option<u64>,
    #[serde(default)]
    lww: bool,
}

/// Deletes the document: `{deleted}` (`false` when there was none).
pub async fn delete(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsDelete>,
    _o: SameOrigin,
    who: CallerParts,
    audit: AuditCtx,
    p: DocParams,
    q: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Delete {
        ctx: d.ctx(&audit),
        caps: d.caps.clone(),
        path: doc,
        pin: Pin {
            if_version: q.if_version,
            lww: q.lww,
        },
        caller: d.caller(),
    };
    let events = s.events.clone();
    let aid = id.to_string();
    let w = s
        .app_data_call(&id, op, move |r| {
            let w = r?.into_written()?;
            announce(&events, &aid, w.change.clone());
            Ok(w)
        })
        .await?;
    // A delete leaves no document to show.
    let deleted = w.deleted;
    let left = s
        .store_call(move |st| d.vis.filter_written(st, id.as_str(), &w))
        .await?;
    debug_assert!(left.is_none());
    Ok(Json(json!({"deleted": deleted})))
}

#[derive(Deserialize)]
pub struct ListQuery {
    collection: String,
    #[serde(rename = "where")]
    where_: Option<String>,
    order_by: Option<String>,
    direction: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

/// One collection's documents: `{docs, next_cursor}`. `where` is a JSON array
/// of `[field, operator, value]` triples. For an agent whose reader may not
/// see every document, only those it may see are matched, ordered and
/// paged, and `withheld_count` counts the collection's others.
pub async fn list(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsList>,
    _o: SameOrigin,
    who: CallerParts,
    aid: Result<Path<String>, PathRejection>,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let filters = match &q.where_ {
        Some(w) => parse_where(&serde_json::from_str(w).map_err(|e| {
            ApiError::bad_request("invalid_argument", format!("where must be JSON: {e}"))
        })?)?,
        None => Vec::new(),
    };
    let descending = match q.direction.as_deref() {
        None | Some("asc") => false,
        Some("desc") => true,
        Some(d) => {
            return Err(ApiError::bad_request(
                "invalid_argument",
                format!("direction is asc or desc, not '{d}'"),
            ));
        }
    };
    let query = DocQuery {
        collection: q.collection,
        filters,
        order_by: q.order_by,
        descending,
        limit: q.limit,
        cursor: q.cursor,
    };
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Query {
        caps: d.caps.clone(),
        query,
        caller: d.caller(),
        view: d.view(&id),
    };
    let page = s.app_data_call(&id, op, |r| r?.into_page()).await?;
    Ok(Json(d.vis.filter_doc_page(page)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchEntry {
    op: String,
    path: String,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    if_version: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    writes: Vec<BatchEntry>,
    #[serde(default)]
    lww: bool,
}

/// Applies up to 50 writes atomically: `{results: [{op, path, version, deleted}]}`.
/// An `update` of a document the agent's reader may not see fails the
/// batch with 403 `withheld`, naming its index and path.
pub async fn batch(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsBatch>,
    _o: SameOrigin,
    who: CallerParts,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
    req: Result<JsonBytes, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let (ops, writes, lww) = parse_body(req, "the docs batch limit", |b: BatchBody| {
        let ops: Vec<String> = b.writes.iter().map(|w| w.op.clone()).collect();
        let writes = b
            .writes
            .into_iter()
            .map(|w| {
                let op = match (w.op.as_str(), w.data) {
                    ("set", Some(d)) => BatchOp::Set(d),
                    ("update", Some(d)) => BatchOp::Update(d),
                    ("delete", None) => BatchOp::Delete,
                    (op, _) => {
                        return Err(invalid_argument(format!(
                            "'{op}' on {}: op is set or update (with data) or delete (without)",
                            w.path
                        )));
                    }
                };
                Ok(BatchWrite {
                    path: w.path,
                    op,
                    if_version: w.if_version,
                })
            })
            .collect::<clax_core::Result<Vec<_>>>()?;
        Ok((ops, writes, b.lww))
    })
    .await?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Batch {
        ctx: d.ctx(&audit),
        caps: d.caps.clone(),
        writes,
        lww,
        caller: d.caller(),
        view: d.view(&id),
    };
    let events = s.events.clone();
    let aid = id.to_string();
    let ws = s
        .app_data_call(&id, op, move |r| {
            Ok(r.and_then(AppDataReply::into_batch)
                .inspect(|ws| announce(&events, &aid, ws.iter().filter_map(|w| w.change.clone()))))
        })
        .await?;
    let results = match ws {
        Ok(ws) => d.vis.filter_batch(&ws, ops),
        Err(e) => {
            return Err(s
                .store_call(move |st| refusal::<()>(&d.vis, st, e))
                .await?
                .expect_err("a refusal is an error"));
        }
    };
    Ok(Json(json!({"results": results})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrReplaceBody {
    path: String,
    field: String,
    old_str: String,
    new_str: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    if_version: Option<u64>,
    #[serde(default)]
    lww: bool,
}

/// Replaces `old_str` with `new_str` in the string field `field` (every
/// occurrence with `replace_all`, else exactly one): `{doc}`, the updated
/// document. For an agent whose reader may not see the document, 403
/// `withheld` with its withheld `authors`, before the pin or the text is
/// looked at.
pub async fn str_replace(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsStrReplace>,
    _o: SameOrigin,
    who: CallerParts,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<StrReplaceBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let b = body(req)?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::StrReplace {
        ctx: d.ctx(&audit),
        caps: d.caps.clone(),
        path: b.path,
        replace: StrReplace {
            field: b.field,
            old_str: b.old_str,
            new_str: b.new_str,
            replace_all: b.replace_all,
        },
        pin: Pin {
            if_version: b.if_version,
            lww: b.lww,
        },
        caller: d.caller(),
        view: d.view(&id),
    };
    let events = s.events.clone();
    let aid = id.to_string();
    let w = s
        .app_data_call(&id, op, move |r| {
            Ok(r.and_then(AppDataReply::into_written)
                .inspect(|w| announce(&events, &aid, w.change.clone())))
        })
        .await?;
    let done = s
        .store_call(move |st| match w {
            Ok(w) => Ok(Ok(d.vis.filter_written(st, id.as_str(), &w)?)),
            Err(e) => refusal(&d.vis, st, e),
        })
        .await??;
    Ok(Json(json!({"doc": done})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquireBody {
    path: String,
    holder: String,
    #[serde(default)]
    ttl_ms: Option<u64>,
    #[serde(default)]
    data: Option<Value>,
}

/// `{acquired, version, expires_at, holder}`. On a document the agent's
/// reader may not see, `data` is 403 `withheld`, and a lease in force is
/// busy whatever its holder.
pub async fn acquire(
    State(s): State<AppState>,
    a: clax_http::agent::Agent<clax_http::agent::DocsAcquire>,
    _o: SameOrigin,
    who: CallerParts,
    audit: AuditCtx,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<AcquireBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    clax_http::seats::require_seat(&s, &a.caller, &id).await?;
    let b = body(req)?;
    let d = decide(&s, who, &id, VisFor::of(&a.caller, &s)).await?;
    let op = AppDataOp::Acquire {
        ctx: d.ctx(&audit),
        caps: d.caps.clone(),
        path: b.path,
        acquire: Acquire {
            holder: b.holder,
            ttl_ms: b.ttl_ms,
            data: b.data,
        },
        caller: d.caller(),
        view: d.view(&id),
    };
    let events = s.events.clone();
    let aid = id.to_string();
    let got = s
        .app_data_call(&id, op, move |r| {
            Ok(r.and_then(AppDataReply::into_acquired)
                .inspect(|(_, change)| announce(&events, &aid, change.clone())))
        })
        .await?;
    match got {
        Ok((a, _)) => Ok(Json(d.vis.filter_acquired(&a))),
        Err(e) => Err(s
            .store_call(move |st| refusal::<()>(&d.vis, st, e))
            .await?
            .expect_err("a refusal is an error")),
    }
}

/// `GET /api/artifacts/<aid>/docs:collections` (token): every collection of
/// the artifact's documents with its document count, sorted by path,
/// without private `data/users/<id>/` ones: `{collections: [{path, count}]}`.
/// A request reading for another app (`X-Clax-Reader`) is refused 403
/// `not_granted`: the listing ignores the read rules, so it is the owner's.
pub async fn collections(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    headers: HeaderMap,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    if let Some(reader) = reader_of(&headers)? {
        return Err(not_granted(&reader, &id).into());
    }
    let caps = {
        let id = id.clone();
        s.store_call(move |st| st.doc_capabilities(&id)).await?
    };
    let cs = s
        .app_data_call(&id, AppDataOp::Collections { caps, caller: None }, |r| {
            r.and_then(AppDataReply::into_collections)
        })
        .await?;
    let cs: Vec<Value> = cs
        .into_iter()
        .map(|(path, count)| json!({"path": path, "count": count}))
        .collect();
    Ok(Json(json!({"collections": cs})))
}

type GrantParams = Result<Path<(String, String)>, PathRejection>;

/// `PUT /api/artifacts/<reader>/db-grants/<source>` (token): the owner lets
/// `reader` read `source`'s documents once it declares the read:
/// `{grant: {source_id, granted_at}}`. Granting again keeps the first grant.
/// 404 for an unknown artifact, and on a hosted daemon for a source out of
/// the caller's reach ([`clax_http::hosted::require_reach`]); 400
/// `invalid_args` when both are one.
pub async fn grant(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    hosted: Option<Extension<HostedRequest>>,
    audit: AuditCtx,
    p: GrantParams,
) -> Result<Json<Value>, ApiError> {
    let (reader, source) = path(p)?;
    let (reader, source) = (parse_id(&reader)?, parse_id(&source)?);
    clax_http::hosted::require_reach(&s, hosted.as_deref(), source.as_str()).await?;
    let g = s
        .store_call(move |st| {
            st.grant_read(&audit, &reader, &source)?;
            st.grants_of(&reader)?
                .into_iter()
                .find(|g| g.source_id == source.as_str())
                .ok_or(clax_core::CoreError::NotFound)
        })
        .await?;
    Ok(Json(json!({"grant": g})))
}

/// `DELETE /api/artifacts/<reader>/db-grants/<source>` (token): removes the
/// owner's grant: `{revoked}` (`false` when there was none). A share the
/// source declares is not a grant and stays. Errors as [`grant`].
pub async fn revoke(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    hosted: Option<Extension<HostedRequest>>,
    audit: AuditCtx,
    p: GrantParams,
) -> Result<Json<Value>, ApiError> {
    let (reader, source) = path(p)?;
    let (reader, source) = (parse_id(&reader)?, parse_id(&source)?);
    clax_http::hosted::require_reach(&s, hosted.as_deref(), source.as_str()).await?;
    let revoked = s
        .store_call(move |st| st.revoke_read(&audit, &reader, &source))
        .await?;
    Ok(Json(json!({"revoked": revoked})))
}

/// `GET /api/artifacts/<reader>/db-grants` (token): the owner's grants to
/// `reader`, oldest first: `{grants: [{source_id, granted_at}]}`, on a
/// hosted daemon those of sources in the caller's reach alone. 404 for an
/// unknown artifact.
pub async fn grants(
    State(s): State<AppState>,
    _d: clax_http::agent::DeniedToGrants,
    _t: RequireToken,
    hosted: Option<Extension<HostedRequest>>,
    aid: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let scope = s.hosted.as_ref().map(|_| hosted.and_then(|h| h.0.member));
    let gs = s
        .store_call(move |st| {
            let mut gs = st.grants_of(&id)?;
            if let Some(scope) = scope {
                gs.retain(|g| scope.as_ref().is_some_and(|m| m.lists(st, &g.source_id)));
            }
            Ok(gs)
        })
        .await?;
    Ok(Json(json!({"grants": gs})))
}
