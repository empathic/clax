//! The owner's inbox (spec 2026-10-06-agent-questions-and-inbox-design §7):
//! items that reference what agents sent back, made inside their sources'
//! transactions; read marks; and search through a contentless FTS5 index.
//! Items are never deleted.
//!
//! Every list is newest first by `seq` and always bounded by a `seq` cursor
//! (`i64::MAX` on the first page), so each page is an index range: the
//! unread list walks `inbox_unread`, a filter its `inbox_by_*` index, a
//! search the FTS index in rowid order, and the rest the rowid itself.

use super::Store;
use super::questions::{QuestionRow, Status};
use crate::working::Ended;
use crate::{CoreError, Result, new_ulid};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::Serialize;
use serde_json::Value;

/// Items per page when a query gives no limit.
pub const DEFAULT_PAGE: u32 = 50;
/// Most items one page returns.
pub const MAX_PAGE: u32 = 200;
/// Most search terms a query uses; later ones are ignored.
pub const MAX_TERMS: usize = 16;

/// What an item is about (spec §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Reply,
    Version,
    Published,
    Question,
    Finished,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Reply,
        Kind::Version,
        Kind::Published,
        Kind::Question,
        Kind::Finished,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Reply => "reply",
            Kind::Version => "version",
            Kind::Published => "published",
            Kind::Question => "question",
            Kind::Finished => "finished",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// One stored item: its kind, the keys of its source, the agent that sent
/// it, and when it was made and read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ItemRow {
    /// Order and cursor: higher is newer.
    pub seq: i64,
    /// The public ID: a ULID, or `b` and 24 hex digits for an item filled
    /// in from the history by migration 21.
    pub id: String,
    pub kind: Kind,
    pub artifact_id: Option<String>,
    pub thread_id: Option<String>,
    pub comment_id: Option<String>,
    pub version_n: Option<u32>,
    pub question_id: Option<String>,
    pub session_id: Option<String>,
    pub harness: Option<String>,
    /// `finished` only: `{message, thread_ids}`.
    pub detail: Option<Value>,
    pub created_at: String,
    pub read_at: Option<String>,
}

/// Which agent's items a query keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agent {
    /// Every session of a harness (`claude`).
    Harness(String),
    /// The one session with this agent handle (`a_…`).
    Handle(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadFilter {
    Unread,
    Read,
    #[default]
    All,
}

/// A list, count or mark-all query. Every filter given must hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxQuery {
    /// Search text: each term (separated by whitespace or control
    /// characters) must prefix a word of the item's index entry
    /// ([`fts_query`]). No terms: no text filter.
    pub text: Option<String>,
    /// Any of these kinds; empty for all.
    pub kinds: Vec<Kind>,
    pub artifact: Option<String>,
    pub agent: Option<Agent>,
    /// Made at or after this time, compared as stored text: RFC 3339 UTC
    /// with milliseconds and `Z` (as [`Store::now`] writes).
    pub since: Option<String>,
    /// Made before this time (as `since`).
    pub until: Option<String>,
    pub read: ReadFilter,
    /// Items older than this `seq` (the previous page's cursor).
    pub before: Option<i64>,
    /// Items per page: 0 for [`DEFAULT_PAGE`], at most [`MAX_PAGE`].
    pub limit: u32,
    /// Mark-all only: items up to this `seq` (the newest the owner was
    /// shown), so an item made since is not marked read unseen.
    pub upto: Option<i64>,
}

/// One change to `inbox_items` in a committed transaction: the item `seq`
/// was made (`made`) or updated (its read mark, or its page after a thread
/// moved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboxChange {
    pub seq: i64,
    pub made: bool,
}

/// The listener [`Store::set_inbox_listener`] installs.
pub type InboxListener = Box<dyn Fn(Vec<InboxChange>) + Send + Sync>;

/// The FTS5 query for search text `text`: `"term"*` for each term holding
/// a letter or digit (at most [`MAX_TERMS`]), quotes doubled, so every term
/// is required, a prefix, and taken as text (no input is FTS5 syntax).
/// Terms are separated by whitespace and by control characters (FTS5 stops
/// reading a query at a NUL). `None` without such terms.
pub fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .take(MAX_TERMS)
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

macro_rules! columns {
    () => {
        "i.seq, i.id, i.kind, i.artifact_id, i.thread_id, i.comment_id, i.version_n,
         i.question_id, i.session_id, i.harness, i.detail_json, i.created_at, i.read_at"
    };
}
const COLUMNS: &str = columns!();

fn corrupt(column: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("inbox_items.{column} is not a value this store writes").into(),
    )
}

fn row(r: &Row<'_>) -> rusqlite::Result<ItemRow> {
    let detail: Option<String> = r.get(10)?;
    Ok(ItemRow {
        seq: r.get(0)?,
        id: r.get(1)?,
        kind: Kind::parse(r.get_ref(2)?.as_str()?).ok_or_else(|| corrupt("kind"))?,
        artifact_id: r.get(3)?,
        thread_id: r.get(4)?,
        comment_id: r.get(5)?,
        version_n: r.get(6)?,
        question_id: r.get(7)?,
        session_id: r.get(8)?,
        harness: r.get(9)?,
        detail: detail
            .map(|d| serde_json::from_str(&d))
            .transpose()
            .map_err(|_| corrupt("detail_json"))?,
        created_at: r.get(11)?,
        read_at: r.get(12)?,
    })
}

/// A query's SQL, its positional parameters, the index that drives it, and
/// whether it searches text.
pub(super) struct Built {
    pub sql: String,
    pub args: Vec<Sql>,
    /// Read by the query-plan checks.
    #[cfg_attr(not(test), allow(dead_code))]
    pub driver: Driver,
    pub searched: bool,
}

/// What the built statement does with the matching items.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A page, newest first, one row past the limit.
    Page,
    /// At most `cap` matches, counted.
    Count(u32),
    /// Marks the unread matches read at the time given.
    MarkAll,
}

/// The index that drives a statement: the one filter it walks, in `seq`
/// order; every other filter is a residual term (`+`), so the plan never
/// depends on the planner's statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Driver {
    /// The FTS index, in rowid order.
    Text,
    /// `inbox_unread`: the unread items only.
    Unread,
    Artifact,
    Handle,
    Kind,
    Harness,
    /// The rowid between the `seq` bounds `inbox_by_created` gives the dates.
    Dates,
    /// The rowid, newest first.
    Seq,
}

impl Driver {
    /// The index name (or plan words) a plan driven by it names.
    #[cfg(test)]
    pub(super) fn plan_word(self) -> &'static str {
        match self {
            Driver::Text => "VIRTUAL TABLE INDEX",
            Driver::Unread => "inbox_unread",
            Driver::Artifact => "inbox_by_artifact",
            Driver::Handle => "inbox_by_session",
            Driver::Kind => "inbox_by_kind",
            Driver::Harness => "inbox_by_harness",
            Driver::Dates => "inbox_by_created",
            Driver::Seq => "INTEGER PRIMARY KEY (rowid<?)",
        }
    }
}

/// `q`'s kinds, sorted and without repeats.
fn kinds_of(q: &InboxQuery) -> Vec<Kind> {
    let mut k = q.kinds.clone();
    k.sort_by_key(|k| k.as_str());
    k.dedup();
    k
}

/// Which filter drives `q` in `shape`: the search text; else the unread
/// filter (a mark-all always marks unread items); else the most selective
/// equality filter; else the date range; else the rowid.
fn driver(q: &InboxQuery, text: bool, kinds: usize, shape: Shape) -> Driver {
    if text {
        Driver::Text
    } else if shape == Shape::MarkAll || q.read == ReadFilter::Unread {
        Driver::Unread
    } else if q.artifact.is_some() {
        Driver::Artifact
    } else if matches!(q.agent, Some(Agent::Handle(_))) {
        Driver::Handle
    } else if kinds == 1 {
        Driver::Kind
    } else if matches!(q.agent, Some(Agent::Harness(_))) {
        Driver::Harness
    } else if q.since.is_some() || q.until.is_some() {
        Driver::Dates
    } else {
        Driver::Seq
    }
}

/// The `seq` of the first item made at or after `?p` (by `inbox_by_created`),
/// or past every `seq` when there is none.
fn first_seq_at(p: &str) -> String {
    format!(
        "coalesce((SELECT d.seq FROM inbox_items d INDEXED BY inbox_by_created
                    WHERE d.created_at >= {p} ORDER BY d.created_at, d.seq LIMIT 1), {})",
        i64::MAX
    )
}

