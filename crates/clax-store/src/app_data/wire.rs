//! An artifact store's operations as a host sends them ([`AppDataOp`]) and
//! their answers ([`AppDataReply`]): serialisable, so the Worker can carry
//! them to the artifact's Durable Object, and the same shape for a host in
//! the daemon's process. The instance's own types that are not serde travel
//! as plain mirrors: the rules as the artifact's declared capabilities, the
//! caller as [`WireCaller`], the audit envelope pre-rendered
//! ([`super::AuditEnv`]), and an error as [`WireError`], whose fixed codes
//! map back to [`CoreError`].

use super::{AppData, DocView, Exported, Fold, WriteCtx};
use crate::audit::AuditRow;
use crate::docs::{
    Acquire, Acquired, BatchWrite, Doc, DocChange, DocPage, DocQuery, Pin, StrReplace, Written,
    author_tokens,
};
use clax_core::audit::AuditIds;
use clax_core::db::{Caller, Level, Rules};
use clax_core::visibility::Author;
use clax_core::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A [`Caller`] on the wire: its writer as a stored token
/// ([`Author::token`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireCaller {
    pub level: Level,
    pub viewer: Option<String>,
    pub person: String,
    pub via_page: bool,
}

impl From<&Caller> for WireCaller {
    fn from(c: &Caller) -> WireCaller {
        WireCaller {
            level: c.level,
            viewer: c.viewer.clone(),
            person: c
                .person
                .token()
                .unwrap_or(clax_core::visibility::OWNER)
                .to_string(),
            via_page: c.via_page,
        }
    }
}

impl From<&WireCaller> for Caller {
    fn from(c: &WireCaller) -> Caller {
        Caller {
            level: c.level,
            viewer: c.viewer.clone(),
            person: Author::of_token(Some(&c.person)),
            via_page: c.via_page,
        }
    }
}

/// One operation on an artifact's store: each document operation of
/// [`AppData`] with what the instance decided for it (`caps`, the
/// artifact's declared capabilities, whose `db` rules apply), and the
/// move's and the folds' operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDataOp {
    /// [`AppData::doc_get_authored`].
    Get {
        caps: Value,
        path: String,
        caller: WireCaller,
    },
    /// [`AppData::doc_set`].
    Set {
        ctx: WriteCtx,
        caps: Value,
        path: String,
        data: Value,
        pin: Pin,
        caller: WireCaller,
    },
    /// [`AppData::doc_update_visible`].
    Update {
        ctx: WriteCtx,
        caps: Value,
        path: String,
        patch: Value,
        pin: Pin,
        caller: WireCaller,
        view: Option<DocView>,
    },
    /// [`AppData::doc_delete`].
    Delete {
        ctx: WriteCtx,
        caps: Value,
        path: String,
        pin: Pin,
        caller: WireCaller,
    },
    /// [`AppData::doc_str_replace_visible`].
    StrReplace {
        ctx: WriteCtx,
        caps: Value,
        path: String,
        replace: StrReplace,
        pin: Pin,
        caller: WireCaller,
        view: Option<DocView>,
    },
    /// [`AppData::doc_query_visible`].
    Query {
        caps: Value,
        query: DocQuery,
        caller: WireCaller,
        view: Option<DocView>,
    },
    /// [`AppData::doc_batch_visible`].
    Batch {
        ctx: WriteCtx,
        caps: Value,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: WireCaller,
        view: Option<DocView>,
    },
    /// [`AppData::doc_acquire_visible`].
    Acquire {
        ctx: WriteCtx,
        caps: Value,
        path: String,
        acquire: Acquire,
        caller: WireCaller,
        view: Option<DocView>,
    },
    /// [`AppData::import`].
    Import { rows: Exported },
    /// [`AppData::fold`].
    Fold { folds: Vec<Fold> },
    /// [`AppData::fold_mark`].
    FoldMark,
    /// [`AppData::authors`].
    Authors,
    /// [`AppData::authored_by`].
    AuthoredBy { author: String, is_owner: bool },
    /// [`AppData::audit_since`].
    AuditSince { seq: i64 },
}

/// A document with its authors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthoredDoc {
    pub doc: Doc,
    #[serde(with = "author_tokens")]
    pub authors: Vec<Author>,
}

/// An audit record of an artifact's store, as [`AuditRow`] holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireAuditRow {
    pub seq: i64,
    pub at: String,
    pub kind: String,
    pub actor: String,
    pub artifact_id: Option<String>,
    pub session_id: Option<String>,
    pub call_id: Option<String>,
    pub body: String,
    pub backfilled: bool,
}

