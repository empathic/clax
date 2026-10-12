//! Documents of the `db` capability (spec §5 `docs`, §9 "db"): JSON objects at
//! document paths per artifact, each with a version. Versions come from one
//! sequence per artifact (`artifacts.doc_seq`) that every change (a write, a
//! delete, a grant that merges data) advances in its own transaction, so a
//! version is never issued twice in an artifact's lifetime: a document that
//! is deleted and created again gets a version above every earlier one, and a
//! pin taken before the delete can only conflict. Every
//! call loads the artifact's declared rules and checks the caller against
//! them: a document the caller may not read behaves as absent, and a write it
//! may not make fails as `DocNotFound`, as for a missing document; a missing
//! artifact is `NotFound`. Leases (`acquire`) live beside the
//! documents and coordinate only callers that use them.
//!
//! Each document changed is recorded, in the change's transaction, as
//! `doc.write` under the writer's `AuditCtx`, with `op` naming the call
//! (`set`, `update`, `delete`, `str_replace`, or `acquire` for a grant that
//! merges data) and the SHA-256 of the stored JSON, never the content. A
//! write that changes nothing (deleting a missing document) records nothing.

use super::Store;
use crate::audit::{AuditCtx, AuditKind, AuditRecord, sha256_hex};
use crate::db::{Caller, Op, Rules, collection_path, doc_path, invalid_argument};
use crate::store::approvals::OWNER;
use crate::visibility::{Author, Visibility, authors_of_doc};
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// Largest serialised document body.
pub const MAX_DOC_BYTES: usize = 256 * 1024;
/// Deepest nesting of a document body.
pub const MAX_DEPTH: usize = 32;
/// Most documents in one artifact's database.
pub const MAX_DOCS: i64 = 5000;
/// Most `where` filters in one query.
pub const MAX_FILTERS: usize = 10;
/// Most values in an `in` or `not-in` filter.
pub const MAX_IN_VALUES: usize = 30;
/// Largest page of a query.
pub const MAX_LIMIT: usize = 1000;
/// Page size when a query names none.
pub const DEFAULT_LIMIT: usize = 100;
/// Most bytes of document bodies (as stored JSON) that an ordered query
/// without a limit may return. 32 MiB is 128 documents at the 256 KiB cap,
/// or thousands of typical ones: far above a collection a page renders,
/// while keeping one response's memory and build time well inside the
/// request timeout. Over it the query fails `resource_exhausted`.
pub const MAX_UNLIMITED_QUERY_BYTES: usize = 32 * 1024 * 1024;
/// Most writes in one batch.
pub const MAX_BATCH: usize = 50;
/// Lease length when none (or 0) is asked for.
pub const DEFAULT_LEASE_MS: u64 = 30_000;
/// Shortest and longest lease; requests are clamped, never refused.
pub const MIN_LEASE_MS: u64 = 1_000;
pub const MAX_LEASE_MS: u64 = 600_000;
/// Most leases in force at once in one artifact.
pub const MAX_LEASES: i64 = 100;
/// `{"__delete__": true}` in an update removes its field.
pub const DELETE_MARKER: &str = "__delete__";

/// One stored document.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Doc {
    pub path: String,
    pub collection: String,
    pub id: String,
    pub data: Value,
    /// Unique within the artifact for its whole lifetime and greater than
    /// every version the artifact issued before (see the module doc).
    pub version: u64,
    pub updated_at: String,
}

/// How a write is pinned: `if_version`, when given, must equal the document's
/// current version (`None` current when it does not exist). Without it a
/// write to an existing document is refused unless `lww` (last writer wins,
/// the page runtime's mode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pin {
    pub if_version: Option<u64>,
    pub lww: bool,
}

/// A change to announce as the `doc` SSE event; `version: None` after a
/// delete; `private_to` is the viewer whose private subtree holds the path;
/// `read_level` is the least level that may read it. `self_read`, for a path
/// in an opened `{self}` subtree, is that subtree's viewer and the least
/// level at which they may read it (which `{self}` rules may lower).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocChange {
    pub path: String,
    pub version: Option<u64>,
    pub private_to: Option<String>,
    pub read_level: crate::db::Level,
    pub self_read: Option<(String, crate::db::Level)>,
}

/// A write's outcome; `change` is `None` when nothing changed. The written
/// document's version (`doc.version`, `change.version`) is the artifact's
/// next sequence number, never a per-document count.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub path: String,
    pub doc: Option<Doc>,
    pub created: bool,
    pub deleted: bool,
    pub change: Option<DocChange>,
    /// The document's authors after the write, read in its transaction
    /// (spec §8.2): every writer since it was created, the owner when none
    /// is stored; empty after a delete.
    pub authors: Vec<Author>,
}

/// A page of a query ([`Store::doc_query_visible`]): the documents, the
/// cursor of the next page of an unordered query, and, for a reader that
/// may not see every document, how many of the collection's documents it
/// withholds (whatever the filter, cursor and limit).
#[derive(Clone, Debug, PartialEq)]
pub struct DocPage {
    pub docs: Vec<Doc>,
    pub next_cursor: Option<String>,
    pub withheld: Option<usize>,
}

pub enum BatchOp {
    Set(Value),
    Update(Value),
    Delete,
}

/// A batch write's operation, with a set body checked and serialised ahead
/// of the transaction.
enum Prepared {
    Set(Result<Body>),
    Update(Value),
    Delete,
}

pub struct BatchWrite {
    pub path: String,
    pub op: BatchOp,
    pub if_version: Option<u64>,
}

pub struct StrReplace {
    pub field: String,
    pub old_str: String,
    pub new_str: String,
    pub replace_all: bool,
}

pub struct Acquire {
    pub holder: String,
    pub ttl_ms: Option<u64>,
    pub data: Option<Value>,
}

/// `acquire`'s result: on a grant, the holder, the lease's expiry, and the
/// document's version (absent when there is no document); when busy, only the
/// expiry of the lease in force.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Acquired {
    pub acquired: bool,
    pub version: Option<u64>,
    pub expires_at: Option<String>,
    pub holder: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    In,
    NotIn,
    ArrayContains,
}

impl FilterOp {
    /// The contract's spellings (`==`, `!=`, `<`, `<=`, `>`, `>=`, `in`,
    /// `not-in`, `array-contains`) and the tools' aliases (`eq`, `ne`, `lt`,
    /// `lte`, `gt`, `gte`).
    pub fn parse(s: &str) -> Result<FilterOp> {
        Ok(match s {
            "==" | "eq" => FilterOp::Eq,
            "!=" | "ne" => FilterOp::Ne,
            "<" | "lt" => FilterOp::Lt,
            "<=" | "lte" => FilterOp::Lte,
            ">" | "gt" => FilterOp::Gt,
            ">=" | "gte" => FilterOp::Gte,
            "in" => FilterOp::In,
            "not-in" => FilterOp::NotIn,
            "array-contains" => FilterOp::ArrayContains,
            _ => return Err(invalid_argument(format!("'{s}' is not a query operator"))),
        })
    }
}

/// One `where` filter on a top-level field.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub value: Value,
}

/// A query of one collection. Without `order_by` results are in document ID
/// order and page with `cursor` (the last ID of the previous page); with it,
/// one page of at most `limit` (every match when `limit` is `None`), missing
/// fields last in either direction.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DocQuery {
    pub collection: String,
    pub filters: Vec<Filter>,
    pub order_by: Option<String>,
    pub descending: bool,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

/// Parses `[[field, operator, value], ...]`.
///
/// # Errors
/// `invalid_argument` for more than [`MAX_FILTERS`] entries, an entry that is
/// not a triple with a non-empty field, an unknown operator, or an `in` /
/// `not-in` value that is not an array of at most [`MAX_IN_VALUES`].
pub fn parse_where(v: &Value) -> Result<Vec<Filter>> {
    let arr = v
        .as_array()
        .ok_or_else(|| invalid_argument("where is an array of [field, operator, value] triples"))?;
    if arr.len() > MAX_FILTERS {
        return Err(invalid_argument(format!(
            "a query has at most {MAX_FILTERS} filters"
        )));
    }
    arr.iter()
        .map(|t| {
            let t = t
                .as_array()
                .filter(|t| t.len() == 3)
                .ok_or_else(|| invalid_argument("each where entry is [field, operator, value]"))?;
            let field = t[0]
                .as_str()
                .filter(|f| !f.is_empty())
                .ok_or_else(|| invalid_argument("a where field is a non-empty string"))?;
            let op = FilterOp::parse(t[1].as_str().unwrap_or(""))?;
            let f = Filter {
                field: field.to_string(),
                op,
                value: t[2].clone(),
            };
            f.check()?;
            Ok(f)
        })
        .collect()
}

fn rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

/// Orders JSON values by type (null, bool, number, string, array, object),
/// then by value; numbers compare numerically, so `1 == 1.0`.
pub fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&y.as_f64().unwrap_or(0.0))
            .unwrap_or(Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Array(x), Value::Array(y)) => x
            .iter()
            .zip(y)
            .map(|(p, q)| compare(p, q))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| x.len().cmp(&y.len())),
        (Value::Object(_), Value::Object(_)) => a.to_string().cmp(&b.to_string()),
        _ => rank(a).cmp(&rank(b)),
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    compare(a, b) == Ordering::Equal
}

impl Filter {
    /// `invalid_argument` when the field is empty or an `in` / `not-in`
    /// value is not an array of at most [`MAX_IN_VALUES`].
    pub fn check(&self) -> Result<()> {
        if self.field.is_empty() {
            return Err(invalid_argument("a where field is a non-empty string"));
        }
        if matches!(self.op, FilterOp::In | FilterOp::NotIn)
            && self
                .value
                .as_array()
                .is_none_or(|a| a.len() > MAX_IN_VALUES)
        {
            return Err(invalid_argument(format!(
                "in and not-in take an array of at most {MAX_IN_VALUES} values"
            )));
        }
        Ok(())
    }

    /// A missing field matches no filter; ranges compare only within one type.
    fn matches(&self, data: &Value) -> bool {
        let Some(v) = data.get(&self.field) else {
            return false;
        };
        let ranged =
            |f: fn(Ordering) -> bool| rank(v) == rank(&self.value) && f(compare(v, &self.value));
        match self.op {
            FilterOp::Eq => equal(v, &self.value),
            FilterOp::Ne => !equal(v, &self.value),
            FilterOp::Lt => ranged(Ordering::is_lt),
            FilterOp::Lte => ranged(Ordering::is_le),
            FilterOp::Gt => ranged(Ordering::is_gt),
            FilterOp::Gte => ranged(Ordering::is_ge),
            FilterOp::In => self
                .value
                .as_array()
                .is_some_and(|a| a.iter().any(|x| equal(v, x))),
            FilterOp::NotIn => self
                .value
                .as_array()
                .is_some_and(|a| !a.iter().any(|x| equal(v, x))),
            FilterOp::ArrayContains => v
                .as_array()
                .is_some_and(|a| a.iter().any(|x| equal(x, &self.value))),
        }
    }
}

fn is_delete_marker(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.len() == 1 && m.get(DELETE_MARKER) == Some(&Value::Bool(true)))
}