/// The statement for `q` in `shape` and the index that drives it
/// ([`driver`]). A search drives from the FTS index in rowid order
/// (`CROSS JOIN` keeps it outermost), so its cost follows the matches.
/// Several kinds are always residual: one index cannot give them in `seq`
/// order. A date range driving the walk becomes `seq` bounds (items are
/// made in time order), its dates kept as residual terms.
fn build(q: &InboxQuery, shape: Shape, now: &str) -> Built {
    let mut args: Vec<Sql> = Vec::new();
    let arg = |v: Sql, args: &mut Vec<Sql>| -> String {
        args.push(v);
        format!("?{}", args.len())
    };
    let text = q.text.as_deref().and_then(fts_query);
    let kinds = kinds_of(q);
    let drive = driver(q, text.is_some(), kinds.len(), shape);
    let on = |d: Driver| if drive == d { "" } else { "+" };
    let mut sql = String::new();
    let mut wh: Vec<String> = Vec::new();
    match shape {
        Shape::MarkAll => {
            let p = arg(Sql::Text(now.into()), &mut args);
            sql.push_str(&format!(
                "UPDATE inbox_items AS i SET read_at = {p} WHERE {}i.read_at IS NULL",
                on(Driver::Unread)
            ));
            if let Some(t) = &text {
                let p = arg(Sql::Text(t.clone()), &mut args);
                wh.push(format!(
                    "i.seq IN (SELECT rowid FROM inbox_fts WHERE inbox_fts MATCH {p})"
                ));
            }
            if let Some(u) = q.upto {
                let p = arg(Sql::Integer(u), &mut args);
                wh.push(format!("i.seq <= {p}"));
            }
        }
        Shape::Page | Shape::Count(_) => {
            let what = if shape == Shape::Page { COLUMNS } else { "1" };
            let before = match shape {
                Shape::Page => q.before.unwrap_or(i64::MAX),
                _ => i64::MAX,
            };
            if let Some(t) = &text {
                let m = arg(Sql::Text(t.clone()), &mut args);
                let b = arg(Sql::Integer(before), &mut args);
                sql.push_str(&format!(
                    "SELECT {what} FROM inbox_fts f CROSS JOIN inbox_items i ON i.seq = f.rowid
                     WHERE inbox_fts MATCH {m} AND f.rowid < {b}"
                ));
            } else {
                let b = arg(Sql::Integer(before), &mut args);
                sql.push_str(&format!(
                    "SELECT {what} FROM inbox_items i WHERE i.seq < {b}"
                ));
            }
            match q.read {
                ReadFilter::Unread => wh.push(format!("{}i.read_at IS NULL", on(Driver::Unread))),
                ReadFilter::Read => wh.push("+i.read_at IS NOT NULL".into()),
                ReadFilter::All => {}
            }
        }
    }
    match kinds.as_slice() {
        [] => {}
        [k] => {
            let p = arg(Sql::Text(k.as_str().into()), &mut args);
            wh.push(format!("{}i.kind = {p}", on(Driver::Kind)));
        }
        ks => {
            let ps: Vec<String> = ks
                .iter()
                .map(|k| arg(Sql::Text(k.as_str().into()), &mut args))
                .collect();
            wh.push(format!("+i.kind IN ({})", ps.join(", ")));
        }
    }
    if let Some(a) = &q.artifact {
        let p = arg(Sql::Text(a.clone()), &mut args);
        wh.push(format!("{}i.artifact_id = {p}", on(Driver::Artifact)));
    }
    match &q.agent {
        Some(Agent::Harness(h)) => {
            let p = arg(Sql::Text(h.clone()), &mut args);
            wh.push(format!("{}i.harness = {p}", on(Driver::Harness)));
        }
        Some(Agent::Handle(h)) => {
            let p = arg(Sql::Text(h.clone()), &mut args);
            wh.push(format!(
                "{}i.session_id = (SELECT id FROM sessions WHERE agent_handle = {p})",
                on(Driver::Handle)
            ));
        }
        None => {}
    }
    if let Some(s) = &q.since {
        let p = arg(Sql::Text(s.clone()), &mut args);
        wh.push(format!("+i.created_at >= {p}"));
        if drive == Driver::Dates {
            wh.push(format!("i.seq >= {}", first_seq_at(&p)));
        }
    }
    if let Some(u) = &q.until {
        let p = arg(Sql::Text(u.clone()), &mut args);
        wh.push(format!("+i.created_at < {p}"));
        if drive == Driver::Dates {
            wh.push(format!("i.seq < {}", first_seq_at(&p)));
        }
    }
    for w in &wh {
        sql.push_str(" AND ");
        sql.push_str(w);
    }
    match shape {
        Shape::Page => {
            let order = if text.is_some() { "f.rowid" } else { "i.seq" };
            let p = arg(Sql::Integer(i64::from(page_size(q.limit)) + 1), &mut args);
            sql.push_str(&format!(" ORDER BY {order} DESC LIMIT {p}"));
        }
        Shape::Count(cap) => {
            let p = arg(Sql::Integer(i64::from(cap)), &mut args);
            sql = format!("SELECT count(*) FROM ({sql} LIMIT {p})");
        }
        Shape::MarkAll => {}
    }
    Built {
        sql,
        args,
        driver: drive,
        searched: text.is_some(),
    }
}

fn page_size(limit: u32) -> u32 {
    match limit {
        0 => DEFAULT_PAGE,
        n => n.min(MAX_PAGE),
    }
}

/// An SQL error from a search is the search text's fault (there should be
/// none once [`fts_query`] quoted it, but the FTS5 parser's messages are
/// not all prefixed): `invalid_query`, never an internal error. Other
/// failures (busy, interrupted, I/O) keep their own error.
fn search_error(e: CoreError, searched: bool) -> CoreError {
    match e {
        CoreError::Db(rusqlite::Error::SqliteFailure(f, _))
            if searched && f.code == rusqlite::ErrorCode::Unknown =>
        {
            CoreError::invalid("invalid_query", "the search text could not be used")
        }
        e => e,
    }
}

/// The unread count.
pub(super) const UNREAD_COUNT: &str = "SELECT count(*) FROM inbox_items WHERE read_at IS NULL";
pub(super) const BY_ID: &str =
    concat!("SELECT ", columns!(), " FROM inbox_items i WHERE i.id = ?1");
pub(super) const BY_SEQS: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM json_each(?1) j CROSS JOIN inbox_items i ON i.seq = j.value ORDER BY i.seq DESC"
);
pub(super) const MARK_READ: &str = "UPDATE inbox_items SET read_at = ?2
    WHERE id IN (SELECT value FROM json_each(?1)) AND read_at IS NULL";
pub(super) const MARK_UNREAD: &str = "UPDATE inbox_items SET read_at = NULL
    WHERE id IN (SELECT value FROM json_each(?1)) AND read_at IS NOT NULL";
pub(super) const READ_BY_LOOK: &str = "UPDATE inbox_items SET read_at = ?3
    WHERE thread_id = ?1 AND artifact_id = ?2 AND kind = 'reply' AND read_at IS NULL AND created_at <= ?3";
pub(super) const READ_BY_SEEN: &str = "UPDATE inbox_items SET read_at = ?3
    WHERE artifact_id = ?1 AND read_at IS NULL AND
      ((kind = 'version' AND version_n <= ?2) OR (kind IN ('published', 'finished') AND created_at <= ?3))";
/// Touches item `?1`, marking it read at `?2` when `?3` (keeping an earlier read time).
pub(super) const QUESTION_TOUCH: &str = "UPDATE inbox_items
    SET read_at = CASE WHEN ?3 THEN coalesce(read_at, ?2) ELSE read_at END WHERE seq = ?1";
/// The newest item's time (`inbox_by_created`, its last entry).
pub(super) const NEWEST_STAMP: &str = "SELECT max(created_at) FROM inbox_items";
pub(super) const OF_QUESTION: &str = "SELECT seq FROM inbox_items WHERE question_id = ?1";
/// Whether the owner (`?1`, a public ID) is in thread `?2`.
pub(super) const OWNER_IN: &str =
    "SELECT EXISTS (SELECT 1 FROM comments c WHERE c.thread_id = ?2 AND +c.author_public_id = ?1)
         OR EXISTS (SELECT 1 FROM comments c CROSS JOIN mentions m ON m.comment_id = c.id
                     WHERE c.thread_id = ?2 AND +m.public_id = ?1)
         OR EXISTS (SELECT 1 FROM threads t WHERE t.id = ?2 AND t.resolved_by = 'viewer:' || ?1)";
/// Whether the owner (`?2`) wrote a comment on artifact `?1`.
pub(super) const OWNER_COMMENTED: &str =
    "SELECT EXISTS (SELECT 1 FROM threads t CROSS JOIN comments m ON m.thread_id = t.id
                     WHERE t.artifact_id = ?1 AND +m.author_public_id = ?2)";
/// The anchor quotes of the threads version `?2` of `?1` addressed.
pub(super) const ADDRESSED_QUOTES: &str = "SELECT group_concat(CASE WHEN json_valid(t.anchor_json)
                              THEN json_extract(t.anchor_json, '$.quote') END, ' ')
       FROM version_threads vt CROSS JOIN threads t ON t.id = vt.thread_id
      WHERE vt.artifact_id = ?1 AND vt.version_n = ?2";
pub(super) const THREAD_MOVED: &str =
    "UPDATE inbox_items SET artifact_id = ?2 WHERE thread_id = ?1";

