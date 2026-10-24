//! An artifact's app data in a store of its own (spec
//! 2026-10-21-clax-run-site-origins "Each artifact's data in a store of its
//! own"): the documents of the `db` capability, their leases, the
//! artifact's one document sequence, the documents' authors and the
//! `doc.write` and `doc.move` audit records of their changes. Locally it is
//! one SQLite file per artifact ([`Home::app_data_path`](clax_core::Home::app_data_path),
//! [`AppData::open`]); on the Worker, a Durable Object's database
//! ([`AppData::open_on`]). It holds nothing of the instance's: no people,
//! sessions, rights or approvals.
//!
//! The instance decides who the caller is and what their reader may see,
//! and hands each operation what it decided: the artifact's declared
//! capabilities (whose `db` rules the store applies to the caller), the
//! caller, the authors of the page a page's write adds ([`WriteCtx`]), the
//! audit envelope of the change ([`AuditEnv`]) and, for an agent channel,
//! its reader's view of the artifact ([`DocView`]). [`AppData::apply`]
//! runs one [`AppDataOp`], the form a host sends across a process (the
//! Worker's) boundary.
//!
//! Versions come from the artifact's sequence (`doc_seq.seq`), which every
//! change (a write, a delete, a grant that merges data, a fold's move)
//! advances in its own transaction, so a version is never issued twice in
//! the artifact's lifetime: a document that is deleted and created again
//! gets a version above every earlier one, and a pin taken before the
//! delete can only conflict. A document the caller may not read behaves as
//! absent, and a write it may not make fails as `DocNotFound`, as for a
//! missing document. Leases (`acquire`) live beside the documents and
//! coordinate only callers that use them.
//!
//! Each document changed is recorded, in the change's transaction, as
//! `doc.write` under the writer's envelope, with `op` naming the call
//! (`set`, `update`, `delete`, `str_replace`, or `acquire` for a grant that
//! merges data) and the SHA-256 of the stored JSON, never the content. A
//! write that changes nothing (deleting a missing document) records nothing.
//!
//! An artifact's rows from before it had a store of its own arrive once
//! through [`AppData::import`] ([`Exported`]), and the viewer folds the
//! instance made since (a claimed viewer's authorship and private
//! documents) through [`AppData::fold`], each applied once, by its `seq`.

mod instance;
mod migrations;
pub mod shim;
#[cfg(test)]
mod tests;
pub mod wire;

pub use instance::AppDataState;
pub(crate) use instance::log_fold;
pub use wire::{AppDataOp, AppDataReply, WireCaller, WireError};

use crate::audit::AuditRow;
use crate::docs::{
    Acquire, Acquired, BatchOp, BatchWrite, DEFAULT_LEASE_MS, DEFAULT_LIMIT, Doc, DocChange,
    DocPage, DocQuery, Filter, MAX_BATCH, MAX_DOCS, MAX_FILTERS, MAX_LEASE_MS, MAX_LEASES,
    MAX_LIMIT, MAX_UNLIMITED_QUERY_BYTES, MIN_LEASE_MS, Pin, StrReplace, Written, check_body,
    compare, merged,
};
use clax_core::audit::{AuditCtx, AuditIds, AuditKind, AuditRecord, sha256_hex};
use clax_core::db::{Caller, Op, Rules, collection_path, doc_path, invalid_argument};
use clax_core::time::stamp;
use clax_core::visibility::{Author, OWNER, UNCONNECTED, authors_of_doc, normalise};
use clax_core::working::Clock;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// How a change is recorded in the artifact's audit: the actor, the
/// envelope (`v`, `via`, the build, `git`, `git_capture`, `call`) and the
/// call and session IDs of an [`AuditCtx`], rendered as the instance's
/// audit renders them, so the artifact store needs no context of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEnv {
    /// The actor, as stored JSON.
    pub actor: String,
    pub envelope: Map<String, Value>,
    pub call: Option<String>,
    pub session: Option<String>,
}

impl AuditEnv {
    /// `ctx` rendered.
    pub fn of(ctx: &AuditCtx) -> AuditEnv {
        AuditEnv {
            actor: serde_json::to_string(&ctx.actor).expect("serialisable actor"),
            envelope: crate::audit::envelope(ctx),
            call: ctx.call.as_ref().map(|c| c.call_id.clone()),
            session: match &ctx.actor {
                clax_core::audit::Actor::Agent(a) => a.session_id.clone(),
                _ => None,
            },
        }
    }
}

/// What a write is made with besides its caller: how it is recorded, and
/// for a page's write (`Caller::via_page`) the page's authors, which it
/// adds to the document's (the current version's file authors, as stored
/// tokens: `artifacts::page_authors_in`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WriteCtx {
    pub audit: AuditEnv,
    pub page_authors: Vec<String>,
    /// The write's operation ID, when the instance records the write after
    /// it lands here ([`OpId`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<OpId>,
}

impl WriteCtx {
    /// A write under `ctx` that adds no page's authors.
    pub fn of(ctx: &AuditCtx) -> WriteCtx {
        WriteCtx {
            audit: AuditEnv::of(ctx),
            page_authors: Vec::new(),
            op: None,
        }
    }

    /// This write under operation ID `op`.
    #[must_use]
    pub fn with_op(mut self, op: OpId) -> WriteCtx {
        self.op = Some(op);
        self
    }
}

/// One of ClaxBot's document writes, journaled as its call
/// ([`Store::app_write_begin`](crate::Store::app_write_begin)): `db.set`'s
/// data, `db.update`'s patch, or `db.delete`, each with its pin.
#[derive(Clone, Debug, PartialEq)]
pub enum JournaledDoc {
    Set(Value, Pin),
    Update(Value, Pin),
    Delete(Pin),
}

