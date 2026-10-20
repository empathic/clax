//! The inbox (spec 2026-10-06-agent-questions-and-inbox §8.1), one per
//! person (spec 2026-10-08-multi-agent-multi-person §5.3, §18): list and
//! search items, the unread summary, one item, and read marks, each over
//! the caller's own items ([`clax_http::views::inbox::recipient_of`]): the owner's
//! for the owner, a connected person's for their browser. Another
//! person's item is 404. Every route answers 403 `forbidden` to anyone
//! else (a LAN viewer, a revoked person), refuses agents, and keeps the
//! viewer routes' origin rules. The changes the marks make reach clients
//! through the inbox listener ([`clax_http::views::inbox::listen`]); the responses
//! carry the unread count after them.

use super::artifacts::{body, path};
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::{ArtifactId, CoreError};
use clax_http::agent::DeniedToGrants;
use clax_http::error::ApiError;
use clax_http::identity::Identity;
use clax_http::state::AppState;
use clax_http::viewer::SameOrigin;
use clax_http::views::inbox::{recipient_of, view as inbox_view, views as inbox_views};
use clax_store::inbox::{
    Agent, InboxQuery, Kind, MAX_GROUP_ROWS, MAX_GROUPS, MAX_PAGE, ReadFilter, is_item_id,
    stored_time,
};
use clax_store::questions::ListStatus;
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
/// Items each group of `group=artifact` lists when `rows` is not given.
pub const GROUP_ROWS: u32 = 3;
/// The `artifact` filter's value for the items about no artifact.
pub const NO_ARTIFACT: &str = "none";

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
    /// Tags, each by ID or name: items about an artifact carrying every one.
    #[serde(default)]
    tag: Option<Tags>,
}

/// One tag or several, as a request body names them.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Tags {
    One(String),
    List(Vec<String>),
}