/// Each list, count and mark-all shape with representative parameters,
/// built as [`Store::inbox_list`], [`Store::inbox_count`] and
/// [`Store::inbox_mark_all`] build them, for the query-plan checks.
#[cfg(test)]
pub(super) fn shapes() -> Vec<(String, Built)> {
    let now = "2026-01-01T00:00:00.000Z";
    let base = InboxQuery::default();
    let named: Vec<(&str, InboxQuery)> = vec![
        (
            "UNREAD_PAGE",
            InboxQuery {
                read: ReadFilter::Unread,
                ..base.clone()
            },
        ),
        ("ALL_PAGE", base.clone()),
        (
            "READ_PAGE",
            InboxQuery {
                read: ReadFilter::Read,
                before: Some(4000),
                ..base.clone()
            },
        ),
        (
            "BY_ARTIFACT_PAGE",
            InboxQuery {
                artifact: Some("art0007".into()),
                ..base.clone()
            },
        ),
        (
            "BY_KIND_PAGE",
            InboxQuery {
                kinds: vec![Kind::Version],
                ..base.clone()
            },
        ),
        (
            "BY_KINDS_PAGE",
            InboxQuery {
                kinds: vec![Kind::Version, Kind::Reply],
                ..base.clone()
            },
        ),
        (
            "BY_HARNESS_PAGE",
            InboxQuery {
                agent: Some(Agent::Harness("pi".into())),
                ..base.clone()
            },
        ),
        (
            "BY_HANDLE_PAGE",
            InboxQuery {
                agent: Some(Agent::Handle("a_3".into())),
                ..base.clone()
            },
        ),
        (
            "BY_DATES_PAGE",
            InboxQuery {
                since: Some("2026-01-01T00:00:10.000Z".into()),
                until: Some("2026-01-01T00:00:20.000Z".into()),
                ..base.clone()
            },
        ),
        (
            "SINCE_PAGE",
            InboxQuery {
                since: Some("2026-01-01T00:00:10.000Z".into()),
                ..base.clone()
            },
        ),
        (
            "UNTIL_PAGE",
            InboxQuery {
                until: Some("2026-01-01T00:00:20.000Z".into()),
                before: Some(5000),
                ..base.clone()
            },
        ),
        (
            "UPTO_MARK_ALL",
            InboxQuery {
                upto: Some(5000),
                artifact: Some("art0007".into()),
                ..base.clone()
            },
        ),
        (
            "TEXT_PAGE",
            InboxQuery {
                text: Some("blue head".into()),
                ..base.clone()
            },
        ),
        (
            "TEXT_UNREAD_ARTIFACT_PAGE",
            InboxQuery {
                text: Some("blue".into()),
                read: ReadFilter::Unread,
                artifact: Some("art0007".into()),
                ..base.clone()
            },
        ),
        (
            "UNREAD_ARTIFACT_KIND_PAGE",
            InboxQuery {
                read: ReadFilter::Unread,
                artifact: Some("art0007".into()),
                kinds: vec![Kind::Reply],
                before: Some(3000),
                ..base.clone()
            },
        ),
    ];
    let mut out = Vec::new();
    for (name, q) in named {
        for (suffix, shape) in [
            ("", Shape::Page),
            (" count", Shape::Count(10_001)),
            (" mark all", Shape::MarkAll),
        ] {
            out.push((format!("{name}{suffix}"), build(&q, shape, now)));
        }
    }
    out
}

/// The owner's public ID, if there is an owner.
fn owner_pid(c: &Connection) -> Result<Option<String>> {
    Ok(
        c.query_row("SELECT public_id FROM viewers WHERE owner = 1", [], |r| {
            r.get(0)
        })
        .optional()?,
    )
}

/// Whether viewer `viewer_id` (its cookie ID) is the owner.
fn is_owner_row(c: &Connection, viewer_id: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT owner FROM viewers WHERE id = ?1",
        params![viewer_id],
        |r| r.get::<_, bool>(0),
    )
    .optional()?
    .unwrap_or(false))
}

/// Whether the owner (`pid`) is in thread `tid` (main spec §10
/// "Participants"): wrote a comment in it, is mentioned in it, or resolved
/// it. The unary `+` keeps the per-thread lookups on `comments_by_thread`,
/// as the attention query does.
fn owner_in(c: &Connection, pid: &str, tid: &str) -> Result<bool> {
    Ok(c.query_row(OWNER_IN, params![pid, tid], |r| r.get(0))?)
}

struct New<'a> {
    kind: Kind,
    key: String,
    artifact_id: Option<&'a str>,
    thread_id: Option<&'a str>,
    comment_id: Option<&'a str>,
    version_n: Option<u32>,
    question_id: Option<&'a str>,
    session_id: Option<&'a str>,
    /// The harness, when the caller knows it; else read from the session.
    harness: Option<&'a str>,
    detail: Option<Value>,
    /// The kind's own search text (§7.4).
    text: String,
}

fn harness_of(c: &Connection, sid: Option<&str>) -> Result<Option<String>> {
    match sid {
        Some(s) => Ok(c
            .query_row(
                "SELECT harness FROM sessions WHERE id = ?1",
                params![s],
                |r| r.get(0),
            )
            .optional()?),
        None => Ok(None),
    }
}