impl From<AuditRow> for WireAuditRow {
    fn from(r: AuditRow) -> WireAuditRow {
        WireAuditRow {
            seq: r.seq,
            at: r.at,
            kind: r.kind,
            actor: r.actor,
            artifact_id: r.ids.artifact,
            session_id: r.ids.session,
            call_id: r.ids.call,
            body: r.body,
            backfilled: r.backfilled,
        }
    }
}

impl From<WireAuditRow> for AuditRow {
    fn from(r: WireAuditRow) -> AuditRow {
        AuditRow {
            seq: r.seq,
            at: r.at,
            kind: r.kind,
            actor: r.actor,
            ids: AuditIds {
                artifact: r.artifact_id,
                session: r.session_id,
                call: r.call_id,
                ..AuditIds::default()
            },
            body: r.body,
            backfilled: r.backfilled,
        }
    }
}

/// An [`AppDataOp`]'s answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDataReply {
    /// `Get`'s.
    Doc(Option<AuthoredDoc>),
    /// `Set`'s, `Update`'s, `Delete`'s and `StrReplace`'s.
    Written(Written),
    /// `Batch`'s.
    Batch(Vec<Written>),
    /// `Query`'s.
    Page(DocPage),
    /// `Acquire`'s.
    Acquired {
        acquired: Acquired,
        change: Option<DocChange>,
    },
    /// `Import`'s.
    Done,
    /// `Fold`'s and `FoldMark`'s: the store's fold mark.
    Folded(i64),
    /// `Authors`' (author tokens) and `AuthoredBy`'s (document paths).
    Strings(Vec<String>),
    /// `AuditSince`'s.
    Audit(Vec<WireAuditRow>),
    /// A change's reply, with the audit records it made here: the
    /// records the store wrote since just before the change, which the
    /// host copies into the instance's history.
    Audited {
        reply: Box<AppDataReply>,
        audit: Vec<WireAuditRow>,
    },
    /// The operation failed.
    Error(WireError),
}

/// A [`CoreError`] an artifact store answers, on the wire. `Invalid`
/// carries one of a fixed list of codes ([`WIRE_CODES`]); anything else a
/// store can fail with is `Internal`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "code")]
pub enum WireError {
    Invalid {
        invalid: String,
        message: String,
    },
    DocNotFound {
        path: String,
    },
    Conflict {
        path: String,
        current: Option<u64>,
    },
    PinRequired {
        path: String,
        current: u64,
    },
    Withheld {
        path: String,
        authors: Vec<String>,
    },
    InBatch {
        op: usize,
        path: String,
        error: Box<WireError>,
    },
    NotFound,
    Corrupt {
        artifact_id: String,
    },
    /// The store's schema is newer than this binary knows.
    SchemaNewer {
        found: u32,
        known: u32,
    },
    /// SQLite was busy past its timeout.
    Busy {
        message: String,
    },
    Internal {
        message: String,
    },
}

/// The `Invalid` codes an artifact store answers.
pub const WIRE_CODES: &[&str] = &[
    "invalid_argument",
    "quota_exceeded",
    "resource_exhausted",
    "old_str_not_found",
    "old_str_not_unique",
    "invalid_capabilities",
];

impl From<&CoreError> for WireError {
    fn from(e: &CoreError) -> WireError {
        let tokens = |a: &[Author]| {
            a.iter()
                .filter_map(|x| x.token().map(str::to_string))
                .collect()
        };
        match e {
            CoreError::Invalid { code, message } => match WIRE_CODES.iter().find(|c| *c == code) {
                Some(_) => WireError::Invalid {
                    invalid: code.to_string(),
                    message: message.clone(),
                },
                None => WireError::Internal {
                    message: format!("{code}: {message}"),
                },
            },
            CoreError::DocNotFound { path } => WireError::DocNotFound { path: path.clone() },
            CoreError::DocConflict { path, current } => WireError::Conflict {
                path: path.clone(),
                current: *current,
            },
            CoreError::DocPinRequired { path, current } => WireError::PinRequired {
                path: path.clone(),
                current: *current,
            },
            CoreError::DocWithheld { path, authors } => WireError::Withheld {
                path: path.clone(),
                authors: tokens(authors),
            },
            CoreError::InBatch { op, path, error } => WireError::InBatch {
                op: *op,
                path: path.clone(),
                error: Box::new(WireError::from(&**error)),
            },
            CoreError::NotFound => WireError::NotFound,
            CoreError::Corrupt { artifact_id, .. } => WireError::Corrupt {
                artifact_id: artifact_id.clone(),
            },
            CoreError::SchemaNewer { found, known } => WireError::SchemaNewer {
                found: *found,
                known: *known,
            },
            CoreError::Db(rusqlite::Error::SqliteFailure(f, m))
                if f.code == rusqlite::ErrorCode::DatabaseBusy =>
            {
                WireError::Busy {
                    message: m.clone().unwrap_or_default(),
                }
            }
            e => WireError::Internal {
                message: e.to_string(),
            },
        }
    }
}