/// The operation ID a document write carries when the instance records it
/// after it lands in the artifact's store (a job's publish step, a
/// ClaxBot call's journal entry): there is no transaction across the two
/// stores, so the write is made idempotent instead. The store keeps each
/// ID with `digest` (what the write asked, as the caller sums it) and the
/// write's answer, in the write's own transaction: the same write made
/// again under the ID (a retry after a crash between this write and the
/// instance's record) answers as the first did and changes nothing, and
/// another write under it is `key_reused`. A write that fails records
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpId {
    pub id: String,
    pub digest: String,
}

/// What an agent channel's reader may see of one artifact's documents: its
/// `Visibility` projected onto the artifact
/// ([`Visibility::doc_view`](crate::visibility::Visibility::doc_view)), so
/// the artifact store can withhold documents inside its scans (counts,
/// gap-free pages, byte budgets) while the rules and the reader decision
/// stay the instance's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocView {
    /// The reader's public ID; `None` for the owner.
    pub reader: Option<String>,
    /// The owner's public ID, read as [`Author::Owner`].
    pub owner: Option<String>,
    /// The author keys (public IDs, [`OWNER`], [`UNCONNECTED`]) the
    /// reader's approvals let through at the artifact.
    pub approved_keys: BTreeSet<String>,
    /// Every person's public ID, when an approval of [`UNCONNECTED`] makes
    /// the difference matter.
    pub people: Option<BTreeSet<String>>,
}

impl DocView {
    /// Whether the reader may see a document written by `authors`.
    pub fn sees(&self, authors: &[Author]) -> bool {
        authors.iter().all(|a| self.sees_one(a))
    }

    /// The authors of `authors` this view withholds, normalised.
    pub fn withheld_authors(&self, authors: &[Author]) -> Vec<Author> {
        normalise(authors, self.owner.as_deref())
            .into_iter()
            .filter(|a| !self.sees_one(a))
            .collect()
    }

    fn covers(&self, key: &str) -> bool {
        self.approved_keys.contains(key)
    }

    fn sees_one(&self, author: &Author) -> bool {
        let owner_reads = self.reader.is_none();
        match author {
            Author::System => true,
            Author::Owner => owner_reads || self.covers(OWNER),
            Author::Person(p) if self.owner.as_deref() == Some(p.as_str()) => {
                owner_reads || self.covers(OWNER)
            }
            Author::Person(p) => {
                self.reader.as_deref() == Some(p.as_str())
                    || self.covers(p)
                    || (!self.people.as_ref().is_some_and(|ps| ps.contains(p))
                        && self.covers(UNCONNECTED))
            }
            Author::Unconnected => self.covers(UNCONNECTED),
        }
    }
}

/// An artifact's rows as the instance held them before the artifact had a
/// store of its own, read in one snapshot ([`Store::export_app_data`](crate::Store::export_app_data)),
/// for [`AppData::import`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Exported {
    pub artifact_id: String,
    pub docs: Vec<ExportedDoc>,
    /// Each document's authors, `(path, author)`, in the order they first
    /// wrote.
    pub authors: Vec<(String, String)>,
    pub leases: Vec<ExportedLease>,
    /// The artifact's document sequence.
    pub doc_seq: i64,
    /// The last viewer fold the rows reflect (`app_data_folds.seq`).
    pub fold_seq: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportedDoc {
    pub path: String,
    pub collection: String,
    pub json: String,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportedLease {
    pub path: String,
    pub holder: String,
    pub expires_at: String,
}

/// One viewer fold the instance made (`app_data_folds`): viewer `old`'s
/// public ID gave way to `new`'s (into the owner when `into_owner`),
/// recorded under `audit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fold {
    pub seq: i64,
    pub old: String,
    pub new: String,
    pub into_owner: bool,
    pub audit: AuditEnv,
}

/// The checks of a batch of `writes` that need no store: 1 to
/// [`MAX_BATCH`] writes, each a document path, each document at most once,
/// each update's patch a body, and, with `sets`, each set's data a body
/// ([`AppData::doc_batch`] checks those as it writes, at their place).
/// Whatever a batch fails here it fails in every store.
///
/// # Errors
/// `invalid_argument`, or [`CoreError::InBatch`] naming the write.
pub fn check_batch(writes: &[BatchWrite], sets: bool) -> Result<()> {
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
        match &w.op {
            BatchOp::Update(p) => check_body(p, true).map_err(|e| in_batch(i, &w.path, e))?,
            BatchOp::Set(d) if sets => check_body(d, false).map_err(|e| in_batch(i, &w.path, e))?,
            _ => {}
        }
    }
    Ok(())
}

/// One artifact's app data, on one connection.
pub struct AppData {
    conn: Connection,
    clock: Arc<dyn Clock>,
}