/// Checks a body: a JSON object at most [`MAX_DEPTH`] deep and
/// [`MAX_DOC_BYTES`] serialised; delete markers only when `allow_markers`, and
/// never inside arrays.
///
/// # Errors
/// `invalid_argument` naming the problem.
pub fn check_body(data: &Value, allow_markers: bool) -> Result<()> {
    fn walk(v: &Value, depth: usize, allow: bool, in_array: bool) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(invalid_argument(format!(
                "a document is at most {MAX_DEPTH} levels deep"
            )));
        }
        match v {
            _ if is_delete_marker(v) => {
                if allow && !in_array {
                    Ok(())
                } else {
                    Err(invalid_argument(
                        "{\"__delete__\": true} removes a field only in an update, and never inside an array",
                    ))
                }
            }
            Value::Object(m) => m
                .values()
                .try_for_each(|x| walk(x, depth + 1, allow, false)),
            Value::Array(a) => a.iter().try_for_each(|x| walk(x, depth + 1, allow, true)),
            _ => Ok(()),
        }
    }
    if !data.is_object() {
        return Err(invalid_argument("a document body is a JSON object"));
    }
    if is_delete_marker(data) {
        return Err(invalid_argument(
            "{\"__delete__\": true} as the whole body would remove every field; use delete to remove the document",
        ));
    }
    walk(data, 1, allow_markers, false)?;
    if serde_json::to_vec(data)
        .expect("JSON values serialise")
        .len()
        > MAX_DOC_BYTES
    {
        return Err(invalid_argument(format!(
            "a document is at most {MAX_DOC_BYTES} bytes as JSON"
        )));
    }
    Ok(())
}

fn strip_markers(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, x)| !is_delete_marker(x))
                .map(|(k, x)| (k, strip_markers(x)))
                .collect(),
        ),
        other => other,
    }
}

fn merge_into(base: &mut Map<String, Value>, patch: Map<String, Value>) {
    for (k, v) in patch {
        if is_delete_marker(&v) {
            base.remove(&k);
            continue;
        }
        let nested = matches!(
            (base.get(&k), &v),
            (Some(Value::Object(_)), Value::Object(_))
        );
        if nested {
            if let (Some(Value::Object(b)), Value::Object(p)) = (base.get_mut(&k), v) {
                merge_into(b, p);
            }
        } else {
            base.insert(k, strip_markers(v));
        }
    }
}

/// `patch` merged into `base`: nested objects merge recursively, a delete
/// marker removes its field, anything else (arrays included) replaces it.
pub fn merged(base: &Value, patch: Value) -> Value {
    let mut out = base.as_object().cloned().unwrap_or_default();
    if let Value::Object(p) = patch {
        merge_into(&mut out, p);
    }
    Value::Object(out)
}

/// `ms` milliseconds after `now`, as the store writes times.
fn now_plus(now: chrono::DateTime<chrono::Utc>, ms: u64) -> String {
    super::stamp(now + chrono::Duration::milliseconds(ms as i64))
}

/// The live artifact's declared capabilities; `NotFound` when it is missing
/// or deleted.
pub(super) fn caps_in(c: &Connection, id: &ArtifactId) -> Result<Value> {
    let caps: Option<String> = c
        .query_row(
            "SELECT capabilities_json FROM artifacts WHERE id = ?1 AND deleted_at IS NULL AND current_version > 0",
            params![id.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let caps = caps.ok_or(CoreError::NotFound)?;
    serde_json::from_str(&caps).map_err(|_| CoreError::Corrupt {
        artifact_id: id.as_str().to_string(),
        column: "capabilities_json",
        version: None,
    })
}

/// The live artifact's rules; `NotFound` when it is missing or deleted.
fn rules_in(c: &Connection, id: &ArtifactId) -> Result<Rules> {
    Rules::from_capabilities(&caps_in(c, id)?)
}

type Parts = (String, String, String, i64, String);

const SELECT_DOC: &str = "SELECT path, collection, json, version, updated_at FROM docs";

fn row_parts(r: &rusqlite::Row<'_>) -> rusqlite::Result<Parts> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

fn to_doc(id: &ArtifactId, (path, collection, json, version, updated_at): Parts) -> Result<Doc> {
    let data = serde_json::from_str(&json).map_err(|_| CoreError::Corrupt {
        artifact_id: id.as_str().to_string(),
        column: "docs.json",
        version: None,
    })?;
    let doc_id = path.rsplit('/').next().unwrap_or_default().to_string();
    Ok(Doc {
        path,
        collection,
        id: doc_id,
        data,
        version: version as u64,
        updated_at,
    })
}

fn doc_in(c: &Connection, id: &ArtifactId, path: &str) -> Result<Option<Doc>> {
    c.query_row(
        &format!("{SELECT_DOC} WHERE artifact_id = ?1 AND path = ?2"),
        params![id.as_str(), path],
        row_parts,
    )
    .optional()?
    .map(|p| to_doc(id, p))
    .transpose()
}

fn check_pin(path: &str, current: Option<u64>, pin: Pin) -> Result<()> {
    match (current, pin.if_version) {
        (Some(c), Some(v)) if c != v => Err(CoreError::DocConflict {
            path: path.into(),
            current: Some(c),
        }),
        (None, Some(_)) => Err(CoreError::DocConflict {
            path: path.into(),
            current: None,
        }),
        (Some(c), None) if !pin.lww => Err(CoreError::DocPinRequired {
            path: path.into(),
            current: c,
        }),
        _ => Ok(()),
    }
}

/// Advances the artifact's version sequence and returns the new value, which
/// is also above `floor` (the current document's version, guarding rows
/// written outside the sequence).
fn next_version(c: &Connection, id: &ArtifactId, floor: u64) -> Result<u64> {
    let v: i64 = c.query_row(
        "UPDATE artifacts SET doc_seq = MAX(doc_seq, ?2) + 1 WHERE id = ?1 RETURNING doc_seq",
        params![id.as_str(), floor as i64],
        |r| r.get(0),
    )?;
    Ok(v as u64)
}

/// One write: refused as `DocNotFound` unless `rules` let `caller` write `path`;
/// pinned by `pin`; `next` maps the current document to the new body (`None`
/// deletes).
/// A document body that passed [`check_body`], with its JSON text, ready to
/// store. Built before the transaction when the body does not depend on the
/// current document.
struct Body {
    data: Value,
    json: String,
}

impl Body {
    fn new(data: Value) -> Result<Body> {
        check_body(&data, false)?;
        let json = data.to_string();
        Ok(Body { data, json })
    }

    /// `Body::new` of a computed next body, if any.
    fn of(next: Result<Option<Value>>) -> Result<Option<Body>> {
        next?.map(Body::new).transpose()
    }
}

/// Where a write is recorded: `doc.write` with `op`, under `ctx`, in the
/// write's transaction `tx`.
struct Recorder<'a> {
    st: &'a Store,
    tx: &'a rusqlite::Transaction<'a>,
    ctx: &'a AuditCtx,
    op: &'static str,
}

impl Recorder<'_> {
    /// Records the change to `path` (spec §6.1): its new version and the
    /// SHA-256 of its stored JSON (`None`, a delete), never the content.
    fn record(
        &self,
        id: &ArtifactId,
        dp: &crate::db::DocPath,
        version: Option<u64>,
        json: Option<&str>,
        at: &str,
    ) -> Result<()> {
        let mut rec = AuditRecord::new(AuditKind::DocWrite, at)
            .with("collection", dp.collection.as_str())
            .with("doc_id", dp.id.as_str())
            .with("version", version)
            .with("op", self.op)
            .with(
                "sha256",
                json.map(|j| format!("sha256:{}", sha256_hex(j.as_bytes()))),
            );
        rec.ids.artifact = Some(id.as_str().to_string());
        self.st.record_audit(self.tx, self.ctx, rec)?;
        Ok(())
    }
}

fn write_in(
    rc: &Recorder<'_>,
    id: &ArtifactId,
    rules: &Rules,
    caller: &Caller,
    path: &str,
    pin: Pin,
    now: &str,
    next: impl FnOnce(Option<&Doc>) -> Result<Option<Body>>,
) -> Result<Written> {
    let c: &Connection = rc.tx;
    let dp = doc_path(path)?;
    if !rules.allows(&dp.path, Op::Write, caller) {
        return Err(CoreError::DocNotFound { path: dp.path });
    }
    let current = doc_in(c, id, &dp.path)?;
    check_pin(&dp.path, current.as_ref().map(|d| d.version), pin)?;
    let private_to = rules.private_to(&dp.path);
    let read_level = rules.read_level(&dp.path, private_to.as_deref());
    let self_read = rules.opened_self_owner(&dp.path).map(|owner| {
        let level = rules.read_level(&dp.path, Some(&owner));
        (owner, level)
    });
    match next(current.as_ref())? {
        Some(body) => {
            if current.is_none() {
                let n: i64 = c.query_row(
                    "SELECT COUNT(*) FROM docs WHERE artifact_id = ?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )?;
                if n >= MAX_DOCS {
                    return Err(CoreError::invalid(
                        "quota_exceeded",
                        format!(
                            "an artifact's database holds at most {MAX_DOCS} documents; delete some before creating more"
                        ),
                    ));
                }
            }
            let version = next_version(c, id, current.as_ref().map_or(0, |d| d.version))?;
            c.execute(
                "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(artifact_id, path) DO UPDATE SET json = excluded.json, version = excluded.version, updated_at = excluded.updated_at",
                params![id.as_str(), dp.path, dp.collection, body.json, version as i64, now],
            )?;
            rc.record(id, &dp, Some(version), Some(&body.json), now)?;
            add_authors(c, id, &dp.path, caller, current.is_some())?;
            let authors = authors_in(c, id, &dp.path)?;
            let doc = Doc {
                id: dp.path.rsplit('/').next().unwrap_or_default().to_string(),
                path: dp.path.clone(),
                collection: dp.collection.clone(),
                data: body.data,
                version,
                updated_at: now.to_string(),
            };
            Ok(Written {
                path: dp.path.clone(),
                doc: Some(doc),
                created: current.is_none(),
                deleted: false,
                change: Some(DocChange {
                    path: dp.path,
                    version: Some(version),
                    private_to,
                    read_level,
                    self_read,
                }),
                authors,
            })
        }
        None => {
            let n = c.execute(
                "DELETE FROM docs WHERE artifact_id = ?1 AND path = ?2",
                params![id.as_str(), dp.path],
            )?;
            if n > 0 {
                let version = next_version(c, id, current.as_ref().map_or(0, |d| d.version))?;
                rc.record(id, &dp, Some(version), None, now)?;
                // A new document at the path starts a new author set.
                c.execute(
                    "DELETE FROM doc_authors WHERE artifact_id = ?1 AND path = ?2",
                    params![id.as_str(), dp.path],
                )?;
            }
            Ok(Written {
                path: dp.path.clone(),
                doc: None,
                created: false,
                deleted: n > 0,
                change: (n > 0).then_some(DocChange {
                    path: dp.path,
                    version: None,
                    private_to,
                    read_level,
                    self_read,
                }),
                authors: Vec::new(),
            })
        }
    }
}

/// Adds `caller`'s words to the authors of the document at `path` (spec
/// §8.2, M11): the writer, and for a page's write every author of the
/// page (of every file of its current version, now). The set only grows: touching or rewriting
/// someone else's document never makes it the writer's alone. A document
/// with no rows is the owner's, so while every author added is the owner
/// nothing is stored; the first other author makes the owner's authorship
/// of an existing (`existed`) document explicit before joining it.
fn add_authors(
    c: &Connection,
    id: &ArtifactId,
    path: &str,
    caller: &Caller,
    existed: bool,
) -> Result<()> {
    let mut add: Vec<String> = Vec::with_capacity(2);
    if let Some(t) = caller.person.token() {
        add.push(t.to_string());
    }
    if caller.via_page {
        for a in super::artifacts::page_authors_in(c, id.as_str())? {
            if !add.contains(&a) {
                add.push(a);
            }
        }
    }
    let rows: bool = c
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM doc_authors WHERE artifact_id = ?1 AND path = ?2)",
        )?
        .query_row(params![id.as_str(), path], |r| r.get(0))?;
    if !rows {
        if add.iter().all(|a| a == OWNER) {
            return Ok(());
        }
        if existed {
            add.insert(0, OWNER.to_string());
        }
    }
    let mut stmt = c.prepare_cached(
        "INSERT OR IGNORE INTO doc_authors (artifact_id, path, author) VALUES (?1, ?2, ?3)",
    )?;
    for a in add {
        stmt.execute(params![id.as_str(), path, a])?;
    }
    Ok(())
}

