//! The owner's inbox (spec 2026-10-06-agent-questions-and-inbox §8.1): list
//! and search items, the unread summary, one item, and read marks. Every
//! route answers 403 `forbidden` to callers other than the owner and keeps
//! the viewer routes' origin rules. The changes the marks make reach
//! clients through the inbox listener ([`crate::inbox::listen`]); the
//! responses carry the unread count after them.

use super::artifacts::{body, path};
use crate::error::ApiError;
use crate::identity::Identity;
use crate::inbox::{view, views};
use crate::state::AppState;
use crate::viewer::SameOrigin;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::store::inbox::{
    Agent, InboxQuery, Kind, MAX_PAGE, ReadFilter, is_item_id, stored_time,
};
use clax_core::store::questions::ListStatus;
use clax_core::{ArtifactId, CoreError};
use serde::Deserialize;
use serde_json::{Value, json};

/// Most matches `total` counts; past it, `total` is `"10000+"`.
pub const TOTAL_CAP: u32 = 10_000;
/// Most item IDs one `POST /api/inbox/read` names.
pub const MAX_MARK_IDS: usize = 500;
/// Unread items other than questions the summary shows.
pub const SUMMARY_LATEST: u32 = 5;
/// Open questions the summary shows at most.
pub const SUMMARY_QUESTIONS: u32 = 200;

fn invalid(msg: impl Into<String>) -> ApiError {
    ApiError::bad_request("invalid_query", msg)
}

/// The filters a listing and a mark-all share.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    #[serde(default)]
    q: Option<String>,
    /// Kinds, comma-separated (an array in a request body too).
    #[serde(default)]
    kind: Option<Kinds>,
    #[serde(default)]
    artifact: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Kinds {
    Comma(String),
    List(Vec<String>),
}

