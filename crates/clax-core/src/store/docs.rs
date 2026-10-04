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

use super::Store;
use crate::db::{Caller, Op, Rules, collection_path, doc_path, invalid_argument};
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::HashSet;

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
}

pub enum BatchOp {
    Set(Value),
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

fn now_plus(ms: u64) -> String {
    (chrono::Utc::now() + chrono::Duration::milliseconds(ms as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The live artifact's declared capabilities; `NotFound` when it is missing
/// or deleted.
fn caps_in(c: &Connection, id: &ArtifactId) -> Result<Value> {
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

fn write_in(
    c: &Connection,
    id: &ArtifactId,
    rules: &Rules,
    caller: &Caller,
    path: &str,
    pin: Pin,
    next: impl FnOnce(Option<&Doc>) -> Result<Option<Body>>,
) -> Result<Written> {
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
            let now = Store::now();
            c.execute(
                "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(artifact_id, path) DO UPDATE SET json = excluded.json, version = excluded.version, updated_at = excluded.updated_at",
                params![id.as_str(), dp.path, dp.collection, body.json, version as i64, now],
            )?;
            let doc = Doc {
                id: dp.path.rsplit('/').next().unwrap_or_default().to_string(),
                path: dp.path.clone(),
                collection: dp.collection.clone(),
                data: body.data,
                version,
                updated_at: now,
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
            })
        }
        None => {
            let n = c.execute(
                "DELETE FROM docs WHERE artifact_id = ?1 AND path = ?2",
                params![id.as_str(), dp.path],
            )?;
            if n > 0 {
                next_version(c, id, current.as_ref().map_or(0, |d| d.version))?;
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
            })
        }
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
        id: &ArtifactId,
        path: &str,
        data: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        let body = Body::new(data);
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            write_in(tx, id, &rules, caller, path, pin, |_| body.map(Some))
        })
    }

    /// Merges `patch` into the existing document.
    ///
    /// # Errors
    /// `invalid_argument` ("<path> does not exist; use set to create it")
    /// when the document is absent; `DocNotFound` when `caller` may not write it.
    pub fn doc_update(
        &self,
        id: &ArtifactId,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        doc_path(path)?;
        check_body(&patch, true)?;
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            write_in(tx, id, &rules, caller, path, pin, |cur| {
                Body::of(update_body(path, cur, patch))
            })
        })
    }

    /// Deletes the document; deleting a missing document succeeds with `deleted: false`.
    pub fn doc_delete(
        &self,
        id: &ArtifactId,
        path: &str,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            write_in(tx, id, &rules, caller, path, pin, |_| Ok(None))
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
        id: &ArtifactId,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        doc_path(path)?;
        if r.old_str.is_empty() {
            return Err(invalid_argument("old_str must not be empty"));
        }
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            write_in(tx, id, &rules, caller, path, pin, |cur| Body::of((|| {
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
            let mut stmt = c.prepare(&format!("{SELECT_DOC} WHERE artifact_id = ?1 AND collection = ?2 AND path > ?3 ORDER BY path"))?;
            let rows = stmt
                .query_map(params![id.as_str(), collection, after], row_parts)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut docs = Vec::new();
            let mut bytes = 0;
            for parts in rows {
                if !rules.allows(&parts.0, Op::Read, caller) {
                    continue;
                }
                let size = parts.2.len();
                let d = to_doc(id, parts)?;
                if q.filters.iter().all(|f| f.matches(&d.data)) {
                    bytes += size;
                    docs.push(d);
                }
            }
            if q.order_by.is_some() && q.limit.is_none() && bytes > MAX_UNLIMITED_QUERY_BYTES {
                return Err(CoreError::invalid(
                    "resource_exhausted",
                    format!(
                        "the matching documents hold {bytes} bytes, over the {MAX_UNLIMITED_QUERY_BYTES} an ordered query without a limit may return; add a limit or narrow the query"
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
                return Ok((docs, None));
            }
            let next = (docs.len() > limit).then(|| docs[limit - 1].id.clone());
            docs.truncate(limit);
            Ok((docs, next))
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
        id: &ArtifactId,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
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
        let writes: Vec<(BatchWrite, Option<Result<Body>>)> = writes
            .into_iter()
            .map(
                |mut w| match std::mem::replace(&mut w.op, BatchOp::Delete) {
                    BatchOp::Set(data) => (w, Some(Body::new(data))),
                    op => (BatchWrite { op, ..w }, None),
                },
            )
            .collect();
        self.with_tx(|tx| {
            let rules = rules_in(tx, id)?;
            writes
                .into_iter()
                .enumerate()
                .map(|(i, (w, body))| {
                    let pin = Pin {
                        if_version: w.if_version,
                        lww,
                    };
                    let path = w.path.as_str();
                    match (w.op, body) {
                        (_, Some(body)) => {
                            write_in(tx, id, &rules, caller, path, pin, |_| body.map(Some))
                        }
                        (BatchOp::Update(patch), None) => {
                            write_in(tx, id, &rules, caller, path, pin, |cur| {
                                Body::of(update_body(path, cur, patch))
                            })
                        }
                        (_, None) => write_in(tx, id, &rules, caller, path, pin, |_| Ok(None)),
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
        id: &ArtifactId,
        path: &str,
        a: Acquire,
        caller: &Caller,
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
            let now = Store::now();
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
                && *holder != a.holder
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
            let expires = now_plus(ttl);
            tx.execute(
                "INSERT INTO leases (artifact_id, path, holder, expires_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(artifact_id, path) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at",
                params![id.as_str(), dp.path, a.holder, expires],
            )?;
            let empty = Value::Object(Map::new());
            let (version, change) = match a.data {
                Some(patch) => {
                    let w = write_in(tx, id, &rules, caller, &dp.path, Pin { if_version: None, lww: true }, |cur| {
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
    use crate::store::test_util::{artifact_with_caps, store};
    use serde_json::json;

    const A: &str = "u_00000000000000000000aa";
    const B: &str = "u_00000000000000000000bb";

    fn who(level: Level, viewer: Option<&str>) -> Caller {
        Caller {
            level,
            viewer: viewer.map(str::to_string),
        }
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
        let w = st.doc_delete(&id, "tasks/t1", pinned(2), &admin()).unwrap();
        assert!(w.deleted);
        assert_eq!(w.change.unwrap().version, None);
        assert_eq!(st.doc_get(&id, "tasks/t1", &admin()).unwrap(), None);
        assert!(
            !st.doc_delete(&id, "tasks/t1", Pin::default(), &admin())
                .unwrap()
                .deleted,
            "deleting nothing succeeds"
        );
    }

    #[test]
    fn existing_documents_need_a_pin_unless_last_writer_wins() {
        let (_d, st) = store();
        let id = artifact_with_caps(&st, json!({"db": {}}));
        st.doc_set(&id, "tasks/t1", json!({"n": 1}), Pin::default(), &admin())
            .unwrap();
        match st.doc_set(&id, "tasks/t1", json!({"n": 2}), Pin::default(), &admin()) {
            Err(CoreError::DocPinRequired { path, current }) => {
                assert_eq!((path.as_str(), current), ("tasks/t1", 1))
            }
            other => panic!("{other:?}"),
        }
        match st.doc_set(&id, "tasks/t1", json!({"n": 2}), pinned(7), &admin()) {
            Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, Some(1)),
            other => panic!("{other:?}"),
        }
        match st.doc_set(&id, "tasks/none", json!({}), pinned(1), &admin()) {
            Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, None),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            st.doc_set(&id, "tasks/t1", json!({"n": 3}), page(), &admin())
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
            &id,
            "c/d",
            json!({"a": {"x": 1, "y": 2}, "b": [1, 2], "c": "keep"}),
            page(),
            &admin(),
        )
        .unwrap();
        let d = st.doc_update(&id, "c/d", json!({"a": {"y": {"__delete__": true}, "z": 3}, "b": [3], "n": {"m": {"__delete__": true}}}), page(), &admin())
            .unwrap().doc.unwrap();
        assert_eq!(
            d.data,
            json!({"a": {"x": 1, "z": 3}, "b": [3], "c": "keep", "n": {}})
        );
        match st.doc_update(&id, "c/missing", json!({"a": 1}), page(), &admin()) {
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
        match st.doc_batch(&id, batch, true, &admin()) {
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
        match st.doc_update(&id, "c/d", json!({"__delete__": true}), page(), &admin()) {
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
                st.doc_set(&id, "c/d", body, page(), &admin()),
                Err(CoreError::Invalid {
                    code: "invalid_argument",
                    ..
                })
            ));
        }
        assert!(
            matches!(
                st.doc_set(&id, "c", json!({}), page(), &admin()),
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
                &id,
                "t/1",
                json!({}),
                page(),
                &who(Level::Interact, Some(A))
            ),
            Err(CoreError::DocNotFound { .. })
        ));
        st.doc_set(&id, "t/1", json!({}), page(), &admin()).unwrap();
        assert!(
            st.doc_get(&id, "t/1", &who(Level::View, None))
                .unwrap()
                .is_some(),
            "view reads shared documents"
        );
        let open = artifact_with_caps(&st, json!({}));
        assert!(matches!(
            st.doc_set(&open, "t/1", json!({}), page(), &who(Level::View, Some(A))),
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
                    st.doc_set(&id, &mine, json!({}), page(), &c),
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
            st.doc_batch(&id, chunk, true, &admin()).unwrap();
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
            st.doc_set(&id, &format!("tasks/{k}"), v, page(), &admin())
                .unwrap();
        }
        st.doc_set(&id, "tasks/a/sub/z", json!({"n": 0}), page(), &admin())
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
        st.doc_set(&id, "t/1", json!({"n": 1}), page(), &admin())
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
        match st.doc_batch(&id, writes, false, &admin()) {
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
        match st.doc_batch(&id, dup, false, &admin()) {
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
            st.doc_batch(&id, many, false, &admin()),
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
            st.doc_str_replace(&id, "p/1", r("a", false, "html"), pinned(1), &admin()),
            Err(CoreError::Invalid {
                code: "old_str_not_unique",
                ..
            })
        ));
        assert!(matches!(
            st.doc_str_replace(&id, "p/1", r("q", false, "html"), pinned(1), &admin()),
            Err(CoreError::Invalid {
                code: "old_str_not_found",
                ..
            })
        ));
        assert!(matches!(
            st.doc_str_replace(&id, "p/1", r("a", false, "n"), pinned(1), &admin()),
            Err(CoreError::Invalid {
                code: "invalid_argument",
                ..
            })
        ));
        let d = st
            .doc_str_replace(&id, "p/1", r("b", false, "html"), pinned(1), &admin())
            .unwrap()
            .doc
            .unwrap();
        assert_eq!((d.data["html"].as_str(), d.version), (Some("a-Z-a"), 2));
        let d = st
            .doc_str_replace(&id, "p/1", r("a", true, "html"), pinned(2), &admin())
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
            st.doc_set(&id, "f/new", json!({}), page(), &admin()),
            Err(CoreError::Invalid {
                code: "quota_exceeded",
                ..
            })
        ));
        assert!(
            st.doc_set(&id, "f/0", json!({"n": 1}), page(), &admin())
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
        st.doc_set(&id, "t/1", json!({}), page(), &admin()).unwrap();
        st.doc_acquire(
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
        st.delete_artifact(&id).unwrap();
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
            .doc_set(&id, "t/a", json!({"n": 1}), page(), &admin())
            .unwrap());
        let b1 = v(st
            .doc_set(&id, "t/b", json!({"n": 1}), page(), &admin())
            .unwrap());
        assert!(b1 > a1, "two documents get distinct increasing versions");
        st.doc_delete(&id, "t/a", pinned(a1), &admin()).unwrap();
        let a2 = v(st
            .doc_set(&id, "t/a", json!({"n": 2}), page(), &admin())
            .unwrap());
        assert!(
            a2 > b1 && a2 > a1,
            "a recreated document never reuses a version: {a1} {b1} {a2}"
        );
        match st.doc_set(&id, "t/a", json!({"n": 3}), pinned(a1), &admin()) {
            Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, Some(a2)),
            other => panic!("a pin from before the delete must conflict: {other:?}"),
        }
        let batch = st
            .doc_batch(
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
            v(st.doc_set(&other, "t/a", json!({}), page(), &admin())
                .unwrap()),
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
        path_err(st.doc_update(&id, "odd", json!([1]), page(), &admin()));
        let r = StrReplace {
            field: "f".into(),
            old_str: String::new(),
            new_str: "x".into(),
            replace_all: false,
        };
        path_err(st.doc_str_replace(&id, "odd", r, page(), &admin()));
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
        match st.doc_batch(&id, writes, true, &admin()) {
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
        match st.doc_batch(&id, writes, true, &who(Level::Interact, Some(A))) {
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
            st.doc_batch(&id, writes, true, &admin()),
            Err(CoreError::InBatch { op: 1, .. })
        ));
        let writes = vec![BatchWrite {
            path: "t/9".into(),
            op: BatchOp::Set(json!("s")),
            if_version: None,
        }];
        assert!(matches!(
            st.doc_batch(&id, writes, true, &admin()),
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
}