/// The authors of the document at `path` (see [`Store::doc_authors`]).
fn authors_in(c: &Connection, id: &ArtifactId, path: &str) -> Result<Vec<Author>> {
    let mut stmt = c.prepare_cached(
        "SELECT author FROM doc_authors WHERE artifact_id = ?1 AND path = ?2 ORDER BY rowid",
    )?;
    let tokens = stmt
        .query_map(params![id.as_str(), path], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(authors_of_doc(&tokens))
}

/// The stored authors of every document of `collection`, by path; a
/// document with none is absent (the owner's).
fn collection_authors(
    c: &Connection,
    id: &ArtifactId,
    collection: &str,
) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = c.prepare_cached(
        "SELECT a.path, a.author FROM doc_authors a
         JOIN docs d ON d.artifact_id = a.artifact_id AND d.path = a.path
         WHERE a.artifact_id = ?1 AND d.collection = ?2 ORDER BY a.rowid",
    )?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for row in stmt.query_map(params![id.as_str(), collection], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (path, author) = row?;
        out.entry(path).or_default().push(author);
    }
    Ok(out)
}

/// `vis` when it may withhold anything; `None` sees every document.
fn filtering(vis: Option<&Visibility>) -> Option<&Visibility> {
    vis.filter(|v| !v.is_all())
}

/// The authors `vis` withholds of the existing document at `path` that
/// `caller` may write; `None` when there is none, the rules hide it (it
/// then answers as missing), or `vis` lets it through.
fn withheld_in(
    c: &Connection,
    id: &ArtifactId,
    rules: &Rules,
    caller: &Caller,
    path: &str,
    vis: Option<&Visibility>,
) -> Result<Option<Vec<Author>>> {
    let Some(vis) = filtering(vis) else {
        return Ok(None);
    };
    if !rules.allows(path, Op::Write, caller) || doc_in(c, id, path)?.is_none() {
        return Ok(None);
    }
    let withheld = vis.withheld_authors(&authors_in(c, id, path)?, id.as_str(), None);
    Ok((!withheld.is_empty()).then_some(withheld))
}

/// [`CoreError::DocWithheld`] for a write whose outcome depends on the
/// document's content (`str_replace`, a merge) when `vis` withholds it:
/// refused before the pin or the content is looked at (spec §7.5, §8.3).
fn refuse_withheld(
    c: &Connection,
    id: &ArtifactId,
    rules: &Rules,
    caller: &Caller,
    path: &str,
    vis: Option<&Visibility>,
) -> Result<()> {
    match withheld_in(c, id, rules, caller, path, vis)? {
        Some(authors) => Err(CoreError::DocWithheld {
            path: path.to_string(),
            authors,
        }),
        None => Ok(()),
    }
}

/// `patch` merged into the document at `path`; `invalid_argument` when it
/// does not exist (an update never creates).
fn update_body(path: &str, cur: Option<&Doc>, patch: Value) -> Result<Option<Value>> {
    let cur = cur
        .ok_or_else(|| invalid_argument(format!("{path} does not exist; use set to create it")))?;
    Ok(Some(merged(&cur.data, patch)))
}

impl Store {
    /// Whether the live artifact's current declaration names `db`;
    /// `NotFound` when the artifact is missing or deleted.
    pub fn doc_declared(&self, id: &ArtifactId) -> Result<bool> {
        self.with_read(|c| Ok(caps_in(c, id)?.get("db").is_some()))
    }

    /// The authors of the document at `path` (spec §8.2): every writer
    /// since it was created, in the order they first wrote; the owner when
    /// none is stored (or the document does not exist).
    pub fn doc_authors(&self, id: &ArtifactId, path: &str) -> Result<Vec<Author>> {
        let dp = doc_path(path)?;
        self.with_read(|c| authors_in(c, id, &dp.path))
    }

    /// [`Store::doc_get`] with the document's authors, read together.
    pub fn doc_get_authored(
        &self,
        id: &ArtifactId,
        path: &str,
        caller: &Caller,
    ) -> Result<Option<(Doc, Vec<Author>)>> {
        let dp = doc_path(path)?;
        self.with_read(|c| {
            let rules = rules_in(c, id)?;
            if !rules.allows(&dp.path, Op::Read, caller) {
                return Ok(None);
            }
            match doc_in(c, id, &dp.path)? {
                Some(d) => Ok(Some((d, authors_in(c, id, &dp.path)?))),
                None => Ok(None),
            }
        })
    }