impl From<WireError> for CoreError {
    fn from(e: WireError) -> CoreError {
        match e {
            WireError::Invalid { invalid, message } => {
                match WIRE_CODES.iter().find(|c| **c == invalid) {
                    Some(code) => CoreError::Invalid { code, message },
                    None => CoreError::Io(std::io::Error::other(format!("{invalid}: {message}"))),
                }
            }
            WireError::DocNotFound { path } => CoreError::DocNotFound { path },
            WireError::Conflict { path, current } => CoreError::DocConflict { path, current },
            WireError::PinRequired { path, current } => CoreError::DocPinRequired { path, current },
            WireError::Withheld { path, authors } => CoreError::DocWithheld {
                path,
                authors: authors.iter().map(|t| Author::of_token(Some(t))).collect(),
            },
            WireError::InBatch { op, path, error } => CoreError::InBatch {
                op,
                path,
                error: Box::new(CoreError::from(*error)),
            },
            WireError::NotFound => CoreError::NotFound,
            WireError::Corrupt { artifact_id } => CoreError::Corrupt {
                artifact_id,
                column: "docs.json",
                version: None,
            },
            WireError::SchemaNewer { found, known } => CoreError::SchemaNewer { found, known },
            WireError::Busy { message } => CoreError::Db(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(message),
            )),
            WireError::Internal { message } => CoreError::Io(std::io::Error::other(message)),
        }
    }
}

/// The reply that is not `want`'s: an artifact store answered another
/// operation's shape.
fn unexpected(want: &str, got: &AppDataReply) -> CoreError {
    CoreError::Io(std::io::Error::other(format!(
        "an artifact store answered {want} with {got:?}"
    )))
}

impl AppDataReply {
    /// The reply, or its error; a change's reply without its audit
    /// records ([`AppDataReply::audited`] takes them).
    pub fn ok(self) -> Result<AppDataReply> {
        match self {
            AppDataReply::Error(e) => Err(e.into()),
            AppDataReply::Audited { reply, .. } => reply.ok(),
            r => Ok(r),
        }
    }

    /// The reply, or its error, keeping a change's audit records.
    pub fn checked(self) -> Result<AppDataReply> {
        match self {
            AppDataReply::Error(e) => Err(e.into()),
            r => Ok(r),
        }
    }

    /// The reply and the audit records it carries (none for a read).
    pub fn audited(self) -> (AppDataReply, Vec<AuditRow>) {
        match self {
            AppDataReply::Audited { reply, audit } => {
                (*reply, audit.into_iter().map(AuditRow::from).collect())
            }
            r => (r, Vec::new()),
        }
    }

    pub fn into_doc(self) -> Result<Option<(Doc, Vec<Author>)>> {
        match self.ok()? {
            AppDataReply::Doc(d) => Ok(d.map(|d| (d.doc, d.authors))),
            r => Err(unexpected("get", &r)),
        }
    }

    pub fn into_written(self) -> Result<Written> {
        match self.ok()? {
            AppDataReply::Written(w) => Ok(w),
            r => Err(unexpected("a write", &r)),
        }
    }

    pub fn into_batch(self) -> Result<Vec<Written>> {
        match self.ok()? {
            AppDataReply::Batch(w) => Ok(w),
            r => Err(unexpected("batch", &r)),
        }
    }

    pub fn into_page(self) -> Result<DocPage> {
        match self.ok()? {
            AppDataReply::Page(p) => Ok(p),
            r => Err(unexpected("query", &r)),
        }
    }

    pub fn into_acquired(self) -> Result<(Acquired, Option<DocChange>)> {
        match self.ok()? {
            AppDataReply::Acquired { acquired, change } => Ok((acquired, change)),
            r => Err(unexpected("acquire", &r)),
        }
    }

    pub fn into_done(self) -> Result<()> {
        match self.ok()? {
            AppDataReply::Done => Ok(()),
            r => Err(unexpected("import", &r)),
        }
    }

    pub fn into_folded(self) -> Result<i64> {
        match self.ok()? {
            AppDataReply::Folded(n) => Ok(n),
            r => Err(unexpected("fold", &r)),
        }
    }

    pub fn into_strings(self) -> Result<Vec<String>> {
        match self.ok()? {
            AppDataReply::Strings(s) => Ok(s),
            r => Err(unexpected("authors", &r)),
        }
    }

    pub fn into_audit(self) -> Result<Vec<AuditRow>> {
        match self.ok()? {
            AppDataReply::Audit(rows) => Ok(rows.into_iter().map(AuditRow::from).collect()),
            r => Err(unexpected("audit_since", &r)),
        }
    }
}

