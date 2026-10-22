//! Documents of the `db` capability (spec §5 `docs`, §9 "db"): what a
//! document, a query, a pin and a write's outcome are, and the checks every
//! body and filter passes. The documents themselves live in each
//! artifact's own store ([`crate::app_data::AppData`]), which applies the
//! artifact's rules to the caller the instance resolved; the instance keeps
//! the artifact's declaration ([`Store::doc_declared`]).

use super::Store;
use clax_core::db::invalid_argument;
use clax_core::visibility::Author;
use clax_core::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cmp::Ordering;

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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub if_version: Option<u64>,
    pub lww: bool,
}

/// A change to announce as the `doc` SSE event; `version: None` after a
/// delete; `private_to` is the viewer whose private subtree holds the path;
/// `read_level` is the least level that may read it. `self_read`, for a path
/// in an opened `{self}` subtree, is that subtree's viewer and the least
/// level at which they may read it (which `{self}` rules may lower).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocChange {
    pub path: String,
    pub version: Option<u64>,
    pub private_to: Option<String>,
    pub read_level: clax_core::db::Level,
    pub self_read: Option<(String, clax_core::db::Level)>,
}

/// A write's outcome; `change` is `None` when nothing changed. The written
/// document's version (`doc.version`, `change.version`) is the artifact's
/// next sequence number, never a per-document count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub path: String,
    pub doc: Option<Doc>,
    pub created: bool,
    pub deleted: bool,
    pub change: Option<DocChange>,
    /// The document's authors after the write, read in its transaction
    /// (spec §8.2): every writer since it was created, the owner when none
    /// is stored; empty after a delete. On the wire as their stored
    /// tokens ([`author_tokens`]).
    #[serde(with = "author_tokens")]
    pub authors: Vec<Author>,
}

impl Written {
    /// The written document as a journaled `db.*` write answers it:
    /// `{id, version}`, `null` without a document (a delete).
    #[must_use]
    pub fn doc_ref(&self) -> Value {
        self.doc.as_ref().map_or(
            Value::Null,
            |d| serde_json::json!({"id": d.id, "version": d.version}),
        )
    }
}

/// A page of a query ([`AppData::doc_query_visible`](crate::app_data::AppData::doc_query_visible)): the documents, the
/// cursor of the next page of an unordered query, and, for a reader that
/// may not see every document, how many of the collection's documents it
/// withholds (whatever the filter, cursor and limit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocPage {
    pub docs: Vec<Doc>,
    pub next_cursor: Option<String>,
    pub withheld: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchOp {
    Set(Value),
    Update(Value),
    Delete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchWrite {
    pub path: String,
    pub op: BatchOp,
    pub if_version: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrReplace {
    pub field: String,
    pub old_str: String,
    pub new_str: String,
    pub replace_all: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Acquire {
    pub holder: String,
    pub ttl_ms: Option<u64>,
    pub data: Option<Value>,
}

/// `acquire`'s result: on a grant, the holder, the lease's expiry, and the
/// document's version (absent when there is no document); when busy, only the
/// expiry of the lease in force.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Acquired {
    pub acquired: bool,
    pub version: Option<u64>,
    pub expires_at: Option<String>,
    pub holder: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub value: Value,
}

/// A query of one collection. Without `order_by` results are in document ID
/// order and page with `cursor` (the last ID of the previous page); with it,
/// one page of at most `limit` (every match when `limit` is `None`), missing
/// fields last in either direction.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
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

pub(crate) fn rank(v: &Value) -> u8 {
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
    pub(crate) fn matches(&self, data: &Value) -> bool {
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

/// The live artifact's declared capabilities; `NotFound` when it is missing
/// or deleted.
pub(crate) fn caps_in(c: &Connection, id: &ArtifactId) -> Result<Value> {
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

/// A document's authors as their stored tokens ([`Author::token`]), as
/// the artifact store's wire carries them.
pub mod author_tokens {
    use clax_core::visibility::Author;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(authors: &[Author], s: S) -> Result<S::Ok, S::Error> {
        authors
            .iter()
            .filter_map(Author::token)
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Author>, D::Error> {
        let tokens = Vec::<String>::deserialize(d)?;
        Ok(tokens.iter().map(|t| Author::of_token(Some(t))).collect())
    }
}

impl Store {
    /// Whether the live artifact's current declaration names `db`;
    /// `NotFound` when the artifact is missing or deleted.
    pub fn doc_declared(&self, id: &ArtifactId) -> Result<bool> {
        self.with_read(|c| Ok(caps_in(c, id)?.get("db").is_some()))
    }

    /// The live artifact's declared capabilities, whose `db` rules its
    /// store applies to each caller
    /// ([`AppDataOp`](crate::app_data::AppDataOp)'s `caps`); `NotFound`
    /// when it is missing or deleted.
    pub fn doc_capabilities(&self, id: &ArtifactId) -> Result<Value> {
        self.with_read(|c| caps_in(c, id))
    }
}