impl Filter {
    /// The query these filters make. 400 `invalid_query` for an unknown
    /// kind, an artifact that is not an artifact ID, or a date or time that
    /// is not RFC 3339 (`since`/`until` are normalised to the stored form).
    fn query(self) -> Result<InboxQuery, ApiError> {
        let names: Vec<String> = match self.kind {
            None => Vec::new(),
            Some(Kinds::Comma(s)) => s.split(',').map(str::to_string).collect(),
            Some(Kinds::List(v)) => v,
        };
        let mut kinds = Vec::new();
        for k in names.iter().map(|k| k.trim()).filter(|k| !k.is_empty()) {
            kinds.push(Kind::parse(k).ok_or_else(|| {
                invalid(format!(
                    "'{k}' is not a kind: reply, version, published, question or finished"
                ))
            })?);
        }
        let artifact = match self.artifact.as_deref().filter(|a| !a.is_empty()) {
            Some(a) => Some(
                ArtifactId::parse(a)
                    .map_err(|_| invalid(format!("'{a}' is not an artifact ID")))?
                    .as_str()
                    .to_string(),
            ),
            None => None,
        };
        let agent = self
            .agent
            .filter(|a| !a.is_empty())
            .map(|a| match a.starts_with("a_") {
                true => Agent::Handle(a),
                false => Agent::Harness(a),
            });
        let time = |raw: Option<String>, end: bool| -> Result<Option<String>, ApiError> {
            match raw.as_deref().filter(|t| !t.is_empty()) {
                None => Ok(None),
                Some(t) => stored_time(t, end).map(Some).ok_or_else(|| {
                    invalid(format!(
                        "'{t}' is not a date (YYYY-MM-DD) or an RFC 3339 time"
                    ))
                }),
            }
        };
        Ok(InboxQuery {
            text: self.q.filter(|q| !q.trim().is_empty()),
            kinds,
            artifact,
            agent,
            since: time(self.since, false)?,
            until: time(self.until, true)?,
            ..InboxQuery::default()
        })
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: Option<String>,
    /// Kinds, comma-separated.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    artifact: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    read: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

fn query_of<T>(q: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    q.map(|Query(q)| q).map_err(|e| invalid(e.body_text()))
}

/// `GET /api/inbox?q=&kind=<k,…>&artifact=<aid>&agent=<harness|handle>&since=&until=&read=unread|read|all&before=<cursor>&limit=`
/// (owner) → `{items, next_cursor, unread, total?}`: items newest first,
/// `limit` (default 50, at most 200) a page, `next_cursor` the `before` of
/// the next page (a decimal string) or null. `total`, the number matching,
/// is there when the query has text or a filter (`read` other than `all`
/// included), counted up to 10,000 and `"10000+"` beyond. Default
/// `read=all`. `unread` and `total` are read just after the page, not in
/// its snapshot: under concurrent writes they may differ from it by the
/// changes since, which the `inbox` topic then announces. 400
/// `invalid_query` for a bad filter, date or cursor.
pub async fn list(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    who.require_owner("reads the inbox")?;
    let q = query_of(q)?;
    let mut iq = Filter {
        q: q.q,
        kind: q.kind.map(Kinds::Comma),
        artifact: q.artifact,
        agent: q.agent,
        since: q.since,
        until: q.until,
    }
    .query()?;
    iq.read = match q.read.as_deref() {
        None | Some("" | "all") => ReadFilter::All,
        Some("unread") => ReadFilter::Unread,
        Some("read") => ReadFilter::Read,
        Some(_) => return Err(invalid("read is unread, read or all")),
    };
    iq.before = match q.before.as_deref().filter(|b| !b.is_empty()) {
        None => None,
        Some(b) => Some(
            b.parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid("before is a cursor from next_cursor"))?,
        ),
    };
    iq.limit = match q.limit.as_deref().filter(|l| !l.is_empty()) {
        None => 0,
        Some(l) => l
            .parse::<u32>()
            .map_err(|_| invalid(format!("limit is 1 to {MAX_PAGE}")))?
            .clamp(1, MAX_PAGE),
    };
    let filtered = iq.text.is_some()
        || !iq.kinds.is_empty()
        || iq.artifact.is_some()
        || iq.agent.is_some()
        || iq.since.is_some()
        || iq.until.is_some()
        || iq.read != ReadFilter::All;
    // A filtered list (a search included) also counts what matches, which
    // reads up to the whole inbox, so it waits in the bulk lane; an
    // unfiltered page reads one page and the unread count.
    let job = move |db: &clax_core::Store| {
        let (rows, next) = db.inbox_list(&iq)?;
        let items = views(db, &rows)?;
        let unread = db.inbox_unread()?;
        let mut out = json!({
            "items": items,
            "next_cursor": next.map(|n| n.to_string()),
            "unread": unread,
        });
        if filtered {
            let n = db.inbox_count(&iq, TOTAL_CAP + 1)?;
            out["total"] = if n > TOTAL_CAP {
                json!(format!("{TOTAL_CAP}+"))
            } else {
                json!(n)
            };
        }
        Ok(out)
    };
    let out = if filtered {
        s.store_call_bulk(job).await?
    } else {
        s.store_call(job).await?
    };
    Ok(Json(out))
}

/// `GET /api/inbox/summary` (owner) → `{unread, questions, latest}`: the
/// unread count, the open questions' views oldest first, and the five
/// newest unread items other than questions, for the gallery and the top
/// bar. Each part is read on its own, one just after another.
pub async fn summary(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
) -> Result<Json<Value>, ApiError> {
    who.require_owner("reads the inbox")?;
    let out = s
        .store_call(move |db| {
            let (open, _) = db.list_questions(ListStatus::Open, SUMMARY_QUESTIONS)?;
            let questions = open
                .iter()
                .map(|q| crate::questions::view(db, q))
                .collect::<clax_core::Result<Vec<_>>>()?;
            let (rows, _) = db.inbox_list(&InboxQuery {
                kinds: vec![Kind::Reply, Kind::Version, Kind::Published, Kind::Finished],
                read: ReadFilter::Unread,
                limit: SUMMARY_LATEST,
                ..InboxQuery::default()
            })?;
            Ok(json!({
                "unread": db.inbox_unread()?,
                "questions": questions,
                "latest": views(db, &rows)?,
            }))
        })
        .await?;
    Ok(Json(out))
}

/// The item ID in a path: 404 when it is not one.
fn item_id(p: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    let id = path(p)?;
    if is_item_id(&id) {
        Ok(id)
    } else {
        Err(CoreError::NotFound.into())
    }
}

/// `GET /api/inbox/<id>` (owner) → `{item}`; 404 when there is none.
pub async fn get_one(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    who.require_owner("reads the inbox")?;
    let id = item_id(p)?;
    let item = s
        .store_call(move |db| {
            let i = db.inbox_item(&id)?.ok_or(CoreError::NotFound)?;
            view(db, &i)
        })
        .await?;
    Ok(Json(json!({"item": item})))
}

/// Marks item `id` read or unread → `{item, unread}`.
async fn mark_one(s: AppState, id: String, read: bool) -> Result<Json<Value>, ApiError> {
    let out = s
        .store_call(move |db| {
            db.inbox_item(&id)?.ok_or(CoreError::NotFound)?;
            db.inbox_mark(std::slice::from_ref(&id), read)?;
            let i = db.inbox_item(&id)?.ok_or(CoreError::NotFound)?;
            Ok(json!({"item": view(db, &i)?, "unread": db.inbox_unread()?}))
        })
        .await?;
    Ok(Json(out))
}

/// `POST /api/inbox/<id>/read` (owner): the owner opened the item →
/// `{item, unread}`; 404 when there is none. Idempotent.
pub async fn read_one(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    who.require_owner("reads the inbox")?;
    mark_one(s, item_id(p)?, true).await
}

/// `POST /api/inbox/<id>/unread` (owner) → `{item, unread}`; 404 when
/// there is none.
pub async fn unread_one(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    who.require_owner("reads the inbox")?;
    mark_one(s, item_id(p)?, false).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadBody {
    #[serde(default)]
    ids: Option<Vec<String>>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    filter: Option<Filter>,
    /// The newest `seq` the client showed: items made since stay unread.
    #[serde(default)]
    upto: Option<i64>,
}

/// `POST /api/inbox/read` (owner), body `{ids}` (at most 500 item IDs) or
/// `{all: true, filter?: {q, kind, artifact, agent, since, until}, upto?}`
/// (every unread item matching the filter, up to item `seq` `upto`, the
/// newest the client showed) → `{marked, unread}`. 400 `invalid_query` for
/// neither or both, too many IDs, or a bad filter.
pub async fn read_many(
    State(s): State<AppState>,
    _o: SameOrigin,
    who: Identity,
    req: Result<Json<ReadBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    who.require_owner("reads the inbox")?;
    let b = body(req)?;
    enum Mark {
        Ids(Vec<String>),
        All(InboxQuery),
    }
    let mark = match (b.ids, b.all) {
        (Some(ids), false) => {
            if b.filter.is_some() || b.upto.is_some() {
                return Err(invalid("filter and upto go with all"));
            }
            if ids.len() > MAX_MARK_IDS {
                return Err(invalid(format!("at most {MAX_MARK_IDS} ids")));
            }
            if let Some(bad) = ids.iter().find(|i| !is_item_id(i)) {
                return Err(invalid(format!("'{bad}' is not an item ID")));
            }
            Mark::Ids(ids)
        }
        (None, true) => {
            let mut q = b.filter.unwrap_or_default().query()?;
            q.upto = b.upto;
            Mark::All(q)
        }
        _ => return Err(invalid("give ids, or all: true")),
    };
    let out = s
        .store_call(move |db| {
            let marked = match &mark {
                Mark::Ids(ids) => db.inbox_mark(ids, true)?,
                Mark::All(q) => db.inbox_mark_all(q)?,
            };
            Ok(json!({"marked": marked, "unread": db.inbox_unread()?}))
        })
        .await?;
    Ok(Json(out))
}