    /// Each collection holding documents, with its document count, sorted by
    /// path. Collections under a private per-viewer prefix (`data/users/<id>`
    /// and each `{self}` rule's prefix not opened by a rule at it) are left
    /// out, so no viewer's public ID or private count shows.
    ///
    /// # Errors
    /// `NotFound` when the artifact is missing or deleted.
    pub fn doc_collections(&self, id: &ArtifactId) -> Result<Vec<(String, u64)>> {
        self.with_read(|c| {
            let rules = rules_in(c, id)?;
            let mut stmt = c.prepare(
                "SELECT collection, COUNT(*) FROM docs WHERE artifact_id = ?1
                 GROUP BY collection ORDER BY collection",
            )?;
            let rows = stmt.query_map(params![id.as_str()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (collection, n) = row?;
                if rules.private_to(&collection).is_none() {
                    out.push((collection, n as u64));
                }
            }
            Ok(out)
        })
    }

    /// The document at `path`, or `None` when it is absent or `caller` may not read it.
    pub fn doc_get(&self, id: &ArtifactId, path: &str, caller: &Caller) -> Result<Option<Doc>> {
        let dp = doc_path(path)?;
        self.with_read(|c| {
            let rules = rules_in(c, id)?;
            if !rules.allows(&dp.path, Op::Read, caller) {
                return Ok(None);
            }
            doc_in(c, id, &dp.path)
        })
    }

    /// Replaces (or creates) the document with `data`.
    pub fn doc_set(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        data: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        let body = Body::new(data);
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            let rc = Recorder {
                st: self,
                tx,
                ctx,
                op: "set",
            };
            write_in(&rc, id, &rules, caller, path, pin, &self.now(), |_| {
                body.map(Some)
            })
        })
    }

    /// Merges `patch` into the existing document.
    ///
    /// # Errors
    /// `invalid_argument` ("<path> does not exist; use set to create it")
    /// when the document is absent; `DocNotFound` when `caller` may not write it.
    pub fn doc_update(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.doc_update_visible(ctx, id, path, patch, pin, caller, None)
    }

    /// [`Store::doc_update`] for a caller whose reader sees what `vis`
    /// lets through: a merge's outcome (and its size check) depends on the
    /// stored body, so on an existing document it may write but whose
    /// authors `vis` withholds it is [`CoreError::DocWithheld`] before the
    /// pin or the merge (spec §7.5, §8.3).
    ///
    /// # Errors
    /// As above, and [`Store::doc_update`]'s.
    #[allow(clippy::too_many_arguments)]
    pub fn doc_update_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<Written> {
        let dp = doc_path(path)?;
        check_body(&patch, true)?;
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            refuse_withheld(tx, id, &rules, caller, &dp.path, vis)?;
            let rc = Recorder {
                st: self,
                tx,
                ctx,
                op: "update",
            };
            write_in(&rc, id, &rules, caller, path, pin, &self.now(), |cur| {
                Body::of(update_body(path, cur, patch))
            })
        })
    }

    /// Deletes the document; deleting a missing document succeeds with `deleted: false`.
    pub fn doc_delete(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            let rc = Recorder {
                st: self,
                tx,
                ctx,
                op: "delete",
            };
            write_in(&rc, id, &rules, caller, path, pin, &self.now(), |_| {
                Ok(None)
            })
        })
    }

    /// Replaces `old_str` in the top-level string field `field`.
    ///
    /// # Errors
    /// `invalid_argument` when `old_str` is empty or `field` is not a
    /// top-level string; `old_str_not_found`; `old_str_not_unique` when it
    /// occurs more than once without `replace_all`; `DocNotFound` when the
    /// document is absent or `caller` may not write it.
    pub fn doc_str_replace(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.doc_str_replace_visible(ctx, id, path, r, pin, caller, None)
    }

    /// [`Store::doc_str_replace`] for a caller whose reader sees what
    /// `vis` lets through: on an existing document it may write but whose
    /// authors `vis` withholds, [`CoreError::DocWithheld`] before the pin,
    /// the field or `old_str` is looked at, so the answer never tells
    /// whether the text would match (spec §7.5, §8.3).
    ///
    /// # Errors
    /// As above, and [`Store::doc_str_replace`]'s.
    #[allow(clippy::too_many_arguments)]
    pub fn doc_str_replace_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<Written> {
        let dp = doc_path(path)?;
        if r.old_str.is_empty() {
            return Err(invalid_argument("old_str must not be empty"));
        }
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            refuse_withheld(tx, id, &rules, caller, &dp.path, vis)?;
            let rc = Recorder { st: self, tx, ctx, op: "str_replace" };
            write_in(&rc, id, &rules, caller, path, pin, &self.now(), |cur| Body::of((|| {
                let cur = cur.ok_or_else(|| CoreError::DocNotFound { path: path.to_string() })?;
                let mut data = cur.data.clone();
                let Some(Value::String(text)) = data.get_mut(&r.field) else {
                    return Err(invalid_argument(format!("'{}' is not a top-level string field of {}", r.field, cur.path)));
                };
                let n = text.matches(&r.old_str).count();
                if n == 0 {
                    return Err(CoreError::invalid("old_str_not_found", format!("old_str does not occur in '{}'", r.field)));
                }
                if n > 1 && !r.replace_all {
                    return Err(CoreError::invalid(
                        "old_str_not_unique",
                        format!("old_str occurs {n} times in '{}'; pass replace_all or a longer old_str", r.field),
                    ));
                }
                *text = if r.replace_all { text.replace(&r.old_str, &r.new_str) } else { text.replacen(&r.old_str, &r.new_str, 1) };
                Ok(Some(data))
            })()))
        })
    }

    /// Runs `q` over the documents `caller` may read, returning the page and,
    /// for an unordered query with more results, the cursor for the next page.
    /// An unordered query without a limit returns [`DEFAULT_LIMIT`] documents a
    /// page; an ordered one without a limit returns every match (at most
    /// [`MAX_DOCS`], the artifact's quota), or fails `resource_exhausted`
    /// when their bodies pass [`MAX_UNLIMITED_QUERY_BYTES`].
    pub fn doc_query(
        &self,
        id: &ArtifactId,
        q: &DocQuery,
        caller: &Caller,
    ) -> Result<(Vec<Doc>, Option<String>)> {
        let page = self.doc_query_visible(id, q, caller, None)?;
        Ok((page.docs, page.next_cursor))
    }

    /// [`Store::doc_query`] for a caller whose reader sees what `vis`
    /// lets through (spec §8.3): a document whose authors `vis` withholds
    /// is never matched, ordered, sized or paged over, as if absent; the
    /// page counts the collection's documents the caller may read but
    /// `vis` withholds, whatever the filter, cursor and limit. Without a
    /// view that withholds anything, `withheld` is `None`.
    ///
    /// # Errors
    /// [`Store::doc_query`]'s.
    pub fn doc_query_visible(
        &self,
        id: &ArtifactId,
        q: &DocQuery,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<DocPage> {
        self.doc_query_within(id, q, caller, vis, MAX_UNLIMITED_QUERY_BYTES)
    }

    /// [`Store::doc_query_visible`] with `budget` bytes for an ordered
    /// query without a limit.
    fn doc_query_within(
        &self,
        id: &ArtifactId,
        q: &DocQuery,
        caller: &Caller,
        vis: Option<&Visibility>,
        budget: usize,
    ) -> Result<DocPage> {
        let vis = filtering(vis);
        let collection = collection_path(&q.collection)?;
        let limit = match (q.limit, &q.order_by) {
            (Some(l), _) => l,
            (None, Some(_)) => usize::MAX,
            (None, None) => DEFAULT_LIMIT,
        };
        if q.limit.is_some_and(|l| !(1..=MAX_LIMIT).contains(&l)) {
            return Err(invalid_argument(format!("limit is 1 to {MAX_LIMIT}")));
        }
        if q.filters.len() > MAX_FILTERS {
            return Err(invalid_argument(format!(
                "a query has at most {MAX_FILTERS} filters"
            )));
        }
        q.filters.iter().try_for_each(Filter::check)?;
        if q.order_by.is_some() && q.cursor.is_some() {
            return Err(invalid_argument(
                "a query with order_by is a single page; drop cursor",
            ));
        }
        self.with_read(|c| {
            let rules = rules_in(c, id)?;
            let after = q.cursor.as_ref().map_or(String::new(), |cur| format!("{collection}/{cur}"));
            // A reader that may be withheld documents counts them over the
            // whole collection, so the scan starts at its first path.
            let from = if vis.is_some() { "" } else { after.as_str() };
            let authors = match vis {
                Some(_) => collection_authors(c, id, &collection)?,
                None => HashMap::new(),
            };
            let mut stmt = c.prepare(&format!("{SELECT_DOC} WHERE artifact_id = ?1 AND collection = ?2 AND path > ?3 ORDER BY path"))?;
            let rows = stmt
                .query_map(params![id.as_str(), collection, from], row_parts)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut docs = Vec::new();
            let mut bytes = 0;
            let mut withheld = 0;
            for parts in rows {
                if !rules.allows(&parts.0, Op::Read, caller) {
                    continue;
                }
                if let Some(vis) = vis {
                    let tokens = authors.get(&parts.0).map_or(&[][..], Vec::as_slice);
                    if !vis.sees(&authors_of_doc(tokens), id.as_str(), None) {
                        withheld += 1;
                        continue;
                    }
                    if parts.0 <= after {
                        continue;
                    }
                }
                let size = parts.2.len();
                let d = to_doc(id, parts)?;
                if q.filters.iter().all(|f| f.matches(&d.data)) {
                    bytes += size;
                    docs.push(d);
                }
            }
            if q.order_by.is_some() && q.limit.is_none() && bytes > budget {
                return Err(CoreError::invalid(
                    "resource_exhausted",
                    format!(
                        "the matching documents hold {bytes} bytes, over the {budget} an ordered query without a limit may return; add a limit or narrow the query"
                    ),
                ));
            }
            if let Some(field) = &q.order_by {
                docs.sort_by(|a, b| {
                    let o = match (a.data.get(field), b.data.get(field)) {
                        (Some(x), Some(y)) if q.descending => compare(y, x),
                        (Some(x), Some(y)) => compare(x, y),
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        (None, None) => Ordering::Equal,
                    };
                    o.then_with(|| a.id.cmp(&b.id))
                });
                docs.truncate(limit);
                return Ok(DocPage { docs, next_cursor: None, withheld: vis.map(|_| withheld) });
            }
            let next_cursor = (docs.len() > limit).then(|| docs[limit - 1].id.clone());
            docs.truncate(limit);
            Ok(DocPage { docs, next_cursor, withheld: vis.map(|_| withheld) })
        })
    }

    /// Applies `writes` in order in one transaction: all land or none do. Each
    /// document may appear once; `lww` applies to every entry.
    ///
    /// # Errors
    /// `invalid_argument` for an empty batch or one over [`MAX_BATCH`];
    /// otherwise a failing write's error wrapped in [`CoreError::InBatch`]
    /// with its index and path. Paths are checked (all of them, in order)
    /// before duplicates and bodies.
    pub fn doc_batch(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
    ) -> Result<Vec<Written>> {
        self.doc_batch_visible(ctx, id, writes, lww, caller, None)
    }

    /// [`Store::doc_batch`] for a caller whose reader sees what `vis` lets
    /// through: an `update` of a document `vis` withholds fails at its
    /// place as [`CoreError::DocWithheld`] before its pin or merge, as
    /// [`Store::doc_update_visible`]; `set` and `delete` stay blind.
    ///
    /// # Errors
    /// As above, and [`Store::doc_batch`]'s.
    pub fn doc_batch_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<Vec<Written>> {
        if writes.is_empty() || writes.len() > MAX_BATCH {
            return Err(invalid_argument(format!(
                "a batch holds 1 to {MAX_BATCH} writes"
            )));
        }
        let in_batch = |op: usize, path: &str, error: CoreError| CoreError::InBatch {
            op,
            path: path.to_string(),
            error: Box::new(error),
        };
        for (i, w) in writes.iter().enumerate() {
            doc_path(&w.path).map_err(|e| in_batch(i, &w.path, e))?;
        }
        let mut seen = HashSet::new();
        for (i, w) in writes.iter().enumerate() {
            if !seen.insert(w.path.as_str()) {
                return Err(in_batch(
                    i,
                    &w.path,
                    invalid_argument(format!(
                        "a batch addresses each document at most once; '{}' appears twice",
                        w.path
                    )),
                ));
            }
            if let BatchOp::Update(p) = &w.op {
                check_body(p, true).map_err(|e| in_batch(i, &w.path, e))?;
            }
        }
        // Set bodies are checked and serialised before the transaction; a bad
        // one still fails at its own place in the batch.
        let writes: Vec<(String, Option<u64>, Prepared)> = writes
            .into_iter()
            .map(|w| {
                let op = match w.op {
                    BatchOp::Set(data) => Prepared::Set(Body::new(data)),
                    BatchOp::Update(patch) => Prepared::Update(patch),
                    BatchOp::Delete => Prepared::Delete,
                };
                (w.path, w.if_version, op)
            })
            .collect();
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            writes
                .into_iter()
                .enumerate()
                .map(|(i, (path, if_version, op))| {
                    let pin = Pin { if_version, lww };
                    let path = path.as_str();
                    let rc = |op| Recorder {
                        st: self,
                        tx,
                        ctx,
                        op,
                    };
                    match op {
                        Prepared::Set(body) => write_in(
                            &rc("set"),
                            id,
                            &rules,
                            caller,
                            path,
                            pin,
                            &self.now(),
                            |_| body.map(Some),
                        ),
                        Prepared::Update(patch) => {
                            refuse_withheld(tx, id, &rules, caller, path, vis).and_then(|()| {
                                write_in(
                                    &rc("update"),
                                    id,
                                    &rules,
                                    caller,
                                    path,
                                    pin,
                                    &self.now(),
                                    |cur| Body::of(update_body(path, cur, patch)),
                                )
                            })
                        }
                        Prepared::Delete => write_in(
                            &rc("delete"),
                            id,
                            &rules,
                            caller,
                            path,
                            pin,
                            &self.now(),
                            |_| Ok(None),
                        ),
                    }
                    .map_err(|e| in_batch(i, path, e))
                })
                .collect()
        })
    }

    /// Grants `a.holder` the lease on `path` unless another holder's lease is
    /// still in force; a grant merges `a.data` into the document (creating it).
    /// Needs write access to `path`. Lapsed leases of the artifact are pruned
    /// first; a new lease beyond [`MAX_LEASES`] in force is
    /// `resource_exhausted`.
    pub fn doc_acquire(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        a: Acquire,
        caller: &Caller,
    ) -> Result<(Acquired, Option<DocChange>)> {
        self.doc_acquire_visible(ctx, id, path, a, caller, None)
    }

    /// [`Store::doc_acquire`] for a caller whose reader sees what `vis`
    /// lets through, on a document `vis` withholds: with `data` (a merge)
    /// [`CoreError::DocWithheld`] before anything is looked at; without, a
    /// lease in force is busy whatever its holder, so the answer never
    /// confirms a guessed holder and never takes another's lease over.
    ///
    /// # Errors
    /// As above, and [`Store::doc_acquire`]'s.
    pub fn doc_acquire_visible(
        &self,
        ctx: &AuditCtx,
        id: &ArtifactId,
        path: &str,
        a: Acquire,
        caller: &Caller,
        vis: Option<&Visibility>,
    ) -> Result<(Acquired, Option<DocChange>)> {
        let dp = doc_path(path)?;
        if a.holder.is_empty() || a.holder.chars().count() > 200 {
            return Err(invalid_argument("holder is 1 to 200 characters"));
        }
        if let Some(d) = &a.data {
            check_body(d, true)?;
        }
        let ttl = match a.ttl_ms {
            None | Some(0) => DEFAULT_LEASE_MS,
            Some(t) => t.clamp(MIN_LEASE_MS, MAX_LEASE_MS),
        };
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            if !rules.allows(&dp.path, Op::Write, caller) {
                return Err(CoreError::DocNotFound { path: dp.path.clone() });
            }
            let withheld = withheld_in(tx, id, &rules, caller, &dp.path, vis)?;
            if let Some(authors) = &withheld
                && a.data.is_some()
            {
                return Err(CoreError::DocWithheld { path: dp.path.clone(), authors: authors.clone() });
            }
            let now = self.now();
            tx.execute(
                "DELETE FROM leases WHERE artifact_id = ?1 AND expires_at <= ?2",
                params![id.as_str(), now],
            )?;
            let held: Option<(String, String)> = tx
                .query_row(
                    "SELECT holder, expires_at FROM leases WHERE artifact_id = ?1 AND path = ?2",
                    params![id.as_str(), dp.path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((holder, expires)) = &held
                && (*holder != a.holder || withheld.is_some())
                && *expires > now
            {
                return Ok((Acquired { acquired: false, version: None, expires_at: Some(expires.clone()), holder: None }, None));
            }
            if held.is_none() {
                let active: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM leases WHERE artifact_id = ?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )?;
                if active >= MAX_LEASES {
                    return Err(CoreError::invalid(
                        "resource_exhausted",
                        format!(
                            "an artifact has at most {MAX_LEASES} leases in force; let some lapse before acquiring more"
                        ),
                    ));
                }
            }
            let expires = now_plus(self.now_utc(), ttl);
            tx.execute(
                "INSERT INTO leases (artifact_id, path, holder, expires_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(artifact_id, path) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at",
                params![id.as_str(), dp.path, a.holder, expires],
            )?;
            let empty = Value::Object(Map::new());
            let (version, change) = match a.data {
                Some(patch) => {
                    let rc = Recorder { st: self, tx, ctx, op: "acquire" };
                    let w = write_in(&rc, id, &rules, caller, &dp.path, Pin { if_version: None, lww: true }, &self.now(), |cur| {
                        Body::new(merged(cur.map_or(&empty, |d| &d.data), patch)).map(Some)
                    })?;
                    (w.doc.map(|d| d.version), w.change)
                }
                None => (doc_in(tx, id, &dp.path)?.map(|d| d.version), None),
            };
            Ok((Acquired { acquired: true, version, expires_at: Some(expires), holder: Some(a.holder) }, change))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Level;
    use crate::store::test_util::DAEMON;
    use crate::store::test_util::{artifact_with_caps, store};
    use serde_json::json;

    const A: &str = "u_00000000000000000000aa";
    const B: &str = "u_00000000000000000000bb";

    fn who(level: Level, viewer: Option<&str>) -> Caller {
        Caller::new(level, viewer.map(str::to_string))
    }
    fn admin() -> Caller {
        who(Level::Admin, None)
    }
    fn page() -> Pin {
        Pin {
            if_version: None,
            lww: true,
        }
    }
    fn pinned(v: u64) -> Pin {
        Pin {
            if_version: Some(v),
            lww: false,
        }
    }

    #[test]
    fn set_get_update_delete_round_trip() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({"db": {}}));
        let w = st
            .doc_set(
                DAEMON,
                &id,
                "tasks/t1",
                json!({"title": "Ship", "meta": {"a": 1}}),
                Pin::default(),
                &admin(),
            )
            .unwrap();
        assert!(w.created);
        assert_eq!(w.doc.as_ref().unwrap().version, 1);
        assert_eq!(
            w.change,
            Some(DocChange {
                path: "tasks/t1".into(),
                version: Some(1),
                private_to: None,
                read_level: Level::View,
                self_read: None,
            })
        );
        let d = st.doc_get(&id, "tasks/t1", &admin()).unwrap().unwrap();
        assert_eq!((d.id.as_str(), d.collection.as_str()), ("t1", "tasks"));
        let w = st
            .doc_update(
                DAEMON,
                &id,
                "tasks/t1",
                json!({"meta": {"b": 2}}),
                pinned(1),
                &admin(),
            )
            .unwrap();
        assert_eq!(
            w.doc.unwrap().data,
            json!({"title": "Ship", "meta": {"a": 1, "b": 2}})
        );
        let w = st
            .doc_delete(DAEMON, &id, "tasks/t1", pinned(2), &admin())
            .unwrap();
        assert!(w.deleted);
        assert_eq!(w.change.unwrap().version, None);
        assert_eq!(st.doc_get(&id, "tasks/t1", &admin()).unwrap(), None);
        assert!(
            !st.doc_delete(DAEMON, &id, "tasks/t1", Pin::default(), &admin())
                .unwrap()
                .deleted,
            "deleting nothing succeeds"
        );
    }

    #[test]
    fn existing_documents_need_a_pin_unless_last_writer_wins() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({"db": {}}));
        st.doc_set(
            DAEMON,
            &id,
            "tasks/t1",
            json!({"n": 1}),
            Pin::default(),
            &admin(),
        )
        .unwrap();
        match st.doc_set(
            DAEMON,
            &id,
            "tasks/t1",
            json!({"n": 2}),
            Pin::default(),
            &admin(),
        ) {
            Err(CoreError::DocPinRequired { path, current }) => {
                assert_eq!((path.as_str(), current), ("tasks/t1", 1))
            }
            other => panic!("{other:?}"),
        }
        match st.doc_set(
            DAEMON,
            &id,
            "tasks/t1",
            json!({"n": 2}),
            pinned(7),
            &admin(),
        ) {
            Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, Some(1)),
            other => panic!("{other:?}"),
        }
        match st.doc_set(DAEMON, &id, "tasks/none", json!({}), pinned(1), &admin()) {
            Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, None),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            st.doc_set(DAEMON, &id, "tasks/t1", json!({"n": 3}), page(), &admin())
                .unwrap()
                .doc
                .unwrap()
                .version,
            2
        );
    }

    #[test]
    fn update_merges_nested_objects_and_removes_marked_fields() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        st.doc_set(
            DAEMON,
            &id,
            "c/d",
            json!({"a": {"x": 1, "y": 2}, "b": [1, 2], "c": "keep"}),
            page(),
            &admin(),
        )
        .unwrap();
        let d = st.doc_update(DAEMON, &id, "c/d", json!({"a": {"y": {"__delete__": true}, "z": 3}, "b": [3], "n": {"m": {"__delete__": true}}}), page(), &admin())
            .unwrap().doc.unwrap();
        assert_eq!(
            d.data,
            json!({"a": {"x": 1, "z": 3}, "b": [3], "c": "keep", "n": {}})
        );
        match st.doc_update(DAEMON, &id, "c/missing", json!({"a": 1}), page(), &admin()) {
            Err(CoreError::Invalid {
                code: "invalid_argument",
                message,
            }) => assert_eq!(message, "c/missing does not exist; use set to create it"),
            other => panic!("update needs an existing document: {other:?}"),
        }
        let batch = vec![BatchWrite {
            path: "c/missing".into(),
            op: BatchOp::Update(json!({"a": 1})),
            if_version: None,
        }];
        match st.doc_batch(DAEMON, &id, batch, true, &admin()) {
            Err(CoreError::InBatch { op: 0, error, .. }) => assert!(
                matches!(
                    *error,
                    CoreError::Invalid {
                        code: "invalid_argument",
                        ..
                    }
                ),
                "a batched update needs an existing document too: {error:?}"
            ),
            other => panic!("{other:?}"),
        }
        match st.doc_update(
            DAEMON,
            &id,
            "c/d",
            json!({"__delete__": true}),
            page(),
            &admin(),
        ) {
            Err(CoreError::Invalid {
                code: "invalid_argument",
                message,
            }) => assert!(message.contains("use delete"), "{message}"),
            other => panic!("a whole-body marker is refused: {other:?}"),
        }
        assert_eq!(st.doc_get(&id, "c/missing", &admin()).unwrap(), None);
        assert!(
            matches!(
                st.doc_update(
                    DAEMON,
                    &id,
                    "c/d",
                    json!({"b": [{"__delete__": true}]}),
                    page(),
                    &admin()
                ),
                Err(CoreError::Invalid {
                    code: "invalid_argument",
                    ..
                })
            ),
            "no markers inside arrays"
        );
    }

    #[test]
    fn set_refuses_markers_non_objects_deep_and_oversized_bodies() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let mut deep = json!(1);
        for _ in 0..MAX_DEPTH {
            deep = json!({"x": deep});
        }
        for body in [
            json!([1]),
            json!("s"),
            json!({"a": {"__delete__": true}}),
            deep,
            json!({"s": "x".repeat(MAX_DOC_BYTES)}),
        ] {
            assert!(matches!(
                st.doc_set(DAEMON, &id, "c/d", body, page(), &admin()),
                Err(CoreError::Invalid {
                    code: "invalid_argument",
                    ..
                })
            ));
        }
        assert!(
            matches!(
                st.doc_set(DAEMON, &id, "c", json!({}), page(), &admin()),
                Err(CoreError::Invalid {
                    code: "invalid_argument",
                    ..
                })
            ),
            "odd path"
        );
    }

    #[test]
    fn refused_writes_read_as_missing_and_view_writes_nothing() {
        let (_d, st) = store();
        let id = artifact_with_caps(
            &st,
            json!({"db": {"rules": [{"path": "", "write": "admin"}]}}),
        );
        assert!(matches!(
            st.doc_set(
                DAEMON,
                &id,
                "t/1",
                json!({}),
                page(),
                &who(Level::Interact, Some(A))
            ),
            Err(CoreError::DocNotFound { .. })
        ));
        st.doc_set(DAEMON, &id, "t/1", json!({}), page(), &admin())
            .unwrap();
        assert!(
            st.doc_get(&id, "t/1", &who(Level::View, None))
                .unwrap()
                .is_some(),
            "view reads shared documents"
        );
        let open = artifact_with_caps(&st, json!({}));
        assert!(matches!(
            st.doc_set(
                DAEMON,
                &open,
                "t/1",
                json!({}),
                page(),
                &who(Level::View, Some(A))
            ),
            Err(CoreError::DocNotFound { .. })
        ));
    }

    #[test]
    fn private_subtrees_are_invisible_to_siblings_and_the_owner() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({"db": {}}));
        let mine = format!("data/users/{A}/profile");
        let w = st
            .doc_set(
                DAEMON,
                &id,
                &mine,
                json!({"pick": 3}),
                page(),
                &who(Level::Interact, Some(A)),
            )
            .unwrap();
        assert_eq!(w.change.unwrap().private_to.as_deref(), Some(A));
        for c in [
            who(Level::Interact, Some(B)),
            who(Level::Admin, None),
            who(Level::Owner, Some(B)),
        ] {
            assert_eq!(st.doc_get(&id, &mine, &c).unwrap(), None, "{c:?}");
            let q = DocQuery {
                collection: format!("data/users/{A}"),
                ..Default::default()
            };
            assert!(st.doc_query(&id, &q, &c).unwrap().0.is_empty(), "{c:?}");
            assert!(
                matches!(
                    st.doc_set(DAEMON, &id, &mine, json!({}), page(), &c),
                    Err(CoreError::DocNotFound { .. })
                ),
                "{c:?}"
            );
        }
        let q = DocQuery {
            collection: format!("data/users/{A}"),
            ..Default::default()
        };
        assert_eq!(
            st.doc_query(&id, &q, &who(Level::Interact, Some(A)))
                .unwrap()
                .0
                .len(),
            1
        );
    }

    #[test]
    fn an_ordered_query_without_a_limit_returns_every_match() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let n = MAX_LIMIT + 5;
        for start in (0..n).step_by(MAX_BATCH) {
            let chunk = (start..n.min(start + MAX_BATCH))
                .map(|i| BatchWrite {
                    path: format!("log/e{i:05}"),
                    op: BatchOp::Set(json!({"at": n - i})),
                    if_version: None,
                })
                .collect();
            st.doc_batch(DAEMON, &id, chunk, true, &admin()).unwrap();
        }
        let q = DocQuery {
            collection: "log".into(),
            order_by: Some("at".into()),
            ..Default::default()
        };
        let (docs, next) = st.doc_query(&id, &q, &admin()).unwrap();
        assert_eq!((docs.len(), next), (n, None));
        assert_eq!(docs[0].data["at"], 1);
        // Unordered, the page is the default size and a cursor follows.
        let (docs, next) = st
            .doc_query(
                &id,
                &DocQuery {
                    order_by: None,
                    ..q
                },
                &admin(),
            )
            .unwrap();
        assert_eq!((docs.len(), next.is_some()), (DEFAULT_LIMIT, true));
    }

    #[test]
    fn an_ordered_query_without_a_limit_has_a_byte_budget() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        // Bodies just under the document cap; enough of them to pass the budget.
        let big = "x".repeat(MAX_DOC_BYTES - 64);
        let n = MAX_UNLIMITED_QUERY_BYTES / (MAX_DOC_BYTES - 64) + 1;
        for i in 0..n {
            st.doc_set(
                DAEMON,
                &id,
                &format!("big/d{i:04}"),
                json!({"at": i, "s": big}),
                page(),
                &admin(),
            )
            .unwrap();
        }
        let q = DocQuery {
            collection: "big".into(),
            order_by: Some("at".into()),
            ..Default::default()
        };
        let e = st.doc_query(&id, &q, &admin()).unwrap_err();
        assert!(
            matches!(
                &e,
                CoreError::Invalid {
                    code: "resource_exhausted",
                    ..
                }
            ),
            "{e:?}"
        );
        // With a limit, or narrowed below the budget, it answers.
        let limited = DocQuery {
            limit: Some(3),
            ..q.clone()
        };
        assert_eq!(st.doc_query(&id, &limited, &admin()).unwrap().0.len(), 3);
        let narrowed = DocQuery {
            filters: parse_where(&json!([["at", "<", 10]])).unwrap(),
            ..q
        };
        assert_eq!(st.doc_query(&id, &narrowed, &admin()).unwrap().0.len(), 10);
    }

    #[test]
    fn query_filters_orders_limits_and_pages() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        for (k, v) in [
            ("a", json!({"n": 3, "tags": ["x"], "s": "open"})),
            ("b", json!({"n": 1, "s": "done"})),
            ("c", json!({"n": 2, "tags": ["x", "y"], "s": "open"})),
            ("d", json!({"s": "open"})),
        ] {
            st.doc_set(DAEMON, &id, &format!("tasks/{k}"), v, page(), &admin())
                .unwrap();
        }
        st.doc_set(
            DAEMON,
            &id,
            "tasks/a/sub/z",
            json!({"n": 0}),
            page(),
            &admin(),
        )
        .unwrap();
        let ids = |q: DocQuery| {
            st.doc_query(&id, &q, &admin())
                .unwrap()
                .0
                .into_iter()
                .map(|d| d.id)
                .collect::<Vec<_>>()
        };
        let base = || DocQuery {
            collection: "tasks".into(),
            ..Default::default()
        };
        assert_eq!(
            ids(base()),
            ["a", "b", "c", "d"],
            "document ID order; nested collections excluded"
        );
        assert_eq!(
            ids(DocQuery {
                filters: parse_where(&json!([["s", "==", "open"], ["n", ">=", 2]])).unwrap(),
                ..base()
            }),
            ["a", "c"]
        );
        assert_eq!(
            ids(DocQuery {
                filters: parse_where(&json!([["tags", "array-contains", "y"]])).unwrap(),
                ..base()
            }),
            ["c"]
        );
        assert_eq!(
            ids(DocQuery {
                filters: parse_where(&json!([["n", "in", [1, 3]]])).unwrap(),
                ..base()
            }),
            ["a", "b"]
        );
        assert_eq!(
            ids(DocQuery {
                filters: parse_where(&json!([["n", "not-in", [1]]])).unwrap(),
                ..base()
            }),
            ["a", "c"],
            "a missing field matches nothing"
        );
        assert_eq!(
            ids(DocQuery {
                filters: parse_where(&json!([["n", "<", "z"]])).unwrap(),
                ..base()
            }),
            Vec::<String>::new(),
            "ranges compare within one type"
        );
        assert_eq!(
            ids(DocQuery {
                order_by: Some("n".into()),
                ..base()
            }),
            ["b", "c", "a", "d"],
            "missing sorts last"
        );
        assert_eq!(
            ids(DocQuery {
                order_by: Some("n".into()),
                descending: true,
                limit: Some(2),
                ..base()
            }),
            ["a", "c"]
        );
        let (page1, next) = st
            .doc_query(
                &id,
                &DocQuery {
                    limit: Some(3),
                    ..base()
                },
                &admin(),
            )
            .unwrap();
        assert_eq!((page1.len(), next.as_deref()), (3, Some("c")));
        let (page2, next) = st
            .doc_query(
                &id,
                &DocQuery {
                    limit: Some(3),
                    cursor: Some("c".into()),
                    ..base()
                },
                &admin(),
            )
            .unwrap();
        assert_eq!((page2.len(), next), (1, None));
        for bad in [
            json!([["n", "~", 1]]),
            json!([["n", "in", 1]]),
            json!([["n", "=="]]),
            json!(vec![json!(["n", "==", 1]); MAX_FILTERS + 1]),
        ] {
            assert!(parse_where(&bad).is_err(), "{bad}");
        }
        assert!(
            st.doc_query(
                &id,
                &DocQuery {
                    limit: Some(MAX_LIMIT + 1),
                    ..base()
                },
                &admin()
            )
            .is_err()
        );
        assert!(
            st.doc_query(
                &id,
                &DocQuery {
                    order_by: Some("n".into()),
                    cursor: Some("a".into()),
                    ..base()
                },
                &admin()
            )
            .is_err()
        );
    }

    #[test]
    fn batch_is_atomic_and_names_the_failing_path() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        st.doc_set(DAEMON, &id, "t/1", json!({"n": 1}), page(), &admin())
            .unwrap();
        let writes = vec![
            BatchWrite {
                path: "t/2".into(),
                op: BatchOp::Set(json!({"n": 2})),
                if_version: None,
            },
            BatchWrite {
                path: "t/1".into(),
                op: BatchOp::Update(json!({"n": 9})),
                if_version: Some(5),
            },
        ];
        match st.doc_batch(DAEMON, &id, writes, false, &admin()) {
            Err(CoreError::InBatch { op, path, error }) => {
                assert_eq!((op, path.as_str()), (1, "t/1"));
                assert!(
                    matches!(
                        *error,
                        CoreError::DocConflict {
                            current: Some(1),
                            ..
                        }
                    ),
                    "{error:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            st.doc_get(&id, "t/2", &admin()).unwrap(),
            None,
            "nothing landed"
        );
        let ok = st
            .doc_batch(
                DAEMON,
                &id,
                vec![
                    BatchWrite {
                        path: "t/2".into(),
                        op: BatchOp::Set(json!({"n": 2})),
                        if_version: None,
                    },
                    BatchWrite {
                        path: "t/1".into(),
                        op: BatchOp::Delete,
                        if_version: Some(1),
                    },
                ],
                false,
                &admin(),
            )
            .unwrap();
        assert_eq!((ok[0].created, ok[1].deleted), (true, true));
        let dup = vec![
            BatchWrite {
                path: "t/3".into(),
                op: BatchOp::Delete,
                if_version: None,
            },
            BatchWrite {
                path: "t/3".into(),
                op: BatchOp::Delete,
                if_version: None,
            },
        ];
        match st.doc_batch(DAEMON, &id, dup, false, &admin()) {
            Err(CoreError::InBatch { op: 1, path, error }) => {
                assert_eq!(path, "t/3");
                assert!(matches!(
                    *error,
                    CoreError::Invalid {
                        code: "invalid_argument",
                        ..
                    }
                ));
            }
            other => panic!("{other:?}"),
        }
        let many = (0..=MAX_BATCH)
            .map(|i| BatchWrite {
                path: format!("t/x{i}"),
                op: BatchOp::Delete,
                if_version: None,
            })
            .collect();
        assert!(matches!(
            st.doc_batch(DAEMON, &id, many, false, &admin()),
            Err(CoreError::Invalid {
                code: "invalid_argument",
                ..
            })
        ));
    }

    #[test]
    fn str_replace_requires_one_occurrence_unless_replace_all() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        st.doc_set(
            DAEMON,
            &id,
            "p/1",
            json!({"html": "a-b-a", "n": 1}),
            page(),
            &admin(),
        )
        .unwrap();
        let r = |old: &str, all: bool, field: &str| StrReplace {
            field: field.into(),
            old_str: old.into(),
            new_str: "Z".into(),
            replace_all: all,
        };
        assert!(matches!(
            st.doc_str_replace(
                DAEMON,
                &id,
                "p/1",
                r("a", false, "html"),
                pinned(1),
                &admin()
            ),
            Err(CoreError::Invalid {
                code: "old_str_not_unique",
                ..
            })
        ));
        assert!(matches!(
            st.doc_str_replace(
                DAEMON,
                &id,
                "p/1",
                r("q", false, "html"),
                pinned(1),
                &admin()
            ),
            Err(CoreError::Invalid {
                code: "old_str_not_found",
                ..
            })
        ));
        assert!(matches!(
            st.doc_str_replace(DAEMON, &id, "p/1", r("a", false, "n"), pinned(1), &admin()),
            Err(CoreError::Invalid {
                code: "invalid_argument",
                ..
            })
        ));
        let d = st
            .doc_str_replace(
                DAEMON,
                &id,
                "p/1",
                r("b", false, "html"),
                pinned(1),
                &admin(),
            )
            .unwrap()
            .doc
            .unwrap();
        assert_eq!((d.data["html"].as_str(), d.version), (Some("a-Z-a"), 2));
        let d = st
            .doc_str_replace(
                DAEMON,
                &id,
                "p/1",
                r("a", true, "html"),
                pinned(2),
                &admin(),
            )
            .unwrap()
            .doc
            .unwrap();
        assert_eq!(d.data["html"], "Z-Z-Z");
    }

    #[test]
    fn creating_past_the_document_cap_is_a_quota_error() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        st.with_tx(|tx| {
            for i in 0..MAX_DOCS {
                tx.execute(
                    "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at) VALUES (?1, ?2, 'f', '{}', 1, 'x')",
                    params![id.as_str(), format!("f/{i}")],
                )?;
            }
            Ok(())
        }).unwrap();
        assert!(matches!(
            st.doc_set(DAEMON, &id, "f/new", json!({}), page(), &admin()),
            Err(CoreError::Invalid {
                code: "quota_exceeded",
                ..
            })
        ));
        assert!(
            st.doc_set(DAEMON, &id, "f/0", json!({"n": 1}), page(), &admin())
                .is_ok(),
            "existing documents stay writable"
        );
    }

    #[test]
    fn acquire_grants_one_holder_until_the_lease_lapses() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let acq = |holder: &str, data: Option<Value>| {
            st.doc_acquire(
                DAEMON,
                &id,
                "locks/editor",
                Acquire {
                    holder: holder.into(),
                    ttl_ms: Some(5),
                    data,
                },
                &admin(),
            )
            .unwrap()
        };
        let (a, change) = acq("tab-a", Some(json!({"by": "tab-a"})));
        assert!(
            a.acquired
                && a.holder.as_deref() == Some("tab-a")
                && a.version == Some(1)
                && change.is_some()
        );
        let (b, _) = acq("tab-b", None);
        assert!(
            !b.acquired && b.holder.is_none() && b.expires_at.is_some(),
            "busy reveals only the expiry"
        );
        let (renew, _) = acq("tab-a", None);
        assert!(
            renew.acquired && renew.version == Some(1),
            "renewal without data writes nothing"
        );
        st.with_write(|c| {
            Ok(c.execute(
                "UPDATE leases SET expires_at = '2000-01-01T00:00:00.000Z'",
                [],
            )?)
        })
        .unwrap();
        assert!(acq("tab-b", None).0.acquired, "a lapsed lease is free");
        let (none, _) = st
            .doc_acquire(
                DAEMON,
                &id,
                "locks/empty",
                Acquire {
                    holder: "x".into(),
                    ttl_ms: None,
                    data: None,
                },
                &admin(),
            )
            .unwrap();
        assert!(
            none.acquired && none.version.is_none(),
            "no data: no document is created"
        );
        assert!(
            st.doc_acquire(
                DAEMON,
                &id,
                "locks/e",
                Acquire {
                    holder: String::new(),
                    ttl_ms: None,
                    data: None
                },
                &admin()
            )
            .is_err()
        );
    }

    #[test]
    fn deleting_the_artifact_erases_its_documents_and_leases() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        st.doc_set(DAEMON, &id, "t/1", json!({}), page(), &admin())
            .unwrap();
        st.doc_acquire(
            DAEMON,
            &id,
            "t/1",
            Acquire {
                holder: "h".into(),
                ttl_ms: None,
                data: None,
            },
            &admin(),
        )
        .unwrap();
        st.delete_artifact(DAEMON, &id).unwrap();
        let left: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM docs) + (SELECT COUNT(*) FROM leases)",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(left, 0);
        assert!(matches!(
            st.doc_get(&id, "t/1", &admin()),
            Err(CoreError::NotFound)
        ));
    }

    fn doc_count(st: &Store, table: &str) -> i64 {
        st.with_read(|c| {
            Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        })
        .unwrap()
    }

    #[test]
    fn versions_rise_across_the_artifact_and_survive_deletes() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let v = |w: Written| w.doc.unwrap().version;
        let a1 = v(st
            .doc_set(DAEMON, &id, "t/a", json!({"n": 1}), page(), &admin())
            .unwrap());
        let b1 = v(st
            .doc_set(DAEMON, &id, "t/b", json!({"n": 1}), page(), &admin())
            .unwrap());
        assert!(b1 > a1, "two documents get distinct increasing versions");
        st.doc_delete(DAEMON, &id, "t/a", pinned(a1), &admin())
            .unwrap();
        let a2 = v(st
            .doc_set(DAEMON, &id, "t/a", json!({"n": 2}), page(), &admin())
            .unwrap());
        assert!(
            a2 > b1 && a2 > a1,
            "a recreated document never reuses a version: {a1} {b1} {a2}"
        );
        match st.doc_set(DAEMON, &id, "t/a", json!({"n": 3}), pinned(a1), &admin()) {
            Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, Some(a2)),
            other => panic!("a pin from before the delete must conflict: {other:?}"),
        }
        let batch = st
            .doc_batch(
                DAEMON,
                &id,
                vec![BatchWrite {
                    path: "t/c".into(),
                    op: BatchOp::Set(json!({})),
                    if_version: None,
                }],
                true,
                &admin(),
            )
            .unwrap();
        let c1 = batch[0].doc.as_ref().unwrap().version;
        assert!(c1 > a2);
        let (acq, _) = st
            .doc_acquire(
                DAEMON,
                &id,
                "t/d",
                Acquire {
                    holder: "h".into(),
                    ttl_ms: None,
                    data: Some(json!({"x": 1})),
                },
                &admin(),
            )
            .unwrap();
        assert!(acq.version.unwrap() > c1);
        let other = artifact_with_caps(&st, json!({}));
        assert_eq!(
            v(
                st.doc_set(DAEMON, &other, "t/a", json!({}), page(), &admin())
                    .unwrap()
            ),
            1,
            "each artifact has its own sequence"
        );
    }

    #[test]
    fn paths_are_checked_before_bodies() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let path_err = |r: Result<Written>| match r {
            Err(CoreError::Invalid { message, .. }) => {
                assert!(message.contains("document path"), "{message}")
            }
            other => panic!("{other:?}"),
        };
        path_err(st.doc_update(DAEMON, &id, "odd", json!([1]), page(), &admin()));
        let r = StrReplace {
            field: "f".into(),
            old_str: String::new(),
            new_str: "x".into(),
            replace_all: false,
        };
        path_err(st.doc_str_replace(DAEMON, &id, "odd", r, page(), &admin()));
        let writes = vec![
            BatchWrite {
                path: "t/1".into(),
                op: BatchOp::Delete,
                if_version: None,
            },
            BatchWrite {
                path: "odd".into(),
                op: BatchOp::Update(json!([1])),
                if_version: None,
            },
        ];
        match st.doc_batch(DAEMON, &id, writes, true, &admin()) {
            Err(CoreError::InBatch { op: 1, path, error }) => {
                assert_eq!(path, "odd");
                assert!(error.to_string().contains("document path"), "{error}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn batch_failures_name_the_op_for_refusals_and_bodies() {
        let (_d, st) = store();
        let id = artifact_with_caps(
            &st,
            json!({"db": {"rules": [{"path": "locked", "write": "admin"}]}}),
        );
        let writes = vec![
            BatchWrite {
                path: "t/1".into(),
                op: BatchOp::Set(json!({})),
                if_version: None,
            },
            BatchWrite {
                path: "locked/x".into(),
                op: BatchOp::Set(json!({})),
                if_version: None,
            },
        ];
        match st.doc_batch(DAEMON, &id, writes, true, &who(Level::Interact, Some(A))) {
            Err(CoreError::InBatch { op: 1, path, error }) => {
                assert_eq!(path, "locked/x");
                assert!(matches!(&*error, CoreError::DocNotFound { path } if path == "locked/x"));
            }
            other => panic!("{other:?}"),
        }
        let writes = vec![
            BatchWrite {
                path: "t/1".into(),
                op: BatchOp::Set(json!({})),
                if_version: None,
            },
            BatchWrite {
                path: "t/2".into(),
                op: BatchOp::Update(json!([1])),
                if_version: None,
            },
        ];
        assert!(matches!(
            st.doc_batch(DAEMON, &id, writes, true, &admin()),
            Err(CoreError::InBatch { op: 1, .. })
        ));
        let writes = vec![BatchWrite {
            path: "t/9".into(),
            op: BatchOp::Set(json!("s")),
            if_version: None,
        }];
        assert!(matches!(
            st.doc_batch(DAEMON, &id, writes, true, &admin()),
            Err(CoreError::InBatch { op: 0, .. })
        ));
        assert_eq!(doc_count(&st, "docs"), 0);
    }

    #[test]
    fn built_queries_are_held_to_the_in_limit() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let q = |value: Value| DocQuery {
            collection: "t".into(),
            filters: vec![Filter {
                field: "n".into(),
                op: FilterOp::In,
                value,
            }],
            ..Default::default()
        };
        assert!(
            st.doc_query(&id, &q(json!(vec![1; MAX_IN_VALUES + 1])), &admin())
                .is_err()
        );
        assert!(st.doc_query(&id, &q(json!(1)), &admin()).is_err());
        assert!(
            st.doc_query(&id, &q(json!(vec![1; MAX_IN_VALUES])), &admin())
                .is_ok()
        );
    }

    #[test]
    fn leases_are_pruned_and_capped_per_artifact() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({}));
        let acq = |path: &str| {
            st.doc_acquire(
                DAEMON,
                &id,
                path,
                Acquire {
                    holder: "h".into(),
                    ttl_ms: None,
                    data: None,
                },
                &admin(),
            )
        };
        for i in 0..MAX_LEASES {
            assert!(acq(&format!("l/{i}")).unwrap().0.acquired);
        }
        match acq("l/extra") {
            Err(CoreError::Invalid {
                code: "resource_exhausted",
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
        assert!(
            acq("l/0").unwrap().0.acquired,
            "renewing a held lease is not a new one"
        );
        st.with_write(|c| {
            Ok(c.execute(
                "UPDATE leases SET expires_at = '2000-01-01T00:00:00.000Z' WHERE path <> 'l/0'",
                [],
            )?)
        })
        .unwrap();
        assert!(
            acq("l/extra").unwrap().0.acquired,
            "lapsed leases do not count"
        );
        assert_eq!(doc_count(&st, "leases"), 2, "lapsed leases are pruned");
    }

    /// Documents per reader (spec §8.2, §8.3): Sam and Mia, two connected
    /// people, on a store with a `db` artifact.
    mod per_reader {
        use super::*;
        use crate::store::people::tests::{approved, store};
        use crate::visibility::{Reader, Visibility};

        struct Two {
            _dir: tempfile::TempDir,
            st: Store,
            sam: String,
            mia: String,
            aid: ArtifactId,
        }

        fn two() -> Two {
            let (dir, _clock, st) = store();
            let (g, _, _) = approved(&st, "Sam", [1; 32]);
            let sam = st.viewer_of_grant(&g).unwrap().public_id;
            let (g, _, _) = approved(&st, "Mia", [2; 32]);
            let mia = st.viewer_of_grant(&g).unwrap().public_id;
            let aid = artifact_with_caps(&st, json!({"db": {}}));
            Two {
                _dir: dir,
                st,
                sam,
                mia,
                aid,
            }
        }

        fn as_(p: &str) -> Caller {
            Caller::new(Level::Admin, Some(p.to_string()))
        }

        impl Two {
            fn set(&self, by: &str, path: &str, data: Value) -> Written {
                self.st
                    .doc_set(DAEMON, &self.aid, path, data, page(), &as_(by))
                    .unwrap()
            }
            fn view(&self, p: &str) -> Visibility {
                Visibility::for_reader(
                    &self.st,
                    &Reader::Person(p.to_string()),
                    self.st.visibility_generation(),
                )
                .unwrap()
            }
            fn query(&self, reader: &str, q: DocQuery) -> DocPage {
                self.st
                    .doc_query_visible(&self.aid, &q, &as_(reader), Some(&self.view(reader)))
                    .unwrap()
            }
        }

        fn filters(w: Value) -> Vec<Filter> {
            parse_where(&w).unwrap()
        }

        #[test]
        fn where_runs_over_visible_documents_only() {
            let t = two();
            t.set(&t.mia, "c/1", json!({"secret": "aaa"}));
            t.set(&t.mia, "c/2", json!({"secret": "zzz"}));
            t.set(&t.sam, "c/3", json!({"secret": "mmm"}));
            for w in [
                json!([["secret", "<", "b"]]),
                json!([["secret", ">", "y"]]),
                json!([["secret", "==", "aaa"]]),
                json!([["secret", "!=", "mmm"]]),
                json!([]),
            ] {
                for order_by in [None, Some("secret".to_string())] {
                    for descending in [false, true] {
                        let page = t.query(
                            &t.sam,
                            DocQuery {
                                collection: "c".into(),
                                filters: filters(w.clone()),
                                order_by: order_by.clone(),
                                descending,
                                ..DocQuery::default()
                            },
                        );
                        assert!(
                            page.docs.iter().all(|d| d.path == "c/3"),
                            "{w}: the result never depends on Mia's values"
                        );
                        assert_eq!(page.withheld, Some(2), "{w}: counted whatever the filter");
                        assert_eq!(page.next_cursor, None, "{w}");
                    }
                }
            }
            // Mia's own reader sees hers and counts Sam's.
            let page = t.query(
                &t.mia,
                DocQuery {
                    collection: "c".into(),
                    ..DocQuery::default()
                },
            );
            let paths: Vec<&str> = page.docs.iter().map(|d| d.path.as_str()).collect();
            assert_eq!((paths, page.withheld), (vec!["c/1", "c/2"], Some(1)));
            // Without a view (a person's browser), every document as before.
            let page =
                t.st.doc_query_visible(
                    &t.aid,
                    &DocQuery {
                        collection: "c".into(),
                        ..DocQuery::default()
                    },
                    &as_(&t.sam),
                    None,
                )
                .unwrap();
            assert_eq!((page.docs.len(), page.withheld), (3, None));
        }

        #[test]
        fn the_count_leaves_out_what_the_rules_hide() {
            let t = two();
            // Mia's private subtree: the rules hide it from Sam, so the
            // count never tells him it holds anything.
            let mine = format!("data/users/{}/profile", t.mia);
            t.set(&t.mia, &mine, json!({"pick": 3}));
            let got = t.query(
                &t.sam,
                DocQuery {
                    collection: format!("data/users/{}", t.mia),
                    ..DocQuery::default()
                },
            );
            assert_eq!((got.docs.len(), got.withheld), (0, Some(0)));
            assert!(
                matches!(
                    t.st.doc_str_replace_visible(
                        DAEMON,
                        &t.aid,
                        &mine,
                        StrReplace {
                            field: "pick".into(),
                            old_str: "3".into(),
                            new_str: "4".into(),
                            replace_all: false
                        },
                        page(),
                        &as_(&t.sam),
                        Some(&t.view(&t.sam)),
                    ),
                    Err(CoreError::DocNotFound { .. })
                ),
                "a document the rules hide is missing, never withheld"
            );
        }

        #[test]
        fn pages_of_visible_documents_have_no_gaps() {
            let t = two();
            t.set(&t.sam, "c/a", json!({"n": 1}));
            t.set(&t.mia, "c/b", json!({"n": 2}));
            t.set(&t.mia, "c/c", json!({"n": 3}));
            t.set(&t.sam, "c/d", json!({"n": 4}));
            let page = |cursor: Option<&str>| {
                t.query(
                    &t.sam,
                    DocQuery {
                        collection: "c".into(),
                        limit: Some(1),
                        cursor: cursor.map(str::to_string),
                        ..DocQuery::default()
                    },
                )
            };
            let first = page(None);
            assert_eq!(first.docs[0].path, "c/a");
            assert_eq!(first.next_cursor.as_deref(), Some("a"));
            let second = page(Some("a"));
            assert_eq!(second.docs[0].path, "c/d");
            assert_eq!(second.next_cursor, None, "nothing withheld paginates");
            assert_eq!((first.withheld, second.withheld), (Some(2), Some(2)));
            // An ordered page of one: Sam's highest, never Mia's.
            let top = t.query(
                &t.sam,
                DocQuery {
                    collection: "c".into(),
                    order_by: Some("n".into()),
                    descending: true,
                    limit: Some(1),
                    ..DocQuery::default()
                },
            );
            assert_eq!(top.docs[0].path, "c/d");
        }

        #[test]
        fn str_replace_on_a_withheld_document_is_refused_before_it_is_evaluated() {
            let t = two();
            t.set(&t.mia, "c/1", json!({"text": "Mia's secret"}));
            let before = t.st.doc_get(&t.aid, "c/1", &as_(&t.mia)).unwrap().unwrap();
            let vis = t.view(&t.sam);
            for (old, all) in [
                ("secret", false),
                ("absent", false),
                ("s", false),
                ("s", true),
            ] {
                for pin in [page(), pinned(before.version), pinned(999)] {
                    let r = t.st.doc_str_replace_visible(
                        DAEMON,
                        &t.aid,
                        "c/1",
                        StrReplace {
                            field: "text".into(),
                            old_str: old.into(),
                            new_str: "x".into(),
                            replace_all: all,
                        },
                        pin,
                        &as_(&t.sam),
                        Some(&vis),
                    );
                    match r {
                        Err(CoreError::DocWithheld { path, authors }) => {
                            assert_eq!(path, "c/1");
                            assert_eq!(authors, vec![Author::Person(t.mia.clone())]);
                        }
                        other => panic!("{old}: {other:?}"),
                    }
                }
            }
            // Nothing was written, and a missing document is missing.
            assert_eq!(
                t.st.doc_get(&t.aid, "c/1", &as_(&t.mia)).unwrap().unwrap(),
                before
            );
            assert!(matches!(
                t.st.doc_str_replace_visible(
                    DAEMON,
                    &t.aid,
                    "c/none",
                    StrReplace {
                        field: "text".into(),
                        old_str: "a".into(),
                        new_str: "b".into(),
                        replace_all: false
                    },
                    page(),
                    &as_(&t.sam),
                    Some(&vis),
                ),
                Err(CoreError::DocNotFound { .. })
            ));
            // Sam's own document is his to edit.
            t.set(&t.sam, "c/2", json!({"text": "Sam's"}));
            let w =
                t.st.doc_str_replace_visible(
                    DAEMON,
                    &t.aid,
                    "c/2",
                    StrReplace {
                        field: "text".into(),
                        old_str: "Sam".into(),
                        new_str: "S".into(),
                        replace_all: false,
                    },
                    page(),
                    &as_(&t.sam),
                    Some(&vis),
                )
                .unwrap();
            assert_eq!(w.doc.unwrap().data, json!({"text": "S's"}));
        }

        fn pad_to_cap(base: &Value) -> Value {
            let n = MAX_DOC_BYTES - serde_json::to_vec(base).unwrap().len();
            json!({"pad": "a".repeat(n)})
        }

        #[test]
        fn a_merge_on_a_withheld_document_is_refused_before_it_is_evaluated() {
            let t = two();
            t.set(&t.mia, "c/t", json!({"flag": true, "text": "hello"}));
            t.set(&t.mia, "c/f", json!({"flag": false, "text": "hello"}));
            let vis = t.view(&t.sam);
            // A patch that fits beside {flag: true} but not {flag: false}.
            let patch = pad_to_cap(&json!({"flag": true, "text": "hello", "pad": ""}));
            let before: Vec<Doc> = ["c/t", "c/f"]
                .iter()
                .map(|p| t.st.doc_get(&t.aid, p, &as_(&t.mia)).unwrap().unwrap())
                .collect();
            let refused = |r: Result<Written>, what: &str| match r {
                Err(CoreError::DocWithheld { authors, .. }) => {
                    assert_eq!(authors, vec![Author::Person(t.mia.clone())], "{what}")
                }
                other => panic!("{what}: {other:?}"),
            };
            for p in ["c/t", "c/f"] {
                for pin in [page(), pinned(1), pinned(999)] {
                    refused(
                        t.st.doc_update_visible(
                            DAEMON,
                            &t.aid,
                            p,
                            patch.clone(),
                            pin,
                            &as_(&t.sam),
                            Some(&vis),
                        ),
                        p,
                    );
                }
                match t.st.doc_batch_visible(
                    DAEMON,
                    &t.aid,
                    vec![
                        BatchWrite {
                            path: "c/sam".into(),
                            op: BatchOp::Set(json!({"a": 1})),
                            if_version: None,
                        },
                        BatchWrite {
                            path: p.into(),
                            op: BatchOp::Update(patch.clone()),
                            if_version: None,
                        },
                    ],
                    true,
                    &as_(&t.sam),
                    Some(&vis),
                ) {
                    Err(CoreError::InBatch { op: 1, error, .. }) => {
                        assert!(matches!(*error, CoreError::DocWithheld { .. }), "{error:?}")
                    }
                    other => panic!("batch {p}: {other:?}"),
                }
                match t.st.doc_acquire_visible(
                    DAEMON,
                    &t.aid,
                    p,
                    Acquire {
                        holder: "h".into(),
                        ttl_ms: None,
                        data: Some(patch.clone()),
                    },
                    &as_(&t.sam),
                    Some(&vis),
                ) {
                    Err(CoreError::DocWithheld { .. }) => {}
                    other => panic!("acquire {p}: {other:?}"),
                }
            }
            // Nothing was written, not even the batch's other write.
            let after: Vec<Doc> = ["c/t", "c/f"]
                .iter()
                .map(|p| t.st.doc_get(&t.aid, p, &as_(&t.mia)).unwrap().unwrap())
                .collect();
            assert_eq!(after, before);
            assert_eq!(t.st.doc_get(&t.aid, "c/sam", &as_(&t.sam)).unwrap(), None);
            assert_eq!(doc_count(&t.st, "leases"), 0);
            // Blind writes stay allowed: set and delete in a batch, and a
            // lease without data.
            let w =
                t.st.doc_batch_visible(
                    DAEMON,
                    &t.aid,
                    vec![
                        BatchWrite {
                            path: "c/t".into(),
                            op: BatchOp::Set(json!({"x": 1})),
                            if_version: None,
                        },
                        BatchWrite {
                            path: "c/f".into(),
                            op: BatchOp::Delete,
                            if_version: None,
                        },
                    ],
                    true,
                    &as_(&t.sam),
                    Some(&vis),
                )
                .unwrap();
            assert!(w[1].deleted);
            let (a, _) =
                t.st.doc_acquire_visible(
                    DAEMON,
                    &t.aid,
                    "c/t",
                    Acquire {
                        holder: "h".into(),
                        ttl_ms: None,
                        data: None,
                    },
                    &as_(&t.sam),
                    Some(&vis),
                )
                .unwrap();
            assert!(a.acquired);
            // Sam's own document merges as before.
            t.set(&t.sam, "c/mine", json!({"a": 1}));
            let w =
                t.st.doc_update_visible(
                    DAEMON,
                    &t.aid,
                    "c/mine",
                    json!({"b": 2}),
                    page(),
                    &as_(&t.sam),
                    Some(&vis),
                )
                .unwrap();
            assert_eq!(w.doc.unwrap().data, json!({"a": 1, "b": 2}));
        }

        #[test]
        fn a_lease_on_a_withheld_document_never_confirms_its_holder() {
            let t = two();
            t.set(&t.mia, "c/1", json!({"a": 1}));
            let acquire = |by: &str, holder: &str, vis: Option<&Visibility>| {
                t.st.doc_acquire_visible(
                    DAEMON,
                    &t.aid,
                    "c/1",
                    Acquire {
                        holder: holder.into(),
                        ttl_ms: Some(600_000),
                        data: None,
                    },
                    &as_(by),
                    vis,
                )
                .unwrap()
                .0
            };
            let mine = acquire(&t.mia, "mia-laptop", Some(&t.view(&t.mia)));
            assert!(mine.acquired);
            let vis = t.view(&t.sam);
            let miss = acquire(&t.sam, "guess", Some(&vis));
            let hit = acquire(&t.sam, "mia-laptop", Some(&vis));
            assert_eq!(miss, hit, "a guessed holder answers as any other");
            assert_eq!((hit.acquired, hit.holder), (false, None));
            assert_eq!(hit.expires_at, mine.expires_at, "her lease is untouched");
            // Mia renews her own lease as before.
            assert!(acquire(&t.mia, "mia-laptop", Some(&t.view(&t.mia))).acquired);
        }

        #[test]
        fn withheld_documents_before_the_cursor_are_counted() {
            let t = two();
            t.set(&t.mia, "c/a", json!({"n": 1}));
            t.set(&t.sam, "c/b", json!({"n": 2}));
            t.set(&t.mia, "c/c", json!({"n": 3}));
            t.set(&t.sam, "c/d", json!({"n": 4}));
            t.set(&t.mia, "c/e", json!({"n": 5}));
            for cursor in [None, Some("b"), Some("c"), Some("d"), Some("zz")] {
                let got = t.query(
                    &t.sam,
                    DocQuery {
                        collection: "c".into(),
                        cursor: cursor.map(str::to_string),
                        ..DocQuery::default()
                    },
                );
                assert_eq!(got.withheld, Some(3), "{cursor:?}");
                assert!(
                    got.docs.iter().all(|d| d.id == "b" || d.id == "d"),
                    "{cursor:?}"
                );
            }
        }

        #[test]
        fn the_byte_budget_counts_visible_bytes_only() {
            let t = two();
            t.set(&t.mia, "c/big", json!({"text": "m".repeat(1000)}));
            t.set(&t.sam, "c/small", json!({"text": "s"}));
            let q = DocQuery {
                collection: "c".into(),
                order_by: Some("text".into()),
                ..DocQuery::default()
            };
            let got =
                t.st.doc_query_within(&t.aid, &q, &as_(&t.sam), Some(&t.view(&t.sam)), 500)
                    .unwrap();
            assert_eq!((got.docs.len(), got.withheld), (1, Some(1)));
            // Mia's own reader is held to the budget by her bytes.
            assert!(matches!(
                t.st.doc_query_within(&t.aid, &q, &as_(&t.mia), Some(&t.view(&t.mia)), 500),
                Err(CoreError::Invalid {
                    code: "resource_exhausted",
                    ..
                })
            ));
        }

        #[test]
        fn a_write_answers_with_the_authors_it_leaves() {
            let t = two();
            let w = t.set(&t.mia, "c/1", json!({"a": 1}));
            assert_eq!(w.authors, vec![Author::Person(t.mia.clone())]);
            // Sam's blind write over Mia's document joins her; it never
            // makes it his.
            let w = t.set(&t.sam, "c/1", json!({"b": 2}));
            assert_eq!(
                w.authors,
                vec![Author::Person(t.mia.clone()), Author::Person(t.sam.clone())]
            );
            let (doc, authors) =
                t.st.doc_get_authored(&t.aid, "c/1", &as_(&t.sam))
                    .unwrap()
                    .unwrap();
            assert_eq!((doc.data, authors), (json!({"b": 2}), w.authors.clone()));
            let w =
                t.st.doc_delete(DAEMON, &t.aid, "c/1", page(), &as_(&t.sam))
                    .unwrap();
            assert!(w.deleted && w.authors.is_empty());
            let w = t.set(&t.sam, "c/1", json!({"c": 3}));
            assert_eq!(w.authors, vec![Author::Person(t.sam.clone())]);
        }
    }
}