impl Tags {
    fn into_vec(self) -> Vec<String> {
        match self {
            Tags::One(t) => vec![t],
            Tags::List(v) => v,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Kinds {
    Comma(String),
    List(Vec<String>),
}

impl Filter {
    /// The query these filters make over `recipient`'s items. 400
    /// `invalid_query` for an unknown kind, an artifact that is neither an
    /// artifact ID nor `none` (the items about no artifact), or a date or
    /// time that is not RFC 3339 (`since`/`until` are normalised to the
    /// stored form). Its tags are names until [`with_tags`] makes them IDs.
    fn query(self, recipient: Option<String>) -> Result<InboxQuery, ApiError> {
        let names: Vec<String> = match self.kind {
            None => Vec::new(),
            Some(Kinds::Comma(s)) => s.split(',').map(str::to_string).collect(),
            Some(Kinds::List(v)) => v,
        };
        let mut kinds = Vec::new();
        for k in names.iter().map(|k| k.trim()).filter(|k| !k.is_empty()) {
            kinds.push(Kind::parse(k).ok_or_else(|| {
                invalid(format!(
                    "'{k}' is not a kind: reply, version, published, question, finished, \
                     approval, connection or ended"
                ))
            })?);
        }
        let no_artifact = self.artifact.as_deref() == Some(NO_ARTIFACT);
        let artifact = match self
            .artifact
            .as_deref()
            .filter(|a| !a.is_empty() && !no_artifact)
        {
            Some(a) => Some(
                ArtifactId::parse(a)
                    .map_err(|_| invalid(format!("'{a}' is not an artifact ID or {NO_ARTIFACT}")))?
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
            recipient,
            text: self.q.filter(|q| !q.trim().is_empty()),
            kinds,
            artifact,
            no_artifact,
            agent,
            since: time(self.since, false)?,
            until: time(self.until, true)?,
            tags: tag_refs(self.tag.map(Tags::into_vec).unwrap_or_default()),
            ..InboxQuery::default()
        })
    }
}

/// `refs` trimmed, blanks left out.
fn tag_refs(refs: Vec<String>) -> Vec<String> {
    refs.into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `q` with its tags, names or IDs as the request gave them, made tag IDs
/// ([`clax_store::Store::tag_filter_ids`]: a name no tag has matches
/// nothing).
fn with_tags(db: &clax_store::Store, mut q: InboxQuery) -> clax_core::Result<InboxQuery> {
    if !q.tags.is_empty() {
        q.tags = db.tag_filter_ids(&q.tags)?;
    }
    Ok(q)
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
    /// `agent`: the agent strip instead of items; `artifact`: the items
    /// grouped by artifact.
    #[serde(default)]
    group: Option<String>,
    /// `group=artifact`: items listed in each group.
    #[serde(default)]
    rows: Option<String>,
}

impl ListQuery {
    /// Whether any parameter but `group` is given.
    fn filters(&self) -> bool {
        [
            &self.q,
            &self.kind,
            &self.artifact,
            &self.agent,
            &self.since,
            &self.until,
            &self.read,
            &self.before,
            &self.limit,
            &self.rows,
        ]
        .iter()
        .any(|p| p.is_some())
    }
}

/// `strip`, the agent strip, as `GET /api/inbox?group=agent` answers it
/// to `me` (`None`: the owner): each agent with its person's public ID
/// (null for the owner's agents) and name, and `own` for the caller's own
/// person's agents (spec 2026-10-08 §11 "Display").
fn strip_view(
    db: &clax_store::Store,
    strip: &[clax_store::inbox::AgentGroup],
    me: Option<&str>,
) -> clax_core::Result<Value> {
    let mut names = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(strip.len());
    for g in strip {
        let person_name = match names.get(&g.person) {
            Some(n) => Clone::clone(n),
            None => {
                let n = db.agent_person(g.person.as_deref())?.name;
                names.insert(g.person.clone(), n.clone());
                n
            }
        };
        let mut v = json!({
            "handle": g.handle,
            "name": g.name,
            "harness": g.harness,
            "person": g.person,
            "person_name": person_name,
            "own": g.person.as_deref() == me,
            "live": g.live,
                "unread": g.unread,
            "blocked": g.blocked_since.as_ref().map(|since| json!({
                "since": since,
                "questions": g.questions,
                "approvals": g.approvals,
            })),
        });
        // Another Clax's agent: whose Clax it relays from.
        if g.relay {
            v["via"] = json!(clax_store::attention::relay_via(&person_name));
        }
        out.push(v);
    }
    Ok(json!(out))
}

/// Where the rest of a group begins, as `group=artifact` gives it:
/// `u<seq>` (more unread, before `seq`), `r<seq>` (more read, before
/// `seq`) or `r` (its read items, from the newest).
fn group_cursor(n: clax_store::inbox::GroupNext) -> String {
    use clax_store::inbox::GroupNext;
    match n {
        GroupNext::Unread(s) => format!("u{s}"),
        GroupNext::Read(Some(s)) => format!("r{s}"),
        GroupNext::Read(None) => "r".into(),
    }
}

fn query_of<T>(q: Result<Query<T>, QueryRejection>) -> Result<T, ApiError> {
    q.map(|Query(q)| q).map_err(|e| invalid(e.body_text()))
}

/// `GET /api/inbox?q=&kind=<k,…>&artifact=<aid>&agent=<harness|handle>&tag=<id or name>…&since=&until=&read=unread|read|all&before=<cursor>&limit=`
/// (`tag` repeatable: items about an artifact carrying every tag named;
/// a name no tag has matches nothing),
/// or `GET /api/inbox?group=agent` (no other parameter; spec
/// 2026-10-08-multi-agent-multi-person §13) → `{agents, unread}`: the agent
/// strip, one entry per agent with unread items for the caller or an open
/// question or approval request waiting on them (`{handle, name, harness,
/// person, person_name, own, live, unread, blocked: {since, questions,
/// approvals} | null}`), blocked
/// ones first, longest blocked first, then by newest unread item; another
/// `group` is 400 `invalid_query`.
///
/// With `group=artifact` (and any of the parameters above, `rows=` too) →
/// `{groups, next_cursor, unread, total?}`: the matches grouped by the
/// artifact they are about, one group (`artifact: null`) for those about
/// none, newest group (by its newest match) first. Each group is
/// `{artifact_id, artifact, total, unread, newest_at, items,
/// next_cursor}`: its key (the artifact's ID, null for none), the
/// artifact as its items name it, how many match and how many of those are
/// unread (every match counted, not only those listed), its newest match's
/// time, its first `rows` rows (default 3, 1 to 20): its unread matches
/// newest first, then its read ones, and where the rest begins: `u<seq>`
/// (more unread: the plain list with the same parameters, `read=unread`,
/// `artifact=<its ID>` or `none`, and `before=<seq>`), `r<seq>` (more read:
/// the same with `read=read`) or `r` (its read ones from the newest), or
/// null. `limit` is groups per page (default 20, at most 100)
/// and `before` the top-level `next_cursor`. `total` is as for the plain
/// list. `rows` without `group=artifact` is 400 `invalid_query`.
///
/// (the owner, or a connected person: their own items) → `{items,
/// next_cursor, unread, total?}`: items newest first,
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
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    q: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let hosted = s.hosted.is_some();
    let q = query_of(q)?;
    let tags = super::tags::tag_params(raw.as_deref());
    match q.group.as_deref() {
        None => {}
        Some("artifact") => {}
        Some("agent") => {
            if q.filters() || !tags.is_empty() {
                return Err(invalid("group=agent takes no other parameter"));
            }
            let out = s
                .store_call(move |db| {
                    let strip = db.inbox_agents(me.as_deref())?;
                    Ok(json!({
                        "agents": strip_view(db, &strip, me.as_deref())?,
                        "unread": db.inbox_unread(me.as_deref())?,
                    }))
                })
                .await?;
            return Ok(Json(out));
        }
        Some(g) => {
            return Err(invalid(format!(
                "'{g}' is not a grouping: agent or artifact"
            )));
        }
    }
    let grouped = q.group.is_some();
    let rows = match (grouped, q.rows.as_deref().filter(|r| !r.is_empty())) {
        (false, None) => 0,
        (false, Some(_)) => return Err(invalid("rows goes with group=artifact")),
        (true, None) => GROUP_ROWS,
        (true, Some(r)) => r
            .parse::<u32>()
            .map_err(|_| invalid(format!("rows is 1 to {MAX_GROUP_ROWS}")))?
            .clamp(1, MAX_GROUP_ROWS),
    };
    let most = if grouped { MAX_GROUPS } else { MAX_PAGE };
    let mut iq = Filter {
        q: q.q,
        kind: q.kind.map(Kinds::Comma),
        artifact: q.artifact,
        agent: q.agent,
        since: q.since,
        until: q.until,
        tag: Some(Tags::List(tags)),
    }
    .query(me.clone())?;
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
            .map_err(|_| invalid(format!("limit is 1 to {most}")))?
            .clamp(1, most),
    };
    let filtered = iq.text.is_some()
        || !iq.kinds.is_empty()
        || iq.artifact.is_some()
        || iq.no_artifact
        || iq.agent.is_some()
        || iq.since.is_some()
        || iq.until.is_some()
        || !iq.tags.is_empty()
        || iq.read != ReadFilter::All;
    // A filtered list (a search included) also counts what matches, and a
    // grouping groups every match; each reads up to the whole inbox, so it
    // waits in the bulk lane. An unfiltered page reads one page and the
    // unread count.
    let job = move |db: &clax_store::Store| {
        let mut iq = with_tags(db, iq)?;
        if iq.text.is_some() {
            iq.text_withheld_at =
                clax_http::views::inbox::text_withheld_at(db, me.as_deref(), hosted)?;
        }
        let mut out = if grouped {
            let (groups, next) = db.inbox_groups(&iq, rows)?;
            let mut views = Vec::with_capacity(groups.len());
            for g in &groups {
                let items = inbox_views(db, &g.items, hosted)?;
                // The artifact as its items name it (a deleted one by its ID alone).
                let artifact = match &g.artifact_id {
                    Some(_) => items.first().map_or(Value::Null, |i| i["artifact"].clone()),
                    None => Value::Null,
                };
                views.push(json!({
                    "artifact_id": g.artifact_id,
                    "artifact": artifact,
                    "total": g.total,
                    "unread": g.unread,
                    "newest_at": g.newest_at,
                    "items": items,
                    "next_cursor": g.next.map(group_cursor),
                }));
            }
            json!({"groups": views, "next_cursor": next.map(|n| n.to_string())})
        } else {
            let (rows, next) = db.inbox_list(&iq)?;
            json!({
                "items": inbox_views(db, &rows, hosted)?,
                "next_cursor": next.map(|n| n.to_string()),
            })
        };
        out["unread"] = json!(db.inbox_unread(me.as_deref())?);
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
    let out = if filtered || grouped {
        s.store_call_bulk(job).await?
    } else {
        s.store_call(job).await?
    };
    Ok(Json(out))
}

/// `GET /api/inbox/summary` (the owner, or a connected person: their own)
/// → `{unread, questions, latest}`: the unread count, the open questions'
/// views oldest first, and the five newest unread items other than
/// questions (connection requests among them), for the gallery and the
/// top bar. Each part is read on its
/// own, one just after another. With `?tag=<id or name>` (repeatable),
/// `questions` and `latest` are only those about an artifact carrying every
/// tag named (match all); `unread` stays the whole inbox's.
pub async fn summary(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let hosted = s.hosted.is_some();
    let tags = super::tags::tag_params(raw.as_deref());
    let out = s
        .store_call(move |db| {
            let tag_ids = db.tag_filter_ids(&tags)?;
            let (mut open, _) =
                db.list_questions(me.as_deref(), ListStatus::Open, SUMMARY_QUESTIONS)?;
            if !tag_ids.is_empty() {
                let mut carrying = std::collections::HashMap::new();
                open.retain(|q| {
                    let Some(aid) = &q.artifact_id else {
                        return false;
                    };
                    *carrying.entry(aid.clone()).or_insert_with(|| {
                        clax_core::ArtifactId::parse(aid)
                            .ok()
                            .and_then(|id| db.get_artifact(&id).ok().flatten())
                            .is_some_and(|a| clax_store::tags::carries_all(&a.tags, &tag_ids))
                    })
                });
            }
            let questions = open
                .iter()
                .map(|q| clax_http::views::question::view(db, q))
                .collect::<clax_core::Result<Vec<_>>>()?;
            let (rows, _) = db.inbox_list(&InboxQuery {
                recipient: me.clone(),
                kinds: vec![
                    Kind::Reply,
                    Kind::Version,
                    Kind::Published,
                    Kind::Finished,
                    Kind::Connection,
                    Kind::Approval,
                    Kind::Ended,
                    Kind::Push,
                ],
                read: ReadFilter::Unread,
                limit: SUMMARY_LATEST,
                tags: tag_ids,
                ..InboxQuery::default()
            })?;
            Ok(json!({
                "unread": db.inbox_unread(me.as_deref())?,
                "questions": questions,
                "latest": inbox_views(db, &rows, hosted)?,
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

/// `GET /api/inbox/<id>` (the owner, or a connected person) → `{item}`;
/// 404 when the caller has none with that ID.
pub async fn get_one(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let hosted = s.hosted.is_some();
    let id = item_id(p)?;
    let item = s
        .store_call(move |db| {
            let i = db
                .inbox_item(&id, me.as_deref())?
                .ok_or(CoreError::NotFound)?;
            inbox_view(db, &i, hosted)
        })
        .await?;
    Ok(Json(json!({"item": item})))
}

/// Marks `me`'s item `id` read or unread → `{item, unread}`.
async fn mark_one(
    s: AppState,
    me: Option<String>,
    id: String,
    read: bool,
) -> Result<Json<Value>, ApiError> {
    let hosted = s.hosted.is_some();
    let out = s
        .store_call(move |db| {
            let me = me.as_deref();
            db.inbox_item(&id, me)?.ok_or(CoreError::NotFound)?;
            db.inbox_mark(std::slice::from_ref(&id), read, me)?;
            let i = db.inbox_item(&id, me)?.ok_or(CoreError::NotFound)?;
            Ok(json!({"item": inbox_view(db, &i, hosted)?, "unread": db.inbox_unread(me)?}))
        })
        .await?;
    Ok(Json(out))
}

/// `POST /api/inbox/<id>/read` (the owner, or a connected person): its
/// person opened the item → `{item, unread}`; 404 when the caller has none
/// with that ID. Idempotent.
pub async fn read_one(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    mark_one(s, me, item_id(p)?, true).await
}

/// `POST /api/inbox/<id>/unread` (the owner, or a connected person) →
/// `{item, unread}`; 404 when the caller has none with that ID.
pub async fn unread_one(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    mark_one(s, me, item_id(p)?, false).await
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
    /// Items to leave unread (at most 500 IDs): those the client shows
    /// apart from what it marks, such as cards pinned above a group.
    #[serde(default)]
    except: Option<Vec<String>>,
}

/// `POST /api/inbox/read` (the owner, or a connected person: their own
/// items; another's IDs mark nothing), body `{ids}` (at most 500 item IDs) or
/// `{all: true, filter?: {q, kind, artifact, agent, since, until, tag}, upto?,
/// except?}` (every unread item matching the filter, up to item `seq`
/// `upto`, the newest the client showed, but the item IDs `except`, at
/// most 500) → `{marked, unread}`. 400 `invalid_query` for
/// neither or both, too many IDs, or a bad filter.
pub async fn read_many(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    req: Result<Json<ReadBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    let b = body(req)?;
    enum Mark {
        Ids(Vec<String>),
        // Boxed: a query is far larger than a list of IDs.
        All(Box<InboxQuery>),
    }
    let mark = match (b.ids, b.all) {
        (Some(ids), false) => {
            if b.filter.is_some() || b.upto.is_some() || b.except.is_some() {
                return Err(invalid("filter, upto and except go with all"));
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
            let mut q = b.filter.unwrap_or_default().query(me.clone())?;
            q.upto = b.upto;
            let except = b.except.unwrap_or_default();
            if except.len() > MAX_MARK_IDS {
                return Err(invalid(format!("at most {MAX_MARK_IDS} except IDs")));
            }
            if let Some(bad) = except.iter().find(|i| !is_item_id(i)) {
                return Err(invalid(format!("'{bad}' is not an item ID")));
            }
            q.except = except;
            Mark::All(Box::new(q))
        }
        _ => return Err(invalid("give ids, or all: true")),
    };
    let hosted = s.hosted.is_some();
    let out = s
        .store_call(move |db| {
            let marked = match mark {
                Mark::Ids(ids) => db.inbox_mark(&ids, true, me.as_deref())?,
                Mark::All(q) => {
                    let mut q = with_tags(db, *q)?;
                    if q.text.is_some() {
                        q.text_withheld_at =
                            clax_http::views::inbox::text_withheld_at(db, me.as_deref(), hosted)?;
                    }
                    db.inbox_mark_all(&q)?
                }
            };
            Ok(json!({"marked": marked, "unread": db.inbox_unread(me.as_deref())?}))
        })
        .await?;
    Ok(Json(out))
}