impl AppData {
    /// Opens (creating when missing, with its directory) the store at
    /// `path`, in WAL mode, and migrates it.
    pub fn open(path: &std::path::Path, clock: Arc<dyn Clock>) -> Result<AppData> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = crate::exec::open_writer(path)?;
        AppData::open_on(conn, clock)
    }

    /// Opens the store at `path` only when it exists: `NotFound` when it
    /// does not, so no call but the import that moves an artifact's rows
    /// ([`AppData::open`]) makes a store, and a call that races its
    /// artifact's delete never makes one again.
    pub fn open_existing(path: &std::path::Path, clock: Arc<dyn Clock>) -> Result<AppData> {
        use rusqlite::OpenFlags;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = match Connection::open_with_flags(path, flags) {
            Err(rusqlite::Error::SqliteFailure(f, _))
                if f.code == rusqlite::ErrorCode::CannotOpen =>
            {
                return Err(CoreError::NotFound);
            }
            c => c?,
        };
        conn.busy_timeout(crate::exec::BUSY_TIMEOUT)?;
        AppData::open_on(conn, clock)
    }

    /// The store on `conn`, one connection its host opened (a Durable
    /// Object's), migrated; the journal mode and busy timeout are the
    /// host's.
    pub fn open_on(conn: Connection, clock: Arc<dyn Clock>) -> Result<AppData> {
        migrate(&conn)?;
        Ok(AppData { conn, clock })
    }

    fn now(&self) -> String {
        stamp(self.clock.now())
    }

    /// Runs `f` in one read transaction (one snapshot).
    fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Runs `f` in an `IMMEDIATE` transaction, committed when `f`
    /// succeeds.
    fn with_tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// [`AppData::with_tx`] for a document write made under `w`: with an
    /// operation ID ([`OpId`]) the store already holds, the answer it
    /// recorded (`key_reused` for another digest) and nothing runs; else
    /// `f`, its answer recorded under the ID in the same transaction.
    fn with_write_tx<T: Serialize + serde::de::DeserializeOwned>(
        &self,
        w: &WriteCtx,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.with_tx(|tx| {
            if let Some(op) = &w.op {
                let held: Option<(String, String)> = tx
                    .query_row(
                        "SELECT digest, reply FROM applied_ops WHERE op_id = ?1",
                        params![op.id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((digest, reply)) = held {
                    if digest != op.digest {
                        return Err(CoreError::invalid(
                            "key_reused",
                            format!("operation {} was made with other arguments", op.id),
                        ));
                    }
                    let artifact_id = AppData::artifact_in(tx)?.unwrap_or_default();
                    return serde_json::from_str(&reply).map_err(|_| CoreError::Corrupt {
                        artifact_id,
                        column: "applied_ops.reply",
                        version: None,
                    });
                }
            }
            let out = f(tx)?;
            if let Some(op) = &w.op {
                tx.execute(
                    "INSERT INTO applied_ops (op_id, digest, reply, at) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        op.id,
                        op.digest,
                        serde_json::to_string(&out).expect("a write's answer serialises"),
                        self.now()
                    ],
                )?;
            }
            Ok(out)
        })
    }

    /// The artifact this store is, once an import named it.
    fn artifact_in(c: &Connection) -> Result<Option<String>> {
        Ok(c.query_row("SELECT artifact_id FROM doc_seq", [], |r| r.get(0))?)
    }

    /// The authors of the document at `path` (spec §8.2): every writer
    /// since it was created, in the order they first wrote; the owner when
    /// none is stored (or the document does not exist).
    pub fn doc_authors(&self, path: &str) -> Result<Vec<Author>> {
        let dp = doc_path(path)?;
        self.with_read(|c| authors_in(c, &dp.path))
    }

    /// [`AppData::doc_get`] with the document's authors, read together.
    pub fn doc_get_authored(
        &self,
        rules: &Rules,
        path: &str,
        caller: &Caller,
    ) -> Result<Option<(Doc, Vec<Author>)>> {
        let dp = doc_path(path)?;
        if !rules.allows(&dp.path, Op::Read, caller) {
            return Ok(None);
        }
        self.with_read(|c| match doc_in(c, &dp.path)? {
            Some(d) => Ok(Some((d, authors_in(c, &dp.path)?))),
            None => Ok(None),
        })
    }

    /// The document at `path`, or `None` when it is absent or `caller` may
    /// not read it.
    pub fn doc_get(&self, rules: &Rules, path: &str, caller: &Caller) -> Result<Option<Doc>> {
        Ok(self.doc_get_authored(rules, path, caller)?.map(|(d, _)| d))
    }

    /// Replaces (or creates) the document with `data`.
    pub fn doc_set(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        data: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        let body = Body::new(data);
        self.with_write_tx(w, |tx| {
            let rc = Recorder { tx, w, op: "set" };
            write_in(&rc, rules, caller, path, pin, &self.now(), |_| {
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
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.doc_update_visible(w, rules, path, patch, pin, caller, None)
    }

    /// [`AppData::doc_update`] for a caller whose reader sees what `view`
    /// lets through: a merge's outcome (and its size check) depends on the
    /// stored body, so on an existing document it may write but whose
    /// authors `view` withholds it is [`CoreError::DocWithheld`] before the
    /// pin or the merge (spec §7.5, §8.3).
    ///
    /// # Errors
    /// As above, and [`AppData::doc_update`]'s.
    #[allow(clippy::too_many_arguments)]
    pub fn doc_update_visible(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        patch: Value,
        pin: Pin,
        caller: &Caller,
        view: Option<&DocView>,
    ) -> Result<Written> {
        let dp = doc_path(path)?;
        check_body(&patch, true)?;
        self.with_write_tx(w, |tx| {
            refuse_withheld(tx, rules, caller, &dp.path, view)?;
            let rc = Recorder {
                tx,
                w,
                op: "update",
            };
            write_in(&rc, rules, caller, path, pin, &self.now(), |cur| {
                Body::of(update_body(path, cur, patch))
            })
        })
    }

    /// Deletes the document; deleting a missing document succeeds with
    /// `deleted: false`.
    pub fn doc_delete(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.with_write_tx(w, |tx| {
            let rc = Recorder {
                tx,
                w,
                op: "delete",
            };
            write_in(&rc, rules, caller, path, pin, &self.now(), |_| Ok(None))
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
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
    ) -> Result<Written> {
        self.doc_str_replace_visible(w, rules, path, r, pin, caller, None)
    }

    /// [`AppData::doc_str_replace`] for a caller whose reader sees what
    /// `view` lets through: on an existing document it may write but whose
    /// authors `view` withholds, [`CoreError::DocWithheld`] before the pin,
    /// the field or `old_str` is looked at, so the answer never tells
    /// whether the text would match (spec §7.5, §8.3).
    ///
    /// # Errors
    /// As above, and [`AppData::doc_str_replace`]'s.
    #[allow(clippy::too_many_arguments)]
    pub fn doc_str_replace_visible(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        r: StrReplace,
        pin: Pin,
        caller: &Caller,
        view: Option<&DocView>,
    ) -> Result<Written> {
        let dp = doc_path(path)?;
        if r.old_str.is_empty() {
            return Err(invalid_argument("old_str must not be empty"));
        }
        self.with_write_tx(w, |tx| {
            refuse_withheld(tx, rules, caller, &dp.path, view)?;
            let rc = Recorder { tx, w, op: "str_replace" };
            write_in(&rc, rules, caller, path, pin, &self.now(), |cur| Body::of((|| {
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
        rules: &Rules,
        q: &DocQuery,
        caller: &Caller,
    ) -> Result<(Vec<Doc>, Option<String>)> {
        let page = self.doc_query_visible(rules, q, caller, None)?;
        Ok((page.docs, page.next_cursor))
    }

    /// [`AppData::doc_query`] for a caller whose reader sees what `view`
    /// lets through (spec §8.3): a document whose authors `view` withholds
    /// is never matched, ordered, sized or paged over, as if absent; the
    /// page counts the collection's documents the caller may read but
    /// `view` withholds, whatever the filter, cursor and limit. Without a
    /// view, `withheld` is `None`.
    ///
    /// # Errors
    /// [`AppData::doc_query`]'s.
    pub fn doc_query_visible(
        &self,
        rules: &Rules,
        q: &DocQuery,
        caller: &Caller,
        view: Option<&DocView>,
    ) -> Result<DocPage> {
        self.doc_query_within(rules, q, caller, view, MAX_UNLIMITED_QUERY_BYTES)
    }

    /// [`AppData::doc_query_visible`] with `budget` bytes for an ordered
    /// query without a limit.
    fn doc_query_within(
        &self,
        rules: &Rules,
        q: &DocQuery,
        caller: &Caller,
        view: Option<&DocView>,
        budget: usize,
    ) -> Result<DocPage> {
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
            let aid = AppData::artifact_in(c)?;
            let after = q.cursor.as_ref().map_or(String::new(), |cur| format!("{collection}/{cur}"));
            // A reader that may be withheld documents counts them over the
            // whole collection, so the scan starts at its first path.
            let from = if view.is_some() { "" } else { after.as_str() };
            let authors = match view {
                Some(_) => collection_authors(c, &collection)?,
                None => HashMap::new(),
            };
            let mut stmt = c.prepare(&format!("{SELECT_DOC} WHERE collection = ?1 AND path > ?2 ORDER BY path"))?;
            let rows = stmt
                .query_map(params![collection, from], row_parts)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut docs = Vec::new();
            let mut bytes = 0;
            let mut withheld = 0;
            for parts in rows {
                if !rules.allows(&parts.0, Op::Read, caller) {
                    continue;
                }
                if let Some(view) = view {
                    let tokens = authors.get(&parts.0).map_or(&[][..], Vec::as_slice);
                    if !view.sees(&authors_of_doc(tokens)) {
                        withheld += 1;
                        continue;
                    }
                    if parts.0 <= after {
                        continue;
                    }
                }
                let size = parts.2.len();
                let d = to_doc(aid.as_deref(), parts)?;
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
                return Ok(DocPage { docs, next_cursor: None, withheld: view.map(|_| withheld) });
            }
            let next_cursor = (docs.len() > limit).then(|| docs[limit - 1].id.clone());
            docs.truncate(limit);
            Ok(DocPage { docs, next_cursor, withheld: view.map(|_| withheld) })
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
        w: &WriteCtx,
        rules: &Rules,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
    ) -> Result<Vec<Written>> {
        self.doc_batch_visible(w, rules, writes, lww, caller, None)
    }

    /// [`AppData::doc_batch`] for a caller whose reader sees what `view`
    /// lets through: an `update` of a document `view` withholds fails at
    /// its place as [`CoreError::DocWithheld`] before its pin or merge, as
    /// [`AppData::doc_update_visible`]; `set` and `delete` stay blind.
    ///
    /// # Errors
    /// As above, and [`AppData::doc_batch`]'s.
    pub fn doc_batch_visible(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        writes: Vec<BatchWrite>,
        lww: bool,
        caller: &Caller,
        view: Option<&DocView>,
    ) -> Result<Vec<Written>> {
        check_batch(&writes, false)?;
        let in_batch = |op: usize, path: &str, error: CoreError| CoreError::InBatch {
            op,
            path: path.to_string(),
            error: Box::new(error),
        };
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
        self.with_write_tx(w, |tx| {
            writes
                .into_iter()
                .enumerate()
                .map(|(i, (path, if_version, op))| {
                    let pin = Pin { if_version, lww };
                    let path = path.as_str();
                    let rc = |op| Recorder { tx, w, op };
                    match op {
                        Prepared::Set(body) => {
                            write_in(&rc("set"), rules, caller, path, pin, &self.now(), |_| {
                                body.map(Some)
                            })
                        }
                        Prepared::Update(patch) => refuse_withheld(tx, rules, caller, path, view)
                            .and_then(|()| {
                                write_in(
                                    &rc("update"),
                                    rules,
                                    caller,
                                    path,
                                    pin,
                                    &self.now(),
                                    |cur| Body::of(update_body(path, cur, patch)),
                                )
                            }),
                        Prepared::Delete => {
                            write_in(&rc("delete"), rules, caller, path, pin, &self.now(), |_| {
                                Ok(None)
                            })
                        }
                    }
                    .map_err(|e| in_batch(i, path, e))
                })
                .collect()
        })
    }

    /// Grants `a.holder` the lease on `path` unless another holder's lease is
    /// still in force; a grant merges `a.data` into the document (creating it).
    /// Needs write access to `path`. Lapsed leases are pruned first; a new
    /// lease beyond [`MAX_LEASES`] in force is `resource_exhausted`.
    pub fn doc_acquire(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        a: Acquire,
        caller: &Caller,
    ) -> Result<(Acquired, Option<DocChange>)> {
        self.doc_acquire_visible(w, rules, path, a, caller, None)
    }

    /// [`AppData::doc_acquire`] for a caller whose reader sees what `view`
    /// lets through, on a document `view` withholds: with `data` (a merge)
    /// [`CoreError::DocWithheld`] before anything is looked at; without, a
    /// lease in force is busy whatever its holder, so the answer never
    /// confirms a guessed holder and never takes another's lease over.
    ///
    /// # Errors
    /// As above, and [`AppData::doc_acquire`]'s.
    pub fn doc_acquire_visible(
        &self,
        w: &WriteCtx,
        rules: &Rules,
        path: &str,
        a: Acquire,
        caller: &Caller,
        view: Option<&DocView>,
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
            if !rules.allows(&dp.path, Op::Write, caller) {
                return Err(CoreError::DocNotFound { path: dp.path.clone() });
            }
            let withheld = withheld_in(tx, rules, caller, &dp.path, view)?;
            if let Some(authors) = &withheld
                && a.data.is_some()
            {
                return Err(CoreError::DocWithheld { path: dp.path.clone(), authors: authors.clone() });
            }
            let now = self.now();
            tx.execute("DELETE FROM leases WHERE expires_at <= ?1", params![now])?;
            let held: Option<(String, String)> = tx
                .query_row(
                    "SELECT holder, expires_at FROM leases WHERE path = ?1",
                    params![dp.path],
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
                let active: i64 = tx.query_row("SELECT COUNT(*) FROM leases", [], |r| r.get(0))?;
                if active >= MAX_LEASES {
                    return Err(CoreError::invalid(
                        "resource_exhausted",
                        format!(
                            "an artifact has at most {MAX_LEASES} leases in force; let some lapse before acquiring more"
                        ),
                    ));
                }
            }
            let expires = now_plus(self.clock.now(), ttl);
            tx.execute(
                "INSERT INTO leases (path, holder, expires_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at",
                params![dp.path, a.holder, expires],
            )?;
            let empty = Value::Object(Map::new());
            let (version, change) = match a.data {
                Some(patch) => {
                    let rc = Recorder { tx, w, op: "acquire" };
                    let w = write_in(&rc, rules, caller, &dp.path, Pin { if_version: None, lww: true }, &self.now(), |cur| {
                        Body::new(merged(cur.map_or(&empty, |d| &d.data), patch)).map(Some)
                    })?;
                    (w.doc.map(|d| d.version), w.change)
                }
                None => (doc_in(tx, &dp.path)?.map(|d| d.version), None),
            };
            Ok((Acquired { acquired: true, version, expires_at: Some(expires), holder: Some(a.holder) }, change))
        })
    }

    /// Takes in `rows`, an artifact's rows from the instance, and names the
    /// store's artifact, in one transaction: its documents, authors and
    /// leases, its sequence, and the fold mark the rows reflect. Once the
    /// store names its artifact it has taken its rows in, and importing
    /// again (after a move that stopped before the instance forgot them,
    /// whose rows may since have been folded there) changes nothing: the
    /// store applies the folds past its own mark ([`AppData::fold`]).
    pub fn import(&self, rows: Exported) -> Result<()> {
        self.with_tx(|tx| {
            if AppData::artifact_in(tx)?.is_some() {
                return Ok(());
            }
            tx.execute(
                "UPDATE doc_seq SET artifact_id = ?1, seq = MAX(seq, ?2), folded = MAX(folded, ?3)",
                params![rows.artifact_id, rows.doc_seq, rows.fold_seq],
            )?;
            for d in &rows.docs {
                tx.execute(
                    "INSERT OR IGNORE INTO docs (path, collection, json, version, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![d.path, d.collection, d.json, d.version, d.updated_at],
                )?;
            }
            for (path, author) in &rows.authors {
                tx.execute(
                    "INSERT OR IGNORE INTO doc_authors (path, author) VALUES (?1, ?2)",
                    params![path, author],
                )?;
            }
            for l in &rows.leases {
                tx.execute(
                    "INSERT OR IGNORE INTO leases (path, holder, expires_at) VALUES (?1, ?2, ?3)",
                    params![l.path, l.holder, l.expires_at],
                )?;
            }
            // A row written outside the sequence never repeats a version.
            tx.execute(
                "UPDATE doc_seq SET seq = MAX(seq, COALESCE((SELECT MAX(version) FROM docs), 0))",
                [],
            )?;
            Ok(())
        })
    }

    /// The last viewer fold applied ([`AppData::fold`]).
    pub fn fold_mark(&self) -> Result<i64> {
        self.with_read(|c| Ok(c.query_row("SELECT folded FROM doc_seq", [], |r| r.get(0))?))
    }

    /// Applies each of `folds` above the store's fold mark, in `seq` order,
    /// in one transaction, and returns the new mark. A fold moves viewer
    /// `old`'s authorship to `new` (to [`OWNER`] when `into_owner`), each
    /// author once per document, and moves every `data/users/<old>/…`
    /// document to `data/users/<new>/…` with the next version, its authors
    /// with it, its lease dropped, and records the move as `doc.move` (both
    /// paths and the document's hash, never its content); a document whose
    /// destination exists stays where it is (and is logged).
    pub fn fold(&self, folds: &[Fold]) -> Result<i64> {
        self.with_tx(|tx| {
            let mut mark: i64 = tx.query_row("SELECT folded FROM doc_seq", [], |r| r.get(0))?;
            let aid = AppData::artifact_in(tx)?;
            let mut folds: Vec<&Fold> = folds.iter().filter(|f| f.seq > mark).collect();
            folds.sort_by_key(|f| f.seq);
            for f in folds {
                fold_in(tx, aid.as_deref(), f, &self.now())?;
                mark = f.seq;
            }
            tx.execute("UPDATE doc_seq SET folded = ?1", params![mark])?;
            Ok(mark)
        })
    }

    /// Every author a document of the artifact names, once each, in order.
    pub fn authors(&self) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut stmt =
                c.prepare_cached("SELECT DISTINCT author FROM doc_authors ORDER BY author")?;
            Ok(stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The paths of the documents `author` (a public ID; with `is_owner`,
    /// the owner's, which also holds every document without recorded
    /// authors) holds fields of, in path order.
    pub fn authored_by(&self, author: &str, is_owner: bool) -> Result<Vec<String>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT d.path FROM docs d
                  WHERE EXISTS (SELECT 1 FROM doc_authors x WHERE x.path = d.path
                                  AND (x.author = ?1 OR (?2 AND x.author = '*owner')))
                     OR (?2 AND NOT EXISTS (SELECT 1 FROM doc_authors x WHERE x.path = d.path))
                  ORDER BY d.path",
            )?
            .query_map(params![author, is_owner], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The artifact's document sequence (`doc_seq.seq`): every change to its
    /// documents (a write, a delete, a fold's move) advances it, so a read
    /// at an unchanged sequence finds what it found before.
    pub fn doc_seq(&self) -> Result<i64> {
        self.with_read(|c| Ok(c.query_row("SELECT seq FROM doc_seq", [], |r| r.get(0))?))
    }

    /// Each collection holding documents, with its document count, in name
    /// order. With `caller`, only the collections that hold a document it
    /// may read (by `rules`), each counted whole; without, every collection
    /// but those under a private per-viewer prefix (`data/users/<id>` and
    /// each `{self}` rule's prefix not opened by a rule at it), so no
    /// viewer's public ID or private count shows.
    pub fn doc_collections(
        &self,
        rules: &Rules,
        caller: Option<&Caller>,
    ) -> Result<Vec<(String, u64)>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT collection, COUNT(*), MIN(path) FROM docs GROUP BY collection ORDER BY collection",
            )?;
            let rows = q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (collection, n, first) = row?;
                let shown = match caller {
                    Some(caller) => rules.allows(&first, Op::Read, caller),
                    None => rules.private_to(&collection).is_none(),
                };
                if shown {
                    out.push((collection, n as u64));
                }
            }
            Ok(out)
        })
    }

    /// The documents `caller` may read by `rules`, in path order, as stored,
    /// until `max_docs` documents or `max_bytes` of stored JSON; and whether
    /// any were left. What a draft's copy of its app takes
    /// ([`AppData::seed`]).
    pub fn copy_out(
        &self,
        rules: &Rules,
        caller: &Caller,
        max_docs: usize,
        max_bytes: usize,
    ) -> Result<(Vec<ExportedDoc>, bool)> {
        self.with_read(|c| {
            let mut q = c.prepare(
                "SELECT path, collection, json, version, updated_at FROM docs ORDER BY path",
            )?;
            let mut rows = q.query([])?;
            let (mut out, mut bytes) = (Vec::new(), 0usize);
            while let Some(r) = rows.next()? {
                let path: String = r.get(0)?;
                if !rules.allows(&path, Op::Read, caller) {
                    continue;
                }
                let json: String = r.get(2)?;
                if out.len() == max_docs || bytes + json.len() > max_bytes {
                    return Ok((out, true));
                }
                bytes += json.len();
                out.push(ExportedDoc {
                    path,
                    collection: r.get(1)?,
                    json,
                    version: r.get(3)?,
                    updated_at: r.get(4)?,
                });
            }
            Ok((out, false))
        })
    }

    /// Puts `docs` in a store that holds no document, each at version 1 at
    /// `now`, with no authors and the sequence at 1 (none when `docs` is
    /// empty): a draft's copy of its app's documents. `invalid_args` when the
    /// store holds a document already.
    pub fn seed(&self, docs: &[ExportedDoc]) -> Result<usize> {
        self.with_tx(|tx| {
            let any: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM docs)", [], |r| r.get(0))?;
            if any {
                return Err(CoreError::invalid(
                    "invalid_args",
                    "only a store with no documents takes a copy",
                ));
            }
            let now = self.now();
            for d in docs {
                tx.execute(
                    "INSERT INTO docs (path, collection, json, version, updated_at)
                     VALUES (?1, ?2, ?3, 1, ?4)",
                    params![d.path, d.collection, d.json, now],
                )?;
            }
            if !docs.is_empty() {
                tx.execute("UPDATE doc_seq SET seq = MAX(seq, 1)", [])?;
            }
            Ok(docs.len())
        })
    }

    /// The newest audit record's `seq` (0 when there is none).
    pub fn audit_seq(&self) -> Result<i64> {
        self.with_read(|c| {
            Ok(
                c.query_row("SELECT COALESCE(MAX(seq), 0) FROM audit_events", [], |r| {
                    r.get(0)
                })?,
            )
        })
    }

    /// The audit records with a `seq` above `seq`, in `seq` order. Their
    /// `seq` is this store's own, not the instance's.
    pub fn audit_since(&self, seq: i64) -> Result<Vec<AuditRow>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT seq, at, kind, actor, artifact_id, session_id, call_id, body, backfilled
                 FROM audit_events WHERE seq > ?1 ORDER BY seq",
            )?;
            Ok(q.query_map(params![seq], |r| {
                Ok(AuditRow {
                    seq: r.get(0)?,
                    at: r.get(1)?,
                    kind: r.get(2)?,
                    actor: r.get(3)?,
                    ids: AuditIds {
                        artifact: r.get(4)?,
                        session: r.get(5)?,
                        call: r.get(6)?,
                        ..AuditIds::default()
                    },
                    body: r.get(7)?,
                    backfilled: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
        })
    }
}

/// Applies every migration of [`migrations::MIGRATIONS`] past the
/// database's `user_version`, each in its own `IMMEDIATE` transaction that
/// re-reads the version, so two openers apply each once. Refuses a store
/// whose schema is newer than this binary knows.
fn migrate(conn: &Connection) -> Result<()> {
    let known = migrations::MIGRATIONS.len() as u32;
    // A store at the current schema takes no write lock to find that out.
    if conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? == known {
        return Ok(());
    }
    loop {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > known {
            return Err(CoreError::SchemaNewer {
                found: version,
                known,
            });
        }
        if version == known {
            tx.commit()?;
            return Ok(());
        }
        tx.execute_batch(migrations::MIGRATIONS[version as usize])?;
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
    }
}

/// `ms` milliseconds after `now`, as the store writes times.
fn now_plus(now: chrono::DateTime<chrono::Utc>, ms: u64) -> String {
    stamp(now + chrono::Duration::milliseconds(ms as i64))
}

type Parts = (String, String, String, i64, String);

const SELECT_DOC: &str = "SELECT path, collection, json, version, updated_at FROM docs";

fn row_parts(r: &rusqlite::Row<'_>) -> rusqlite::Result<Parts> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

fn to_doc(aid: Option<&str>, (path, collection, json, version, updated_at): Parts) -> Result<Doc> {
    let data = serde_json::from_str(&json).map_err(|_| CoreError::Corrupt {
        artifact_id: aid.unwrap_or_default().to_string(),
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

fn doc_in(c: &Connection, path: &str) -> Result<Option<Doc>> {
    let aid = AppData::artifact_in(c)?;
    c.query_row(
        &format!("{SELECT_DOC} WHERE path = ?1"),
        params![path],
        row_parts,
    )
    .optional()?
    .map(|p| to_doc(aid.as_deref(), p))
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
fn next_version(c: &Connection, floor: u64) -> Result<u64> {
    let v: i64 = c.query_row(
        "UPDATE doc_seq SET seq = MAX(seq, ?1) + 1 RETURNING seq",
        params![floor as i64],
        |r| r.get(0),
    )?;
    Ok(v as u64)
}

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

/// A batch write's operation, with a set body checked and serialised ahead
/// of the transaction.
enum Prepared {
    Set(Result<Body>),
    Update(Value),
    Delete,
}

/// Inserts one audit record, made under `env`, in `c`.
fn record_in(c: &Connection, env: &AuditEnv, aid: Option<&str>, rec: AuditRecord) -> Result<()> {
    let mut body = rec.body;
    body.extend(env.envelope.clone());
    c.prepare_cached(
        "INSERT INTO audit_events (at, kind, actor, artifact_id, session_id, call_id, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?
    .execute(params![
        rec.at,
        rec.kind.as_str(),
        env.actor,
        aid,
        rec.ids.session.or_else(|| env.session.clone()),
        rec.ids.call.or_else(|| env.call.clone()),
        serde_json::to_string(&body).expect("serialisable body"),
    ])?;
    Ok(())
}

/// Where a write is recorded: `doc.write` with `op`, under `w`'s envelope,
/// in the write's transaction `tx`.
struct Recorder<'a> {
    tx: &'a Transaction<'a>,
    w: &'a WriteCtx,
    op: &'static str,
}

impl Recorder<'_> {
    /// Records the change to `dp` (spec §6.1): its new version and the
    /// SHA-256 of its stored JSON (`None`, a delete), never the content.
    fn record(
        &self,
        dp: &clax_core::db::DocPath,
        version: Option<u64>,
        json: Option<&str>,
        at: &str,
    ) -> Result<()> {
        let rec = AuditRecord::new(AuditKind::DocWrite, at)
            .with("collection", dp.collection.as_str())
            .with("doc_id", dp.id.as_str())
            .with("version", version)
            .with("op", self.op)
            .with(
                "sha256",
                json.map(|j| format!("sha256:{}", sha256_hex(j.as_bytes()))),
            );
        let aid = AppData::artifact_in(self.tx)?;
        record_in(self.tx, &self.w.audit, aid.as_deref(), rec)
    }
}

/// One write: refused as `DocNotFound` unless `rules` let `caller` write
/// `path`; pinned by `pin`; `next` maps the current document to the new
/// body (`None` deletes).
fn write_in(
    rc: &Recorder<'_>,
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
    let current = doc_in(c, &dp.path)?;
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
                let n: i64 = c.query_row("SELECT COUNT(*) FROM docs", [], |r| r.get(0))?;
                if n >= MAX_DOCS {
                    return Err(CoreError::invalid(
                        "quota_exceeded",
                        format!(
                            "an artifact's database holds at most {MAX_DOCS} documents; delete some before creating more"
                        ),
                    ));
                }
            }
            let version = next_version(c, current.as_ref().map_or(0, |d| d.version))?;
            c.execute(
                "INSERT INTO docs (path, collection, json, version, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(path) DO UPDATE SET json = excluded.json, version = excluded.version, updated_at = excluded.updated_at",
                params![dp.path, dp.collection, body.json, version as i64, now],
            )?;
            rc.record(&dp, Some(version), Some(&body.json), now)?;
            add_authors(c, &dp.path, caller, &rc.w.page_authors, current.is_some())?;
            let authors = authors_in(c, &dp.path)?;
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
            let n = c.execute("DELETE FROM docs WHERE path = ?1", params![dp.path])?;
            if n > 0 {
                let version = next_version(c, current.as_ref().map_or(0, |d| d.version))?;
                rc.record(&dp, Some(version), None, now)?;
                // A new document at the path starts a new author set.
                c.execute("DELETE FROM doc_authors WHERE path = ?1", params![dp.path])?;
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
/// §8.2, M11): the writer, and for a page's write every author of the page
/// (`page_authors`, of every file of its current version at the write).
/// The set only grows: touching or rewriting someone else's document never
/// makes it the writer's alone. A document with no rows is the owner's, so
/// while every author added is the owner nothing is stored; the first
/// other author makes the owner's authorship of an existing (`existed`)
/// document explicit before joining it.
fn add_authors(
    c: &Connection,
    path: &str,
    caller: &Caller,
    page_authors: &[String],
    existed: bool,
) -> Result<()> {
    let mut add: Vec<String> = Vec::with_capacity(2);
    if let Some(t) = caller.person.token() {
        add.push(t.to_string());
    }
    if caller.via_page {
        for a in page_authors {
            if !add.contains(a) {
                add.push(a.clone());
            }
        }
    }
    let rows: bool = c
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM doc_authors WHERE path = ?1)")?
        .query_row(params![path], |r| r.get(0))?;
    if !rows {
        if add.iter().all(|a| a == OWNER) {
            return Ok(());
        }
        if existed {
            add.insert(0, OWNER.to_string());
        }
    }
    let mut stmt =
        c.prepare_cached("INSERT OR IGNORE INTO doc_authors (path, author) VALUES (?1, ?2)")?;
    for a in add {
        stmt.execute(params![path, a])?;
    }
    Ok(())
}

/// The authors of the document at `path` (see [`AppData::doc_authors`]).
fn authors_in(c: &Connection, path: &str) -> Result<Vec<Author>> {
    let mut stmt =
        c.prepare_cached("SELECT author FROM doc_authors WHERE path = ?1 ORDER BY rowid")?;
    let tokens = stmt
        .query_map(params![path], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(authors_of_doc(&tokens))
}

/// The stored authors of every document of `collection`, by path; a
/// document with none is absent (the owner's).
fn collection_authors(c: &Connection, collection: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = c.prepare_cached(
        "SELECT a.path, a.author FROM doc_authors a
         JOIN docs d ON d.path = a.path
         WHERE d.collection = ?1 ORDER BY a.rowid",
    )?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for row in stmt.query_map(params![collection], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (path, author) = row?;
        out.entry(path).or_default().push(author);
    }
    Ok(out)
}

/// The authors `view` withholds of the existing document at `path` that
/// `caller` may write; `None` when there is none, the rules hide it (it
/// then answers as missing), or `view` lets it through.
fn withheld_in(
    c: &Connection,
    rules: &Rules,
    caller: &Caller,
    path: &str,
    view: Option<&DocView>,
) -> Result<Option<Vec<Author>>> {
    let Some(view) = view else {
        return Ok(None);
    };
    if !rules.allows(path, Op::Write, caller) || doc_in(c, path)?.is_none() {
        return Ok(None);
    }
    let withheld = view.withheld_authors(&authors_in(c, path)?);
    Ok((!withheld.is_empty()).then_some(withheld))
}

/// [`CoreError::DocWithheld`] for a write whose outcome depends on the
/// document's content (`str_replace`, a merge) when `view` withholds it:
/// refused before the pin or the content is looked at (spec §7.5, §8.3).
fn refuse_withheld(
    c: &Connection,
    rules: &Rules,
    caller: &Caller,
    path: &str,
    view: Option<&DocView>,
) -> Result<()> {
    match withheld_in(c, rules, caller, path, view)? {
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

/// Applies fold `f` (see [`AppData::fold`]) in `tx`, at `now`.
fn fold_in(tx: &Transaction<'_>, aid: Option<&str>, f: &Fold, now: &str) -> Result<()> {
    let token = if f.into_owner { OWNER } else { f.new.as_str() };
    tx.execute(
        "UPDATE OR IGNORE doc_authors SET author = ?2 WHERE author = ?1",
        params![f.old, token],
    )?;
    tx.execute("DELETE FROM doc_authors WHERE author = ?1", params![f.old])?;
    let from = format!("data/users/{}/", f.old);
    let to = format!("data/users/{}/", f.new);
    let rows: Vec<(String, String, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT path, json, version FROM docs WHERE substr(path, 1, length(?1)) = ?1 ORDER BY path",
        )?;
        stmt.query_map(params![from], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (path, json, current) in rows {
        let dest = format!("{to}{}", &path[from.len()..]);
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM docs WHERE path = ?1)",
            params![dest],
            |r| r.get(0),
        )?;
        if taken {
            tracing::warn!(artifact = ?aid, %path, %dest, "a private document stays at its old path: the owner already has one there");
            continue;
        }
        let dp = doc_path(&dest)?;
        let version = next_version(tx, current as u64)?;
        tx.execute(
            "UPDATE docs SET path = ?2, collection = ?3, version = ?4, updated_at = ?5 WHERE path = ?1",
            params![path, dest, dp.collection, version as i64, now],
        )?;
        let rec = AuditRecord::new(AuditKind::DocMove, now)
            .with("from", path.as_str())
            .with("to", dest.as_str())
            .with("collection", dp.collection.as_str())
            .with("doc_id", dp.id.as_str())
            .with("version", version)
            .with("sha256", format!("sha256:{}", sha256_hex(json.as_bytes())));
        record_in(tx, &f.audit, aid, rec)?;
        tx.execute("DELETE FROM leases WHERE path = ?1", params![path])?;
        tx.execute(
            "UPDATE doc_authors SET path = ?2 WHERE path = ?1",
            params![path, dest],
        )?;
    }
    Ok(())
}