fn title_of(c: &Connection, aid: Option<&str>) -> Result<String> {
    match aid {
        Some(a) => Ok(c
            .query_row(
                "SELECT title FROM artifacts WHERE id = ?1",
                params![a],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default()),
        None => Ok(String::new()),
    }
}

/// An item's index entry: the artifact's title now, the harness, and the
/// kind's text.
fn entry(c: &Connection, aid: Option<&str>, harness: Option<&str>, text: &str) -> Result<String> {
    Ok(format!(
        "{} {} {text}",
        title_of(c, aid)?,
        harness.unwrap_or_default()
    ))
}

/// Inserts the item and its index entry unless its key exists; its `seq`.
fn insert(c: &Connection, n: New<'_>) -> Result<Option<i64>> {
    insert_at(c, n, &Store::now())
}

/// The time to stamp a new item with: `now`, or the newest item's time when
/// the clock stepped back, so stamps never go backwards and follow `seq`
/// (date pages walk the `seq` range their dates give).
fn stamp(c: &Connection, now: &str) -> Result<String> {
    let newest: Option<String> = c.query_row(NEWEST_STAMP, [], |r| r.get(0))?;
    Ok(match newest {
        Some(t) if t.as_str() > now => t,
        _ => now.to_string(),
    })
}

/// [`insert`] with the clock reading `now`.
fn insert_at(c: &Connection, n: New<'_>, now: &str) -> Result<Option<i64>> {
    let created_at = stamp(c, now)?;
    let harness = match n.harness {
        Some(h) => Some(h.to_string()),
        None => harness_of(c, n.session_id)?,
    };
    let changed = c.execute(
        "INSERT OR IGNORE INTO inbox_items (id, kind, key, artifact_id, thread_id, comment_id, version_n,
            question_id, session_id, harness, detail_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            new_ulid(),
            n.kind.as_str(),
            n.key,
            n.artifact_id,
            n.thread_id,
            n.comment_id,
            n.version_n,
            n.question_id,
            n.session_id,
            harness,
            n.detail.map(|d| d.to_string()),
            created_at
        ],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    let seq = c.last_insert_rowid();
    let text = entry(c, n.artifact_id, harness.as_deref(), &n.text)?;
    c.execute(
        "INSERT INTO inbox_fts (rowid, text) VALUES (?1, ?2)",
        params![seq, text],
    )?;
    Ok(Some(seq))
}

/// An agent comment `cid` on thread `tid` of artifact `aid`: a `reply`
/// item when the owner is in the thread.
pub(crate) fn note_reply(
    c: &Connection,
    cid: &str,
    tid: &str,
    aid: &str,
    sid: Option<&str>,
    body: &str,
) -> Result<Option<i64>> {
    let Some(pid) = owner_pid(c)? else {
        return Ok(None);
    };
    if !owner_in(c, &pid, tid)? {
        return Ok(None);
    }
    insert(
        c,
        New {
            kind: Kind::Reply,
            key: format!("reply:{cid}"),
            artifact_id: Some(aid),
            thread_id: Some(tid),
            comment_id: Some(cid),
            version_n: None,
            question_id: None,
            session_id: sid,
            harness: None,
            detail: None,
            text: body.into(),
        },
    )
}

/// Version `n` of `aid` by session `sid`: `published` for the first version
/// of an artifact (not a live page) a session created; for a later one,
/// `version` when the owner has written a comment on `aid`, indexed by its
/// note and the quotes of the threads it addressed. Call after the version,
/// its links and the artifact's new title are written.
pub(crate) fn note_version(
    c: &Connection,
    aid: &str,
    n: u32,
    sid: Option<&str>,
    note: Option<&str>,
) -> Result<Option<i64>> {
    let Some(sid) = sid else { return Ok(None) };
    if n == 1 {
        let (kind, description): (String, Option<String>) = c.query_row(
            "SELECT kind, description FROM artifacts WHERE id = ?1",
            params![aid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if kind != "html" {
            return Ok(None);
        }
        return insert(
            c,
            New {
                kind: Kind::Published,
                key: format!("published:{aid}"),
                artifact_id: Some(aid),
                thread_id: None,
                comment_id: None,
                version_n: Some(1),
                question_id: None,
                session_id: Some(sid),
                harness: None,
                detail: None,
                text: description.unwrap_or_default(),
            },
        );
    }
    let Some(pid) = owner_pid(c)? else {
        return Ok(None);
    };
    let commented: bool = c.query_row(OWNER_COMMENTED, params![aid, pid], |r| r.get(0))?;
    if !commented {
        return Ok(None);
    }
    let quotes: Option<String> = c.query_row(ADDRESSED_QUOTES, params![aid, n], |r| r.get(0))?;
    let text = [
        note.unwrap_or_default(),
        quotes.as_deref().unwrap_or_default(),
    ]
    .join(" ");
    insert(
        c,
        New {
            kind: Kind::Version,
            key: format!("version:{aid}:{n}"),
            artifact_id: Some(aid),
            thread_id: None,
            comment_id: None,
            version_n: Some(n),
            question_id: None,
            session_id: Some(sid),
            harness: None,
            detail: None,
            text,
        },
    )
}

/// The questions' text, headers and option labels, for the index.
fn question_text(q: &QuestionRow) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for x in &q.questions {
        parts.push(&x.question);
        parts.push(&x.header);
        parts.extend(x.options.iter().map(|o| o.label.as_str()));
    }
    parts.join(" ")
}

/// [`question_text`] and, once answered, the answers' labels and text.
fn answered_text(q: &QuestionRow) -> String {
    let mut text = question_text(q);
    for a in q.answers.iter().flatten() {
        for s in &a.selected {
            text.push(' ');
            text.push_str(s);
        }
        if let Some(t) = &a.text {
            text.push(' ');
            text.push_str(t);
        }
    }
    text
}

/// A question was created: one `question` item, indexed by its questions,
/// headers and labels.
pub(crate) fn note_question(c: &Connection, q: &QuestionRow) -> Result<Option<i64>> {
    insert(
        c,
        New {
            kind: Kind::Question,
            key: format!("question:{}", q.id),
            artifact_id: q.artifact_id.as_deref(),
            thread_id: None,
            comment_id: None,
            version_n: None,
            question_id: Some(&q.id),
            session_id: Some(&q.session_id),
            harness: None,
            detail: None,
            text: question_text(q),
        },
    )
}

/// A question closed: its index entry gains the answers, and the item is
/// updated (so the listener hears of it); when `seen` (the owner answered,
/// skipped or moved it, or it was answered in the terminal) it is also read.
pub(crate) fn question_changed(c: &Connection, q: &QuestionRow, seen: bool) -> Result<()> {
    let Some(seq) = c
        .query_row(OF_QUESTION, params![q.id], |r| r.get::<_, i64>(0))
        .optional()?
    else {
        return Ok(());
    };
    let (aid, harness): (Option<String>, Option<String>) = c.query_row(
        "SELECT artifact_id, harness FROM inbox_items WHERE seq = ?1",
        params![seq],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let text = entry(c, aid.as_deref(), harness.as_deref(), &answered_text(q))?;
    c.execute("DELETE FROM inbox_fts WHERE rowid = ?1", params![seq])?;
    c.execute(
        "INSERT INTO inbox_fts (rowid, text) VALUES (?1, ?2)",
        params![seq, text],
    )?;
    let read = seen
        && matches!(
            q.status,
            Status::Answered | Status::Declined | Status::Released
        );
    // Always written, so the listener hears that the source changed.
    c.execute(QUESTION_TOUCH, params![seq, Store::now(), read])?;
    Ok(())
}

/// Viewer `viewer_id` looked at `tids` of `aid` at `at`: when it is the
/// owner, their `reply` items made by then are read.
pub(crate) fn read_by_look(
    c: &Connection,
    viewer_id: &str,
    aid: &str,
    tids: &[String],
    at: &str,
) -> Result<()> {
    if !is_owner_row(c, viewer_id)? {
        return Ok(());
    }
    for t in tids {
        c.execute(READ_BY_LOOK, params![t, aid, at])?;
    }
    Ok(())
}

/// Viewer `viewer_id` viewed version `n` of `aid` at `at`: when it is the
/// owner, the artifact's `version` items up to `n`, and its `published` and
/// `finished` items made by then, are read.
pub(crate) fn read_by_seen(
    c: &Connection,
    viewer_id: &str,
    aid: &str,
    n: u32,
    at: &str,
) -> Result<()> {
    if !is_owner_row(c, viewer_id)? {
        return Ok(());
    }
    c.execute(READ_BY_SEEN, params![aid, n, at])?;
    Ok(())
}

/// Thread `tid` moved to page `to_aid`: its items follow it.
pub(crate) fn thread_moved(c: &Connection, tid: &str, to_aid: &str) -> Result<()> {
    c.execute(THREAD_MOVED, params![tid, to_aid])?;
    Ok(())
}

impl Store {
    /// Installs `f`, called with the changes to inbox items after each
    /// committed write transaction that made or updated any, outside the
    /// write turn. Replaces any earlier listener.
    pub fn set_inbox_listener(&self, f: InboxListener) {
        *self.inbox_listener.write().unwrap() = Some(std::sync::Arc::from(f));
    }

    /// A page of items matching `q`, newest first, and the cursor of the
    /// next page (`before`) when there is one.
    ///
    /// # Errors
    /// `invalid_query` when the search text cannot be used.
    pub fn inbox_list(&self, q: &InboxQuery) -> Result<(Vec<ItemRow>, Option<i64>)> {
        let b = build(q, Shape::Page, "");
        let size = page_size(q.limit) as usize;
        let mut items = self
            .with_read(|c| {
                let mut st = c.prepare_cached(&b.sql)?;
                Ok(st
                    .query_map(params_from_iter(b.args.iter()), row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .map_err(|e| search_error(e, b.searched))?;
        let next = if items.len() > size {
            items.truncate(size);
            items.last().map(|i| i.seq)
        } else {
            None
        };
        Ok((items, next))
    }

    /// How many items match `q` (its `before` and `limit` ignored), counting
    /// at most `cap`.
    ///
    /// # Errors
    /// `invalid_query` when the search text cannot be used.
    pub fn inbox_count(&self, q: &InboxQuery, cap: u32) -> Result<u32> {
        let b = build(q, Shape::Count(cap), "");
        self.with_read(|c| Ok(c.query_row(&b.sql, params_from_iter(b.args.iter()), |r| r.get(0))?))
            .map_err(|e| search_error(e, b.searched))
    }

    /// The number of unread items.
    pub fn inbox_unread(&self) -> Result<u32> {
        self.with_read(|c| Ok(c.query_row(UNREAD_COUNT, [], |r| r.get(0))?))
    }

    /// Item `id`, if it exists.
    pub fn inbox_item(&self, id: &str) -> Result<Option<ItemRow>> {
        self.with_read(|c| Ok(c.query_row(BY_ID, params![id], row).optional()?))
    }

    /// The items with these `seq`s that exist, newest first.
    pub fn inbox_items_by_seq(&self, seqs: &[i64]) -> Result<Vec<ItemRow>> {
        if seqs.is_empty() {
            return Ok(Vec::new());
        }
        let json = serde_json::to_string(&seqs.iter().collect::<std::collections::BTreeSet<_>>())
            .expect("integers serialise");
        self.with_read(|c| {
            let mut st = c.prepare_cached(BY_SEQS)?;
            Ok(st
                .query_map(params![json], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Marks the items `ids` read (keeping an earlier read time) or unread;
    /// how many changed.
    pub fn inbox_mark(&self, ids: &[String], read: bool) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let json = super::feedback::id_array(ids);
        let now = Store::now();
        self.with_tx(|tx| {
            Ok(if read {
                tx.execute(MARK_READ, params![json, now])?
            } else {
                tx.execute(MARK_UNREAD, params![json])?
            })
        })
    }

    /// Marks every unread item matching `q`'s filters read (its `read`,
    /// `before` and `limit` ignored); how many changed.
    ///
    /// # Errors
    /// `invalid_query` when the search text cannot be used.
    pub fn inbox_mark_all(&self, q: &InboxQuery) -> Result<usize> {
        let b = build(q, Shape::MarkAll, &Store::now());
        self.with_tx(|tx| Ok(tx.execute(&b.sql, params_from_iter(b.args.iter()))?))
            .map_err(|e| search_error(e, b.searched))
    }

    /// A working record that ended as finished work (the agent said it was
    /// done, or its turn ended): a `finished` item keeping its message and
    /// threads, keyed by the record's key. `None` when it already has one.
    pub fn note_finished(&self, e: &Ended, harness: &str) -> Result<Option<i64>> {
        let detail = serde_json::json!({"message": e.message, "thread_ids": e.thread_ids});
        self.with_tx(|tx| {
            insert(
                tx,
                New {
                    kind: Kind::Finished,
                    key: format!("finished:{}", e.key),
                    artifact_id: Some(&e.artifact_id),
                    thread_id: None,
                    comment_id: None,
                    version_n: None,
                    question_id: None,
                    session_id: Some(&e.session_id),
                    harness: Some(harness),
                    detail: Some(detail),
                    text: e.message.clone().unwrap_or_default(),
                },
            )
        })
    }
}

/// Whether `s` is an item ID: a ULID, or `b` and 24 lowercase hex digits
/// (an item migration 21 filled in from the history).
pub fn is_item_id(s: &str) -> bool {
    crate::is_ulid(s)
        || (s.len() == 25
            && s.starts_with('b')
            && s[1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// `input` (an RFC 3339 time, or a `YYYY-MM-DD` date in UTC) as items store
/// their times: RFC 3339 UTC with milliseconds and `Z`, so it compares with
/// them as text. A date is its first instant, or with `end` the first
/// instant of the next day (an `until` date takes in the whole day).
/// `None` when it is neither.
pub fn stored_time(input: &str, end: bool) -> Option<String> {
    use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
    let s = input.trim();
    let t = if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let d = if end { d.succ_opt()? } else { d };
        d.and_hms_opt(0, 0, 0)?.and_utc()
    } else {
        DateTime::parse_from_rfc3339(s).ok()?.with_timezone(&Utc)
    };
    Some(t.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The agent an item came from (its session).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRef {
    pub handle: String,
    pub harness: String,
    /// The session's working directory.
    pub cwd: String,
}

/// A thread an item names, while it exists on a live artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRef {
    pub id: String,
    pub status: String,
    /// The anchor's one-line summary ([`crate::Anchor::summary`]); `None`
    /// when the anchor does not parse.
    pub summary: Option<String>,
}

/// A `reply` item's comment, while it exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyRef {
    pub body: String,
    pub created_at: String,
    /// The reply said a live page now shows the fix: an explicit address of
    /// its thread was recorded with it (at or after it, before the thread's
    /// next agent comment).
    pub addressed: bool,
}

/// A `version` item's version, while it exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRef {
    pub note: Option<String>,
    /// The threads it addressed that the owner is in.
    pub addressed: Vec<ThreadRef>,
}

/// What an item's view reads from its sources ([`Store::inbox_sources`]):
/// each is `None` (or empty) when the item names none or it is gone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    pub agent: Option<AgentRef>,
    /// The artifact, while it is live (not deleted).
    pub artifact: Option<crate::model::Artifact>,
    /// A live page's URL.
    pub page_url: Option<String>,
    pub thread: Option<ThreadRef>,
    pub reply: Option<ReplyRef>,
    pub version: Option<VersionRef>,
    /// `finished`: the threads of its record that still exist.
    pub threads: Vec<ThreadRef>,
}

const SESSION_REF: &str = "SELECT agent_handle, harness, cwd FROM sessions WHERE id = ?1";
const PAGE_URL: &str = "SELECT origin || path FROM live_pages WHERE artifact_id = ?1";
const THREAD_REF: &str = "SELECT t.id, t.status, t.anchor_json FROM threads t
    JOIN artifacts a ON a.id = t.artifact_id WHERE a.deleted_at IS NULL AND t.id = ?1";
const COMMENT_REF: &str = "SELECT body, created_at FROM comments WHERE id = ?1 AND thread_id = ?2";
/// Whether the agent reply `?3` (made at `?2`) on thread `?1` addressed
/// it: an explicit address of the thread was recorded at or after the
/// reply and before the thread's next agent comment (by insertion order).
const ADDRESSED_BY: &str = "WITH span(next) AS (SELECT coalesce(
        (SELECT n.created_at FROM comments n
          WHERE n.thread_id = ?1 AND n.author_kind = 'agent'
            AND n.rowid > (SELECT rowid FROM comments WHERE id = ?3)
          ORDER BY n.rowid LIMIT 1), '~'))
    SELECT EXISTS (SELECT 1 FROM live_pending, span
        WHERE thread_id = ?1 AND source = 'explicit' AND created_at >= ?2 AND created_at < next)
     OR EXISTS (SELECT 1 FROM version_threads, span
        WHERE thread_id = ?1 AND source = 'explicit' AND created_at >= ?2 AND created_at < next)";
const VERSION_NOTE: &str = "SELECT note FROM versions WHERE artifact_id = ?1 AND n = ?2";
const VERSION_THREADS: &str = "SELECT thread_id FROM version_threads
    WHERE artifact_id = ?1 AND version_n = ?2 ORDER BY rowid";

fn thread_ref(c: &Connection, tid: &str) -> Result<Option<ThreadRef>> {
    let row: Option<(String, String, String)> = c
        .prepare_cached(THREAD_REF)?
        .query_row(params![tid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    Ok(row.map(|(id, status, anchor)| ThreadRef {
        id,
        status,
        summary: serde_json::from_str::<crate::Anchor>(&anchor)
            .ok()
            .map(|a| a.summary()),
    }))
}

impl Store {
    /// What each of `items` reads from its sources, in the same order, read
    /// in one snapshot: point lookups by key, so the cost follows the items,
    /// not the history or the threads' lengths.
    pub fn inbox_sources(&self, items: &[ItemRow]) -> Result<Vec<Sources>> {
        use std::collections::HashMap;
        let owner = |c: &Connection| owner_pid(c);
        self.with_read(|c| {
            let pid = owner(c)?;
            let mut agents: HashMap<String, Option<AgentRef>> = HashMap::new();
            let mut artifacts: HashMap<String, (Option<crate::model::Artifact>, Option<String>)> =
                HashMap::new();
            let mut out = Vec::with_capacity(items.len());
            for i in items {
                let mut s = Sources::default();
                if let Some(sid) = &i.session_id {
                    if !agents.contains_key(sid) {
                        let a = c
                            .prepare_cached(SESSION_REF)?
                            .query_row(params![sid], |r| {
                                Ok(AgentRef {
                                    handle: r.get(0)?,
                                    harness: r.get(1)?,
                                    cwd: r.get(2)?,
                                })
                            })
                            .optional()?;
                        agents.insert(sid.clone(), a);
                    }
                    s.agent = agents[sid].clone();
                }
                if let Some(aid) = &i.artifact_id {
                    if !artifacts.contains_key(aid) {
                        let a = super::artifacts::live_artifact_in(c, aid)?;
                        let url = match &a {
                            Some(a) if a.kind == crate::live::KIND_LIVE => c
                                .prepare_cached(PAGE_URL)?
                                .query_row(params![aid], |r| r.get(0))
                                .optional()?,
                            _ => None,
                        };
                        artifacts.insert(aid.clone(), (a, url));
                    }
                    let (a, url) = &artifacts[aid];
                    s.artifact = a.clone();
                    s.page_url = url.clone();
                }
                let live = s.artifact.is_some();
                match i.kind {
                    Kind::Reply => {
                        if let (Some(tid), Some(cid), true) = (&i.thread_id, &i.comment_id, live) {
                            s.thread = thread_ref(c, tid)?;
                            if s.thread.is_some() {
                                let reply: Option<(String, String)> = c
                                    .prepare_cached(COMMENT_REF)?
                                    .query_row(params![cid, tid], |r| Ok((r.get(0)?, r.get(1)?)))
                                    .optional()?;
                                if let Some((body, created_at)) = reply {
                                    // Only a live page's thread is addressed by a reply.
                                    let on_page = s
                                        .artifact
                                        .as_ref()
                                        .is_some_and(|a| a.kind == crate::live::KIND_LIVE);
                                    let addressed = on_page
                                        && c.prepare_cached(ADDRESSED_BY)?
                                            .query_row(params![tid, created_at, cid], |r| {
                                                r.get(0)
                                            })?;
                                    s.reply = Some(ReplyRef {
                                        body,
                                        created_at,
                                        addressed,
                                    });
                                }
                            }
                        }
                    }
                    Kind::Version => {
                        if let (Some(aid), Some(n), true) = (&i.artifact_id, i.version_n, live) {
                            let note: Option<Option<String>> = c
                                .prepare_cached(VERSION_NOTE)?
                                .query_row(params![aid, n], |r| r.get(0))
                                .optional()?;
                            if let Some(note) = note {
                                let tids: Vec<String> = c
                                    .prepare_cached(VERSION_THREADS)?
                                    .query_map(params![aid, n], |r| r.get(0))?
                                    .collect::<rusqlite::Result<_>>()?;
                                let mut addressed = Vec::new();
                                for t in tids {
                                    let mine = match &pid {
                                        Some(p) => owner_in(c, p, &t)?,
                                        None => false,
                                    };
                                    if mine && let Some(r) = thread_ref(c, &t)? {
                                        addressed.push(r);
                                    }
                                }
                                s.version = Some(VersionRef { note, addressed });
                            }
                        }
                    }
                    Kind::Finished => {
                        let tids = i
                            .detail
                            .as_ref()
                            .and_then(|d| d["thread_ids"].as_array())
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        for t in tids.iter().filter_map(Value::as_str) {
                            if let Some(r) = thread_ref(c, t)? {
                                s.threads.push(r);
                            }
                        }
                    }
                    Kind::Published | Kind::Question => {}
                }
                out.push(s);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::{PublishRequest, validate};
    use crate::questions::{Answer, Question};
    use crate::store::questions::{Close, NewQuestion, Source};
    use crate::store::test_util::{anchor, artifact, session, store};
    use crate::store::threads::{AUTHOR_AGENT, NewComment};
    use crate::{ArtifactId, NewThread};

    const MIA: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    /// An owner (claimed browser viewer) with a thread on a fresh artifact;
    /// returns (owner public ID, artifact, thread).
    fn owner_thread(st: &Store) -> (String, ArtifactId, String) {
        let owner = st.owner_viewer(true).unwrap();
        let aid = artifact(st, None);
        let t = st
            .create_thread(
                &aid,
                NewThread {
                    author_public_id: Some(owner.public_id.clone()),
                    version_n: 1,
                    anchor: anchor(),
                    body: "make it blue".into(),
                    author_name: "Alex".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        (owner.public_id, aid, t.id)
    }

    /// An agent reply as the comments_reply route writes it.
    fn agent_reply(st: &Store, tid: &str, sid: &str, body: &str) {
        let harness = st.get_session(sid).unwrap().unwrap().harness;
        st.add_comment(
            tid,
            NewComment {
                author_kind: AUTHOR_AGENT,
                author_name: harness,
                author_public_id: None,
                via_session_id: Some(sid.into()),
                body: body.into(),
                via_page: false,
            },
        )
        .unwrap();
    }

    /// The artifact's next version, published by session `sid`.
    fn publish_next_as(st: &Store, aid: &ArtifactId, sid: &str, note: Option<&str>) {
        publish_addressing(st, aid, sid, note, &[]);
    }

    fn publish_addressing(
        st: &Store,
        aid: &ArtifactId,
        sid: &str,
        note: Option<&str>,
        tids: &[&str],
    ) {
        let current = st.get_artifact(aid).unwrap().unwrap().current_version;
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "if_version": current,
            "note": note,
            "addresses": tids,
            "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        st.publish_version(aid, validate(req).unwrap(), Some(sid))
            .unwrap();
    }

    fn ask(st: &Store, sid: &str, aid: Option<&str>) -> String {
        ask_as(st, sid, aid, None)
    }

    /// A question; mirrored from the hook when `tool_use_id` is given.
    fn ask_as(st: &Store, sid: &str, aid: Option<&str>, tool_use_id: Option<&str>) -> String {
        let questions: Vec<Question> = serde_json::from_value(serde_json::json!([
            {"question": "Which palette?", "header": "Colours",
             "options": [{"label": "Teal"}, {"label": "Amber"}]}
        ]))
        .unwrap();
        st.create_question(NewQuestion {
            session_id: sid.into(),
            artifact_id: aid.map(Into::into),
            source: if tool_use_id.is_some() {
                Source::Hook
            } else {
                Source::Ask
            },
            tool_use_id: tool_use_id.map(Into::into),
            questions,
            released: false,
        })
        .unwrap()
        .0
        .id
    }

    fn unread_question_ids(st: &Store) -> Vec<String> {
        st.inbox_list(&InboxQuery {
            read: ReadFilter::Unread,
            ..Default::default()
        })
        .unwrap()
        .0
        .into_iter()
        .filter_map(|i| i.question_id)
        .collect()
    }

    fn all(st: &Store) -> Vec<ItemRow> {
        st.inbox_list(&InboxQuery {
            read: ReadFilter::All,
            limit: 200,
            ..Default::default()
        })
        .unwrap()
        .0
    }

    fn find(st: &Store, text: &str) -> usize {
        st.inbox_list(&InboxQuery {
            text: Some(text.into()),
            limit: 50,
            ..Default::default()
        })
        .unwrap()
        .0
        .len()
    }

    #[test]
    fn item_ids_and_stored_times() {
        assert!(is_item_id(MIA));
        assert!(is_item_id("b0123456789abcdef01234567"));
        for bad in [
            "",
            "b0123",
            "B0123456789abcdef01234567",
            "b0123456789ABCDEF01234567",
            "x",
        ] {
            assert!(!is_item_id(bad), "{bad}");
        }
        assert_eq!(
            stored_time("2026-10-06", false).as_deref(),
            Some("2026-10-06T00:00:00.000Z")
        );
        assert_eq!(
            stored_time("2026-10-06", true).as_deref(),
            Some("2026-10-07T00:00:00.000Z"),
            "an until date takes in the day"
        );
        assert_eq!(
            stored_time("2026-10-06T12:30:00+02:00", false).as_deref(),
            Some("2026-10-06T10:30:00.000Z")
        );
        assert_eq!(
            stored_time(" 2026-10-06T10:30:00.123456Z ", true).as_deref(),
            Some("2026-10-06T10:30:00.123Z")
        );
        for bad in ["yesterday", "2026-13-01", "2026-10-06 10:30", ""] {
            assert_eq!(stored_time(bad, false), None, "{bad}");
        }
    }

    #[test]
    fn sources_read_each_kind_and_say_what_is_gone() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let (pid, aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "Done: blue.");
        publish_addressing(&st, &aid, &sid, Some("Blue now"), &[&tid]);
        let items = all(&st);
        let s = st.inbox_sources(&items).unwrap();
        let (reply, version) = (
            items.iter().position(|i| i.kind == Kind::Reply).unwrap(),
            items.iter().position(|i| i.kind == Kind::Version).unwrap(),
        );
        let r = s[reply].reply.as_ref().unwrap();
        assert_eq!((r.body.as_str(), r.addressed), ("Done: blue.", false));
        let t = s[reply].thread.as_ref().unwrap();
        assert_eq!((t.id.as_str(), t.status.as_str()), (tid.as_str(), "open"));
        assert!(t.summary.as_deref().unwrap().contains("Quarterly goals"));
        assert_eq!(s[reply].agent.as_ref().unwrap().harness, "claude");
        assert_eq!(s[reply].artifact.as_ref().unwrap().id, aid.as_str());
        assert_eq!(s[reply].page_url, None);
        let v = s[version].version.as_ref().unwrap();
        assert_eq!(v.note.as_deref(), Some("Blue now"));
        assert_eq!(v.addressed.len(), 1, "the owner's thread");

        // A live page: the owner's thread, and an agent's addressed reply.
        let key = crate::live::PageKey {
            origin: "http://localhost:5173".into(),
            path: "/p".into(),
        };
        let e = st.ensure_live_page(&key, "p", None).unwrap();
        let lid = ArtifactId::parse(&e.artifact.id).unwrap();
        let lt = st
            .create_thread(
                &lid,
                NewThread {
                    author_public_id: Some(pid),
                    version_n: 1,
                    anchor: anchor(),
                    body: "fix".into(),
                    author_name: "Alex".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        agent_reply(&st, &lt.id, &sid, "Looking");
        st.add_addressed_reply(
            &lid,
            &lt.id,
            NewComment {
                author_kind: AUTHOR_AGENT,
                author_name: "claude".into(),
                author_public_id: None,
                via_session_id: Some(sid.clone()),
                body: "Fixed".into(),
                via_page: false,
            },
            "claude",
        )
        .unwrap();
        let live: Vec<ItemRow> = all(&st)
            .into_iter()
            .filter(|i| i.artifact_id.as_deref() == Some(lid.as_str()))
            .collect();
        let s = st.inbox_sources(&live).unwrap();
        let by_body = |b: &str| {
            s.iter()
                .find(|x| x.reply.as_ref().is_some_and(|r| r.body == b))
                .unwrap()
        };
        assert!(by_body("Fixed").reply.as_ref().unwrap().addressed);
        assert!(!by_body("Looking").reply.as_ref().unwrap().addressed);
        assert_eq!(
            by_body("Fixed").page_url.as_deref(),
            Some("http://localhost:5173/p")
        );

        // Gone: a deleted thread takes its reply; a deleted artifact all.
        st.delete_thread(&tid).unwrap();
        let s = st.inbox_sources(&items).unwrap();
        assert!(s[reply].thread.is_none() && s[reply].reply.is_none());
        assert!(s[version].version.is_some());
        st.delete_artifact(&aid).unwrap();
        let s = st.inbox_sources(&items).unwrap();
        assert!(
            s.iter()
                .all(|x| x.artifact.is_none() && x.version.is_none())
        );
        assert!(s[reply].agent.is_some(), "sessions are never deleted");
    }

    #[test]
    fn a_reply_in_the_owners_thread_makes_one_unread_item() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let (_p, aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "Done: it is blue now.");
        let items = all(&st);
        assert_eq!(items.len(), 1);
        let i = &items[0];
        assert_eq!(
            (i.kind, i.read_at.is_none(), i.harness.as_deref()),
            (Kind::Reply, true, Some("claude"))
        );
        assert_eq!(i.artifact_id.as_deref(), Some(aid.as_str()));
        assert_eq!(i.thread_id.as_deref(), Some(tid.as_str()));
        assert_eq!(i.session_id.as_deref(), Some(sid.as_str()));
        assert!(crate::is_ulid(&i.id));
        assert_eq!(st.inbox_unread().unwrap(), 1);
        assert_eq!(st.inbox_item(&i.id).unwrap().as_ref(), Some(i));
        assert_eq!(
            st.inbox_items_by_seq(&[i.seq, 999]).unwrap(),
            vec![i.clone()]
        );
    }

    #[test]
    fn a_mention_or_a_resolve_puts_the_owner_in_the_thread() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let owner = st.set_owner_name("Alex", true).unwrap();
        let aid = artifact(&st, None);
        let thread = |body: String| {
            st.create_thread(
                &aid,
                NewThread {
                    author_public_id: Some("u_stranger".into()),
                    version_n: 1,
                    anchor: anchor(),
                    body,
                    author_name: "Mia".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap()
            .id
        };
        let mentioned = thread("@alex look".into());
        let resolved = thread("x".into());
        st.resolve_thread(&resolved, &format!("viewer:{}", owner.public_id))
            .unwrap();
        agent_reply(&st, &mentioned, &sid, "ok");
        agent_reply(&st, &resolved, &sid, "ok");
        let threads: Vec<_> = all(&st).into_iter().filter_map(|i| i.thread_id).collect();
        assert_eq!(threads.len(), 2, "{threads:?}");
    }

    #[test]
    fn no_item_for_a_stranger_thread_or_a_viewer_comment() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        st.owner_viewer(true).unwrap();
        let aid = artifact(&st, None);
        let t = st
            .create_thread(
                &aid,
                NewThread {
                    author_public_id: Some("u_stranger".into()),
                    version_n: 1,
                    anchor: anchor(),
                    body: "x".into(),
                    author_name: "Mia".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        agent_reply(&st, &t.id, &sid, "ok");
        assert!(all(&st).is_empty());
    }

    #[test]
    fn without_an_owner_only_questions_published_and_finished_are_made() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let aid = artifact(&st, Some(&sid));
        let t = st
            .create_thread(
                &aid,
                NewThread {
                    author_public_id: None,
                    version_n: 1,
                    anchor: anchor(),
                    body: "x".into(),
                    author_name: "Mia".into(),
                    clip: None,
                    via_page: false,
                },
            )
            .unwrap();
        agent_reply(&st, &t.id, &sid, "ok");
        publish_next_as(&st, &aid, &sid, None);
        ask(&st, &sid, None);
        let kinds: Vec<Kind> = all(&st).iter().map(|i| i.kind).collect();
        assert_eq!(kinds, vec![Kind::Question, Kind::Published]);
    }

    #[test]
    fn versions_published_questions_and_finished_work() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let mine = artifact(&st, Some(&sid));
        let (_p, aid, tid) = owner_thread(&st);
        publish_addressing(&st, &aid, &sid, Some("Two columns"), &[&tid]);
        publish_next_as(&st, &mine, &sid, None);
        ask(&st, &sid, Some(aid.as_str()));
        let e = Ended {
            session_id: sid.clone(),
            artifact_id: aid.as_str().into(),
            key: "01J0WORK".into(),
            message: Some("Recoloured the header".into()),
            thread_ids: vec!["t1".into()],
        };
        assert!(st.note_finished(&e, "claude").unwrap().is_some());
        assert_eq!(
            st.note_finished(&e, "claude").unwrap(),
            None,
            "one per record"
        );
        let items = all(&st);
        let kinds: Vec<Kind> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                Kind::Finished,
                Kind::Question,
                Kind::Version,
                Kind::Published
            ]
        );
        assert_eq!(
            items[0].detail,
            Some(serde_json::json!({"message": "Recoloured the header", "thread_ids": ["t1"]}))
        );
        assert_eq!(items[2].version_n, Some(2));
        assert_eq!(items[3].artifact_id.as_deref(), Some(mine.as_str()));
        assert_eq!(find(&st, "columns"), 1, "a version by its note");
        assert_eq!(
            find(&st, "quarterly goals columns"),
            1,
            "and its addressed quotes"
        );
        assert_eq!(find(&st, "recoloured"), 1);
        assert_eq!(
            find(&st, "palette amber"),
            1,
            "a question by its text and labels"
        );
    }

    #[test]
    fn a_question_closed_is_reindexed_and_read_unless_withdrawn() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let answered = ask(&st, &sid, None);
        let withdrawn = ask(&st, &sid, None);
        assert_eq!(st.inbox_unread().unwrap(), 2);
        st.close_question(
            &answered,
            Close::Answer {
                answers: vec![Answer {
                    selected: vec![],
                    text: Some("teal with a darker header".into()),
                }],
                via: "shell",
            },
        )
        .unwrap();
        st.close_question(&withdrawn, Close::Withdraw).unwrap();
        assert_eq!(st.inbox_unread().unwrap(), 1);
        assert_eq!(find(&st, "darker"), 1, "the answer's text");
        assert_eq!(find(&st, "palette"), 2, "the question's text is kept");
        let unread = st
            .inbox_list(&InboxQuery {
                read: ReadFilter::Unread,
                ..Default::default()
            })
            .unwrap()
            .0;
        assert_eq!(unread[0].question_id.as_deref(), Some(withdrawn.as_str()));
    }

    #[test]
    fn looking_and_seeing_mark_read_only_for_the_owner() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let (_p, aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "done");
        let mia = st.mint_viewer(MIA, false).unwrap();
        st.mark_looked(&mia.id, &aid, std::slice::from_ref(&tid))
            .unwrap();
        assert_eq!(
            st.inbox_unread().unwrap(),
            1,
            "a LAN viewer's look marks nothing"
        );
        let other = artifact(&st, None);
        let owner = st.owner_viewer(true).unwrap();
        st.mark_looked(&owner.id, &other, std::slice::from_ref(&tid))
            .unwrap();
        assert_eq!(
            st.inbox_unread().unwrap(),
            1,
            "a look at another page marks nothing"
        );
        st.mark_looked(&owner.id, &aid, std::slice::from_ref(&tid))
            .unwrap();
        assert_eq!(st.inbox_unread().unwrap(), 0);
        publish_next_as(&st, &aid, &sid, None);
        publish_next_as(&st, &aid, &sid, None);
        assert_eq!(st.inbox_unread().unwrap(), 2);
        st.mark_seen(&mia.id, &aid, 3).unwrap();
        assert_eq!(st.inbox_unread().unwrap(), 2);
        st.mark_seen(&owner.id, &aid, 2).unwrap();
        assert_eq!(st.inbox_unread().unwrap(), 1, "version 3 is not seen yet");
        st.mark_seen(&owner.id, &aid, 3).unwrap();
        assert_eq!(st.inbox_unread().unwrap(), 0);
    }

    #[test]
    fn marks_one_all_and_filtered() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let (_p, _aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "blue header");
        agent_reply(&st, &tid, &sid, "green footer");
        let items = all(&st);
        assert_eq!(st.inbox_mark(&[items[0].id.clone()], true).unwrap(), 1);
        assert_eq!(
            st.inbox_mark(&[items[0].id.clone()], true).unwrap(),
            0,
            "idempotent"
        );
        assert_eq!(st.inbox_mark(&[items[0].id.clone()], false).unwrap(), 1);
        let q = InboxQuery {
            text: Some("green".into()),
            ..Default::default()
        };
        assert_eq!(st.inbox_count(&q, 100).unwrap(), 1);
        assert_eq!(st.inbox_mark_all(&q).unwrap(), 1);
        assert_eq!(st.inbox_unread().unwrap(), 1);
        assert_eq!(st.inbox_mark_all(&InboxQuery::default()).unwrap(), 1);
        assert_eq!(st.inbox_unread().unwrap(), 0);
        assert_eq!(all(&st).len(), 2, "marking never removes");
        assert_eq!(
            st.inbox_count(&InboxQuery::default(), 1).unwrap(),
            1,
            "capped"
        );
    }

    #[test]
    fn search_takes_any_text() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let (_p, _aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "Résumé dashboard uses c++ \"quoted\" text");
        assert_eq!(find(&st, "resume"), 1, "diacritics folded");
        assert_eq!(find(&st, "dash"), 1, "prefix");
        assert_eq!(find(&st, "DASHBOARD quoted"), 1, "all terms, any case");
        assert_eq!(find(&st, "quarterly"), 1, "the artifact's title");
        assert_eq!(find(&st, "claude"), 1, "the harness");
        assert_eq!(find(&st, "dashboard missing"), 0);
        let long = "z".repeat(2000);
        for junk in [
            "c++",
            "\"unclosed",
            "NEAR(a b)",
            "-x",
            "a AND",
            "*",
            "   ",
            "\"",
            "a OR b",
            "x:y",
            "^start",
            "(",
            "{col}",
            &long,
        ] {
            st.inbox_list(&InboxQuery {
                text: Some(junk.into()),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("{junk:?}: {e}"));
            st.inbox_count(
                &InboxQuery {
                    text: Some(junk.into()),
                    ..Default::default()
                },
                10,
            )
            .unwrap_or_else(|e| panic!("{junk:?}: {e}"));
        }
        assert_eq!(find(&st, "*"), 1, "no terms: no text filter");
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("* - ()"), None);
        assert_eq!(fts_query("a \"b"), Some("\"a\"* \"\"\"b\"*".into()));
        let many = (0..40)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(fts_query(&many).unwrap().matches('*').count(), MAX_TERMS);
    }

    #[test]
    fn filters_and_cursor() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let pi = session(&st, "pi", "h2");
        let (_p, aid, tid) = owner_thread(&st);
        for i in 0..5 {
            agent_reply(&st, &tid, &sid, &format!("c{i}"));
        }
        agent_reply(&st, &tid, &pi, "from pi");
        let page = |q: InboxQuery| st.inbox_list(&q).unwrap();
        let q = InboxQuery {
            agent: Some(Agent::Harness("pi".into())),
            ..Default::default()
        };
        assert_eq!(page(q).0.len(), 1);
        let handle = st.get_session(&sid).unwrap().unwrap().agent_handle;
        let q = InboxQuery {
            agent: Some(Agent::Handle(handle)),
            ..Default::default()
        };
        assert_eq!(page(q).0.len(), 5);
        let q = InboxQuery {
            artifact: Some(aid.as_str().into()),
            kinds: vec![Kind::Reply],
            limit: 4,
            ..Default::default()
        };
        let (first, next) = page(q.clone());
        assert_eq!(first.len(), 4);
        assert!(
            first.windows(2).all(|w| w[0].seq > w[1].seq),
            "newest first"
        );
        let (rest, none) = page(InboxQuery {
            before: next,
            ..q.clone()
        });
        assert_eq!((rest.len(), none), (2, None));
        assert!(rest[0].seq < first[3].seq);
        let q = InboxQuery {
            kinds: vec![Kind::Version, Kind::Question],
            ..Default::default()
        };
        assert!(page(q).0.is_empty());
        let q = InboxQuery {
            kinds: vec![Kind::Version, Kind::Reply],
            ..Default::default()
        };
        assert_eq!(page(q).0.len(), 6);
        let at = first[0].created_at.clone();
        let q = InboxQuery {
            since: Some(at.clone()),
            ..Default::default()
        };
        assert!(!page(q).0.is_empty());
        let q = InboxQuery {
            until: Some(first[3].created_at.clone()),
            ..Default::default()
        };
        assert!(page(q).0.iter().all(|i| i.created_at < first[3].created_at));
        let q = InboxQuery {
            artifact: Some("nope".into()),
            ..Default::default()
        };
        assert!(page(q).0.is_empty());
        st.inbox_mark(&[first[0].id.clone()], true).unwrap();
        let q = InboxQuery {
            read: ReadFilter::Read,
            ..Default::default()
        };
        assert_eq!(page(q).0.len(), 1);
        let q = InboxQuery {
            read: ReadFilter::Unread,
            ..Default::default()
        };
        assert_eq!(page(q).0.len(), 5);
        let q = InboxQuery {
            text: Some("c".into()),
            limit: 2,
            ..Default::default()
        };
        let (one, next) = page(q.clone());
        assert_eq!(one.len(), 2);
        let (two, _) = page(InboxQuery { before: next, ..q });
        assert!(
            two.iter().all(|i| i.seq < one[1].seq),
            "a search pages by seq too"
        );
    }

    #[test]
    fn listener_hears_committed_changes_only() {
        let (_d, st) = store();
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = heard.clone();
        st.set_inbox_listener(Box::new(move |c| h.lock().unwrap().extend(c)));
        let sid = session(&st, "claude", "h1");
        let (_p, _aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "x");
        assert_eq!(heard.lock().unwrap().len(), 1);
        assert!(heard.lock().unwrap()[0].made);
        let _ = st.with_tx(|tx| -> crate::Result<()> {
            tx.execute("UPDATE inbox_items SET read_at = 'x'", [])?;
            Err(CoreError::NotFound)
        });
        assert_eq!(
            heard.lock().unwrap().len(),
            1,
            "a rolled-back change is not heard"
        );
        let id = all(&st)[0].id.clone();
        st.inbox_mark(&[id], true).unwrap();
        let h = heard.lock().unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(
            h[1],
            InboxChange {
                seq: h[0].seq,
                made: false
            }
        );
    }

    #[test]
    fn only_the_owner_moving_a_question_reads_it_not_the_hooks_timer() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let timed = ask_as(&st, &sid, None, Some("toolu_1"));
        let moved = ask_as(&st, &sid, None, Some("toolu_2"));
        st.close_question(&timed, Close::Expire).unwrap();
        st.close_question(&moved, Close::Release).unwrap();
        assert_eq!(unread_question_ids(&st), vec![timed.clone()]);
        assert_eq!(
            st.question(&timed).unwrap().unwrap().status,
            Status::Released
        );
        // Answered in the terminal afterwards: read.
        st.close_question(
            &timed,
            Close::Terminal {
                answers: vec![Answer {
                    selected: vec!["Teal".into()],
                    text: None,
                }],
            },
        )
        .unwrap();
        assert!(unread_question_ids(&st).is_empty());
        let a = ask(&st, &sid, None);
        assert!(
            st.close_question(&a, Close::Expire).is_err(),
            "only a mirrored question moves to the terminal"
        );
    }

    #[test]
    fn the_listener_hears_every_source_change() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let withdrawn = ask(&st, &sid, None);
        let answered = ask(&st, &sid, None);
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = heard.clone();
        st.set_inbox_listener(Box::new(move |c| h.lock().unwrap().extend(c)));
        st.close_question(&withdrawn, Close::Withdraw).unwrap();
        assert_eq!(heard.lock().unwrap().len(), 1, "a withdrawn question");
        let item = all(&st)
            .into_iter()
            .find(|i| i.question_id.as_deref() == Some(answered.as_str()))
            .unwrap();
        st.inbox_mark(std::slice::from_ref(&item.id), true).unwrap();
        let read_at = st.inbox_item(&item.id).unwrap().unwrap().read_at;
        st.close_question(&answered, Close::Decline).unwrap();
        let h = heard.lock().unwrap().clone();
        assert_eq!(h.len(), 3, "an already read question that closed: {h:?}");
        assert_eq!(
            h[2],
            InboxChange {
                seq: item.seq,
                made: false
            }
        );
        assert_eq!(
            st.inbox_item(&item.id).unwrap().unwrap().read_at,
            read_at,
            "an earlier read time is kept"
        );
    }

    #[test]
    fn a_listener_may_replace_itself() {
        let (_d, st) = store();
        let st = std::sync::Arc::new(st);
        let weak = std::sync::Arc::downgrade(&st);
        st.set_inbox_listener(Box::new(move |_| {
            if let Some(st) = weak.upgrade() {
                st.set_inbox_listener(Box::new(|_| {}));
            }
        }));
        let sid = session(&st, "claude", "h1");
        ask(&st, &sid, None);
        ask(&st, &sid, None);
    }

    #[test]
    fn mark_all_stops_at_the_newest_item_shown() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        ask(&st, &sid, None);
        let shown = all(&st)[0].seq;
        ask(&st, &sid, None);
        let q = InboxQuery {
            upto: Some(shown),
            ..Default::default()
        };
        assert_eq!(st.inbox_mark_all(&q).unwrap(), 1);
        assert_eq!(st.inbox_unread().unwrap(), 1);
    }

    #[test]
    fn dates_bound_the_page_exactly() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let stamps = [
            "2026-01-01T00:00:01.000Z",
            "2026-01-01T00:00:02.000Z",
            "2026-01-01T00:00:02.000Z",
            "2026-01-01T00:00:03.000Z",
            "2026-01-01T00:00:04.000Z",
        ];
        for _ in stamps {
            ask(&st, &sid, None);
        }
        st.with_tx(|tx| {
            for (i, t) in stamps.iter().enumerate() {
                tx.execute(
                    "UPDATE inbox_items SET created_at = ?2 WHERE seq = ?1",
                    params![i as i64 + 1, t],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let count = |since: Option<&str>, until: Option<&str>| {
            let q = InboxQuery {
                since: since.map(Into::into),
                until: until.map(Into::into),
                ..Default::default()
            };
            let (page, _) = st.inbox_list(&q).unwrap();
            assert_eq!(st.inbox_count(&q, 100).unwrap() as usize, page.len());
            page.len()
        };
        assert_eq!(count(Some("2026-01-01T00:00:02.000Z"), None), 4);
        assert_eq!(count(None, Some("2026-01-01T00:00:02.000Z")), 1);
        assert_eq!(
            count(
                Some("2026-01-01T00:00:02.000Z"),
                Some("2026-01-01T00:00:04.000Z")
            ),
            3
        );
        assert_eq!(count(Some("2026-01-01T00:00:05.000Z"), None), 0);
        assert_eq!(count(None, Some("2026-01-01T00:00:09.000Z")), 5);
        assert_eq!(count(None, Some("2026-01-01T00:00:00.000Z")), 0);
    }

    /// Search text the person types or a URL carries: quotes, operators,
    /// NUL and other controls, odd Unicode, empty and blank. Every one is
    /// answered (never an error) by list, count and mark-all.
    #[test]
    fn search_text_never_fails() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let (_p, _aid, tid) = owner_thread(&st);
        agent_reply(&st, &tid, &sid, "Résumé dashboard «quoted» 東京タワー text");
        let mut inputs: Vec<String> = [
            "",
            " ",
            "\t\n",
            "\"",
            "\"\"",
            "\"a",
            "a\"",
            "'",
            "*",
            "**",
            "a*",
            "*a",
            "-",
            "-a",
            "+a",
            "^a",
            "a:b",
            ":",
            "{a b}:c",
            "(",
            ")",
            "(a",
            "NEAR(a b)",
            "NEAR",
            "a AND",
            "AND",
            "OR",
            "NOT a",
            "a OR b",
            "a NOT",
            "\0",
            "a\0b",
            "x\u{0}",
            "\0\0\"",
            "a\u{1}b",
            "\u{7f}",
            "\u{feff}a",
            "\u{200b}",
            "\u{202e}abc",
            "\u{fffd}",
            "\u{ffff}",
            "\u{10ffff}",
            "e\u{301}",
            "👩‍💻",
            "東京",
            "タワー",
            "ß",
            "İ",
            "\u{2028}a",
            "a\u{85}b",
            "\\",
            "\\\"",
            "%",
            "_",
            "a%00b",
            "\u{1f600}\u{1f600}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        inputs.push("z".repeat(5000));
        inputs.push("a ".repeat(500));
        // Deterministic random strings over a hostile alphabet.
        let alphabet: Vec<char> =
            "a Zé\"'*-+^:(){}\0\u{1}\u{7f}\u{feff}\u{fffd}東👩\t\n.,;NEARORAND"
                .chars()
                .collect();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..300 {
            let mut t = String::new();
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for _ in 0..(x % 24) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                t.push(alphabet[(x % alphabet.len() as u64) as usize]);
            }
            inputs.push(t);
        }
        for t in &inputs {
            let q = InboxQuery {
                text: Some(t.clone()),
                ..Default::default()
            };
            st.inbox_list(&q)
                .unwrap_or_else(|e| panic!("list {t:?}: {e}"));
            st.inbox_count(&q, 10)
                .unwrap_or_else(|e| panic!("count {t:?}: {e}"));
            st.inbox_mark_all(&q)
                .unwrap_or_else(|e| panic!("mark all {t:?}: {e}"));
        }
        assert_eq!(
            find(&st, "a\0dash"),
            0,
            "NUL separates terms: `a` is not in it"
        );
        assert_eq!(find(&st, "resume\0dash"), 1);
        assert_eq!(find(&st, "東京"), 1);
        assert_eq!(fts_query("\0"), None);
        assert_eq!(fts_query("x\u{0}"), Some("\"x\"*".into()));
    }

    #[test]
    fn an_sql_error_of_a_search_is_invalid_query() {
        let e = search_error(
            CoreError::Db(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some("unterminated string".into()),
            )),
            true,
        );
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "invalid_query",
                    ..
                }
            ),
            "{e:?}"
        );
        let busy = search_error(
            CoreError::Db(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(5),
                None,
            )),
            true,
        );
        assert!(
            matches!(busy, CoreError::Db(_)),
            "busy stays a database error"
        );
    }

    #[test]
    fn stamps_never_go_backwards_when_the_clock_steps_back() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let made = |key: &str, now: &str| {
            st.with_tx(|tx| {
                insert_at(
                    tx,
                    New {
                        kind: Kind::Finished,
                        key: key.into(),
                        artifact_id: None,
                        thread_id: None,
                        comment_id: None,
                        version_n: None,
                        question_id: None,
                        session_id: Some(&sid),
                        harness: None,
                        detail: None,
                        text: String::new(),
                    },
                    now,
                )
            })
            .unwrap()
            .unwrap()
        };
        made("k1", "2026-01-01T00:00:05.000Z");
        // The clock steps back two seconds.
        let back = made("k2", "2026-01-01T00:00:03.000Z");
        made("k3", "2026-01-01T00:00:06.000Z");
        let stamps: Vec<String> = all(&st).into_iter().rev().map(|i| i.created_at).collect();
        assert_eq!(
            stamps,
            [
                "2026-01-01T00:00:05.000Z",
                "2026-01-01T00:00:05.000Z",
                "2026-01-01T00:00:06.000Z"
            ]
        );
        let q = InboxQuery {
            since: Some("2026-01-01T00:00:05.000Z".into()),
            until: Some("2026-01-01T00:00:06.000Z".into()),
            ..Default::default()
        };
        let page: Vec<i64> = st.inbox_list(&q).unwrap().0.iter().map(|i| i.seq).collect();
        assert_eq!(page.len(), 2, "the stepped-back item is in its range");
        assert!(page.contains(&back));
    }
}