impl AppData {
    /// Runs `op`; a failure is the reply's [`AppDataReply::Error`]. A
    /// change (a write, a batch, an acquire, a fold) answers
    /// [`AppDataReply::Audited`] when it recorded anything, with every
    /// audit record written since just before it (another caller's
    /// among them, when two change the store at once).
    pub fn apply(&self, op: AppDataOp) -> AppDataReply {
        let changes = matches!(
            op,
            AppDataOp::Set { .. }
                | AppDataOp::Update { .. }
                | AppDataOp::Delete { .. }
                | AppDataOp::StrReplace { .. }
                | AppDataOp::Batch { .. }
                | AppDataOp::Acquire { .. }
                | AppDataOp::Fold { .. }
        );
        let out = (|| {
            let before = if changes { self.audit_seq()? } else { 0 };
            let reply = self.run(op)?;
            if !changes {
                return Ok(reply);
            }
            let audit: Vec<WireAuditRow> = self
                .audit_since(before)?
                .into_iter()
                .map(WireAuditRow::from)
                .collect();
            Ok(if audit.is_empty() {
                reply
            } else {
                AppDataReply::Audited {
                    reply: Box::new(reply),
                    audit,
                }
            })
        })();
        out.unwrap_or_else(|e| AppDataReply::Error(WireError::from(&e)))
    }

    fn run(&self, op: AppDataOp) -> Result<AppDataReply> {
        let rules = |caps: &Value| Rules::from_capabilities(caps);
        Ok(match op {
            AppDataOp::Get { caps, path, caller } => AppDataReply::Doc(
                self.doc_get_authored(&rules(&caps)?, &path, &(&caller).into())?
                    .map(|(doc, authors)| AuthoredDoc { doc, authors }),
            ),
            AppDataOp::Set {
                ctx,
                caps,
                path,
                data,
                pin,
                caller,
            } => AppDataReply::Written(self.doc_set(
                &ctx,
                &rules(&caps)?,
                &path,
                data,
                pin,
                &(&caller).into(),
            )?),
            AppDataOp::Update {
                ctx,
                caps,
                path,
                patch,
                pin,
                caller,
                view,
            } => AppDataReply::Written(self.doc_update_visible(
                &ctx,
                &rules(&caps)?,
                &path,
                patch,
                pin,
                &(&caller).into(),
                view.as_ref(),
            )?),
            AppDataOp::Delete {
                ctx,
                caps,
                path,
                pin,
                caller,
            } => AppDataReply::Written(self.doc_delete(
                &ctx,
                &rules(&caps)?,
                &path,
                pin,
                &(&caller).into(),
            )?),
            AppDataOp::StrReplace {
                ctx,
                caps,
                path,
                replace,
                pin,
                caller,
                view,
            } => AppDataReply::Written(self.doc_str_replace_visible(
                &ctx,
                &rules(&caps)?,
                &path,
                replace,
                pin,
                &(&caller).into(),
                view.as_ref(),
            )?),
            AppDataOp::Query {
                caps,
                query,
                caller,
                view,
            } => AppDataReply::Page(self.doc_query_visible(
                &rules(&caps)?,
                &query,
                &(&caller).into(),
                view.as_ref(),
            )?),
            AppDataOp::Batch {
                ctx,
                caps,
                writes,
                lww,
                caller,
                view,
            } => AppDataReply::Batch(self.doc_batch_visible(
                &ctx,
                &rules(&caps)?,
                writes,
                lww,
                &(&caller).into(),
                view.as_ref(),
            )?),
            AppDataOp::Acquire {
                ctx,
                caps,
                path,
                acquire,
                caller,
                view,
            } => {
                let (acquired, change) = self.doc_acquire_visible(
                    &ctx,
                    &rules(&caps)?,
                    &path,
                    acquire,
                    &(&caller).into(),
                    view.as_ref(),
                )?;
                AppDataReply::Acquired { acquired, change }
            }
            AppDataOp::Import { rows } => {
                self.import(rows)?;
                AppDataReply::Done
            }
            AppDataOp::Fold { folds } => AppDataReply::Folded(self.fold(&folds)?),
            AppDataOp::FoldMark => AppDataReply::Folded(self.fold_mark()?),
            AppDataOp::Authors => AppDataReply::Strings(self.authors()?),
            AppDataOp::AuthoredBy { author, is_owner } => {
                AppDataReply::Strings(self.authored_by(&author, is_owner)?)
            }
            AppDataOp::AuditSince { seq } => AppDataReply::Audit(
                self.audit_since(seq)?
                    .into_iter()
                    .map(WireAuditRow::from)
                    .collect(),
            ),
        })
    }
}
