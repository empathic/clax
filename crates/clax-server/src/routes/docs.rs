//! The `db` capability's routes (spec §6 "Docs"). Every route refuses a
//! foreign `Origin` ([`SameOrigin`]) and acts as the [`CallerParts`] caller.
//! A caller without the token gets 403 `not_declared` when the artifact's
//! current declaration does not include `db` (a publish or a metadata edit
//! can drop it).
//! A document the caller may not read answers 404, like a missing one; a
//! write the rules refuse answers 404 too. Those 404s name the document's
//! `path` in the error; a missing artifact's 404 does not. Each change publishes the `doc`
//! SSE event, which carries the path and version but never the body.

use super::artifacts::{body, body_within, parse_id, path};
use crate::db_caller::CallerParts;
use crate::error::ApiError;
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::db::invalid_argument;
use clax_core::store::docs::{
    Acquire, BatchOp, BatchWrite, DocChange, DocQuery, Pin, StrReplace, parse_where,
};
use clax_core::{Event, EventBus};
use serde::Deserialize;
use serde_json::{Value, json};

/// Request body cap for `docs:batch`: 50 documents of up to 256 KiB each,
/// with room for JSON escaping. Single-document writes keep axum's 2 MB default.
pub const DOCS_BATCH_LIMIT: usize = 14 * 1024 * 1024;

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBody {
    data: Value,
    #[serde(default)]
    if_version: Option<u64>,
    #[serde(default)]
    lww: bool,
}

/// The document: `{doc: {path, collection, id, data, version, updated_at}}`;
/// 404 with `path` when it is missing or the caller may not read it.
pub async fn get(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: CallerParts,
    p: DocParams,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    let d = s
        .store_call({
            let doc = doc.clone();
            move |st| st.doc_get(&id, &doc, &who.resolve_for(st, &id)?)
        })
        .await?;
    d.map(|d| Json(json!({"doc": d})))
        .ok_or_else(|| clax_core::CoreError::DocNotFound { path: doc }.into())
}

/// Replaces or creates the document: `{doc, created}`.
pub async fn put(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: CallerParts,
    p: DocParams,
    req: Result<Json<WriteBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let events = s.events.clone();
    let w = s
        .store_call(move |st| {
            let w = st.doc_set(
                &id,
                &doc,
                b.data,
                Pin {
                    if_version: b.if_version,
                    lww: b.lww,
                },
                &who.resolve_for(st, &id)?,
            )?;
            announce(&events, id.as_str(), w.change.clone());
            Ok(w)
        })
        .await?;
    Ok(Json(json!({"doc": w.doc, "created": w.created})))
}

/// Merges into the existing document: `{doc}`.
pub async fn patch(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: CallerParts,
    p: DocParams,
    req: Result<Json<WriteBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let events = s.events.clone();
    let w = s
        .store_call(move |st| {
            let w = st.doc_update(
                &id,
                &doc,
                b.data,
                Pin {
                    if_version: b.if_version,
                    lww: b.lww,
                },
                &who.resolve_for(st, &id)?,
            )?;
            announce(&events, id.as_str(), w.change.clone());
            Ok(w)
        })
        .await?;
    Ok(Json(json!({"doc": w.doc})))
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
    _o: SameOrigin,
    who: CallerParts,
    p: DocParams,
    q: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (aid, doc) = path(p)?;
    let id = parse_id(&aid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_argument", e.body_text()))?;
    let events = s.events.clone();
    let w = s
        .store_call(move |st| {
            let w = st.doc_delete(
                &id,
                &doc,
                Pin {
                    if_version: q.if_version,
                    lww: q.lww,
                },
                &who.resolve_for(st, &id)?,
            )?;
            announce(&events, id.as_str(), w.change.clone());
            Ok(w)
        })
        .await?;
    Ok(Json(json!({"deleted": w.deleted})))
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
/// of `[field, operator, value]` triples.
pub async fn list(
    State(s): State<AppState>,
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
    let (docs, next) = s
        .store_call(move |st| st.doc_query(&id, &query, &who.resolve_for(st, &id)?))
        .await?;
    Ok(Json(json!({"docs": docs, "next_cursor": next})))
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
pub async fn batch(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: CallerParts,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<BatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body_within(req, "the docs batch limit")?;
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
    let events = s.events.clone();
    let written = s
        .store_call(move |st| {
            let ws = st.doc_batch(&id, writes, b.lww, &who.resolve_for(st, &id)?)?;
            announce(
                &events,
                id.as_str(),
                ws.iter().filter_map(|w| w.change.clone()),
            );
            Ok(ws)
        })
        .await?;
    let results: Vec<Value> = written
        .iter()
        .zip(ops)
        .map(|(w, op)| json!({"op": op, "path": w.path, "version": w.doc.as_ref().map(|d| d.version), "deleted": w.deleted}))
        .collect();
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
/// document.
pub async fn str_replace(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: CallerParts,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<StrReplaceBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let events = s.events.clone();
    let w = s
        .store_call(move |st| {
            let r = StrReplace {
                field: b.field,
                old_str: b.old_str,
                new_str: b.new_str,
                replace_all: b.replace_all,
            };
            let w = st.doc_str_replace(
                &id,
                &b.path,
                r,
                Pin {
                    if_version: b.if_version,
                    lww: b.lww,
                },
                &who.resolve_for(st, &id)?,
            )?;
            announce(&events, id.as_str(), w.change.clone());
            Ok(w)
        })
        .await?;
    Ok(Json(json!({"doc": w.doc})))
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

/// `{acquired, version, expires_at, holder}`.
pub async fn acquire(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: CallerParts,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<AcquireBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let events = s.events.clone();
    let a = s
        .store_call(move |st| {
            let (a, change) = st.doc_acquire(
                &id,
                &b.path,
                Acquire {
                    holder: b.holder,
                    ttl_ms: b.ttl_ms,
                    data: b.data,
                },
                &who.resolve_for(st, &id)?,
            )?;
            announce(&events, id.as_str(), change);
            Ok(a)
        })
        .await?;
    Ok(Json(json!(a)))
}
