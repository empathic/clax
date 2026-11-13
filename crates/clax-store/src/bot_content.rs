//! A ClaxBot run's content in the store (migration 65's `bot_content`;
//! `docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
//! "Content: images, files and their metadata").
//!
//! - **An item** is a row of `bot_content` naming the run that holds it
//!   and what brought it (the surface and the registration that uploaded
//!   it; the operation, key and step of the call it first answered), with
//!   Clax's findings about its bytes (`media_type`, `bytes`, `sha256`,
//!   `width` and `height`) and the provider's own words (`name`,
//!   `meta_json`). Its `id` is the handle's, unguessable, so knowing it is
//!   no authority: every read names the run too.
//! - **Its bytes** are a blob of the content store, named by their SHA-256
//!   ([`clax_core::content::ContentStore::put_blob`]): items of the same
//!   bytes share one. A store that keeps no blobs keeps no content
//!   ([`Store::keeps_bot_content`]).
//! - **Limits** ([`ContentLimits`]): an item's bytes, a run's together and
//!   all runs' together, checked in the transaction that writes the row,
//!   and readable before the bytes are ([`Store::bot_content_room`]).
//! - **Pruned with the run's call journal** ([`Store::prune_ended_runs`]):
//!   the rows of a run that ended over
//!   [`crate::bot_runs::BOT_JOURNAL_KEEP`] ago go, and so does each blob no
//!   row names any more.
//!
//! A row and its blob are written in one write transaction of the
//! store's. A prune deletes rows in its transaction and removes the blobs
//! no row names once it has committed, each checked again in a write
//! transaction of its own: writes are taken one at a time, so an upload of
//! some bytes and the removal of the same bytes never interleave, and a
//! prune rolled back removes no blob. A blob named by no row (written for
//! a row whose transaction then failed, or left by a crash between a
//! prune's commit and its removals) is removed by the next prune
//! ([`Store::release_blobs`]).

use super::Store;
use clax_core::content::is_blob_name;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value, json};

/// The most bytes of one item of content, by default: 20 MiB.
pub const CONTENT_ITEM_BYTES: u64 = 20 << 20;
/// The most bytes of all the content of one run, by default: 100 MiB.
pub const CONTENT_RUN_BYTES: u64 = 100 << 20;
/// The most bytes of the content of all runs together, by default: 1 GiB.
pub const CONTENT_TOTAL_BYTES: u64 = 1 << 30;

/// The code of an upload larger than [`ContentLimits::item_bytes`].
pub const CODE_CONTENT_TOO_LARGE: &str = "content_too_large";
/// The code of an upload that would take a run's content past
/// [`ContentLimits::run_bytes`].
pub const CODE_RUN_CONTENT_FULL: &str = "run_content_full";
/// The code of an upload that would take the content all runs hold
/// together past [`ContentLimits::total_bytes`].
pub const CODE_CONTENT_STORE_FULL: &str = "content_store_full";
/// The code of a write of content to a store that keeps none.
pub const CODE_NO_CONTENT_STORE: &str = "no_content_store";

/// How much content an item and a run may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentLimits {
    /// The most bytes of one item.
    pub item_bytes: u64,
    /// The most bytes of all of one run's items together.
    pub run_bytes: u64,
    /// The most bytes of the items of all runs together, as their rows
    /// count them (items of the same bytes share one blob, so the disk
    /// holds no more than this).
    pub total_bytes: u64,
}

impl Default for ContentLimits {
    fn default() -> Self {
        ContentLimits {
            item_bytes: CONTENT_ITEM_BYTES,
            run_bytes: CONTENT_RUN_BYTES,
            total_bytes: CONTENT_TOTAL_BYTES,
        }
    }
}

/// How many more bytes an item of one run may have, by each of
/// [`ContentLimits`]' bounds ([`Store::bot_content_room`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentRoom {
    /// [`ContentLimits::item_bytes`].
    pub item: u64,
    /// What is left of [`ContentLimits::run_bytes`] for the run.
    pub run: u64,
    /// What is left of [`ContentLimits::total_bytes`].
    pub total: u64,
}

impl ContentRoom {
    /// The most bytes the next item may have, and the code of the bound
    /// that sets it ([`CODE_CONTENT_TOO_LARGE`], [`CODE_RUN_CONTENT_FULL`]
    /// or [`CODE_CONTENT_STORE_FULL`]), which an item over it is refused
    /// with.
    #[must_use]
    pub fn most(&self) -> (u64, &'static str) {
        if self.total < self.run.min(self.item) {
            (self.total, CODE_CONTENT_STORE_FULL)
        } else if self.run < self.item {
            (self.run, CODE_RUN_CONTENT_FULL)
        } else {
            (self.item, CODE_CONTENT_TOO_LARGE)
        }
    }

    /// Refuses an item of `size` bytes past [`ContentRoom::most`], with
    /// the code of the bound it passes and a message that says it.
    ///
    /// # Errors
    /// [`CoreError::Invalid`] with that code.
    pub fn fits(&self, size: u64, limits: ContentLimits) -> Result<()> {
        let (most, code) = self.most();
        if size <= most {
            return Ok(());
        }
        let message = match code {
            CODE_CONTENT_STORE_FULL => format!(
                "the content of all runs together is {} bytes at the most, and {} more \
                 fit; this is {size}",
                limits.total_bytes, self.total
            ),
            CODE_RUN_CONTENT_FULL => format!(
                "a run's content is {} bytes at the most, and {} more fit in this run; \
                 this is {size}",
                limits.run_bytes, self.run
            ),
            _ => format!(
                "an item of content is {} bytes at the most; this is {size}",
                limits.item_bytes
            ),
        };
        Err(CoreError::invalid(code, message))
    }
}

/// The room left for an item of `run_id` under `limits`, on `c`.
fn room_in(c: &Connection, run_id: &str, limits: ContentLimits) -> Result<ContentRoom> {
    let (run, all): (u64, u64) = c
        .prepare_cached(
            "SELECT COALESCE(SUM(CASE WHEN run_id = ?1 THEN bytes END), 0),
                    COALESCE(SUM(bytes), 0)
             FROM bot_content",
        )?
        .query_row(params![run_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(ContentRoom {
        item: limits.item_bytes,
        run: limits.run_bytes.saturating_sub(run),
        total: limits.total_bytes.saturating_sub(all),
    })
}

/// One item of a run's content, as its row has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentRow {
    /// The handle's ID.
    pub id: String,
    pub run_id: String,
    /// The surface that brought it.
    pub surface: String,
    /// The registration that uploaded it, for a surface a host registered.
    pub registration_id: Option<String>,
    /// The operation whose answer first named it (`shell.screenshot`).
    pub op: Option<String>,
    /// That call's key (`<run>:<step>:<seq>`).
    pub key: Option<String>,
    /// That call's step.
    pub step_n: Option<i64>,
    /// A file name, as the provider gave it.
    pub name: Option<String>,
    /// The SHA-256 of the bytes, in lowercase hex: the blob's name.
    pub sha256: String,
    pub bytes: u64,
    /// As Clax determined it.
    pub media_type: String,
    /// An image's displayed size, as `clax_core::media::sniff` read it.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The provider's own object, from the first answer that named it.
    pub meta: Option<Value>,
    /// Whether the model was shown it.
    pub shown: bool,
    pub created_at: String,
}

impl ContentRow {
    /// The whole handle a script holds of the item: `id`, `media_type`,
    /// `bytes`, `sha256`, and `name`, `width`, `height` and `meta` when it
    /// has them, in that order.
    #[must_use]
    pub fn handle(&self) -> Value {
        let mut h = Map::new();
        h.insert("id".into(), json!(self.id));
        h.insert("media_type".into(), json!(self.media_type));
        h.insert("bytes".into(), json!(self.bytes));
        h.insert("sha256".into(), json!(self.sha256));
        if let Some(name) = &self.name {
            h.insert("name".into(), json!(name));
        }
        if let Some(w) = self.width {
            h.insert("width".into(), json!(w));
        }
        if let Some(hgt) = self.height {
            h.insert("height".into(), json!(hgt));
        }
        if let Some(meta) = &self.meta {
            h.insert("meta".into(), meta.clone());
        }
        Value::Object(h)
    }
}

/// A call a run sent to a registration, as the store keeps it
/// ([`Store::bot_calls_naming`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SentCall {
    /// The hash of the definition the call was sent under.
    pub def_hash: String,
    /// The operation, as the run named it (`lens.keep`).
    pub op: String,
    /// The arguments, one for each parameter, as they were sent.
    pub args: Value,
}

/// An item to keep ([`Store::put_bot_content`]): its bytes, what Clax
/// found them to be, and who brought them for which run.
#[derive(Clone, Debug)]
pub struct NewContent<'a> {
    /// The handle's ID: the caller draws it, unguessable.
    pub id: &'a str,
    pub run_id: &'a str,
    pub surface: &'a str,
    pub registration_id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub media_type: &'a str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: &'a [u8],
}

const COLS: &str = "id, run_id, surface, registration_id, op, key, step_n, name, sha256, bytes, \
                    media_type, width, height, meta_json, shown, created_at";

fn row_to_content(r: &Row<'_>) -> rusqlite::Result<ContentRow> {
    let meta: Option<String> = r.get(13)?;
    Ok(ContentRow {
        id: r.get(0)?,
        run_id: r.get(1)?,
        surface: r.get(2)?,
        registration_id: r.get(3)?,
        op: r.get(4)?,
        key: r.get(5)?,
        step_n: r.get(6)?,
        name: r.get(7)?,
        sha256: r.get(8)?,
        bytes: r.get(9)?,
        media_type: r.get(10)?,
        width: r.get(11)?,
        height: r.get(12)?,
        meta: meta.and_then(|m| serde_json::from_str(&m).ok()),
        shown: r.get(14)?,
        created_at: r.get(15)?,
    })
}

fn content_in(c: &Connection, id: &str) -> Result<Option<ContentRow>> {
    Ok(
        c.prepare_cached(&format!("SELECT {COLS} FROM bot_content WHERE id = ?1"))?
            .query_row(params![id], row_to_content)
            .optional()?,
    )
}

/// Deletes the content of each run that ended before `before`, on `tx`:
/// the rows deleted, and the blobs that no row names any more, which the
/// caller removes once the transaction has committed ([`Store::release_blobs`]).
pub(crate) fn prune_content_in(c: &Connection, before: &str) -> Result<(usize, Vec<String>)> {
    let ended = "SELECT id FROM bot_runs WHERE ended_at IS NOT NULL AND ended_at < ?1";
    let named: Vec<String> = c
        .prepare(&format!(
            "SELECT DISTINCT sha256 FROM bot_content WHERE run_id IN ({ended})"
        ))?
        .query_map(params![before], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let rows = c.execute(
        &format!("DELETE FROM bot_content WHERE run_id IN ({ended})"),
        params![before],
    )?;
    let mut released = Vec::new();
    for sha in named {
        let still: bool = c
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM bot_content WHERE sha256 = ?1)")?
            .query_row(params![sha], |r| r.get(0))?;
        if !still {
            released.push(sha);
        }
    }
    Ok((rows, released))
}

impl Store {
    /// Whether this store keeps a run's content: its content store keeps
    /// blobs ([`clax_core::content::ContentStore::keeps_blobs`]).
    #[must_use]
    pub fn keeps_bot_content(&self) -> bool {
        self.content().keeps_blobs()
    }

    /// Keeps `new` as an item of its run's content, within `limits`: its
    /// blob (named by the SHA-256 of its bytes, which this computes) and
    /// its row, in one write transaction. Answers the row.
    ///
    /// # Errors
    /// [`CODE_NO_CONTENT_STORE`] for a store that keeps no content;
    /// [`CODE_CONTENT_TOO_LARGE`] for bytes over `limits.item_bytes`;
    /// [`CODE_RUN_CONTENT_FULL`] when the run's items would together pass
    /// `limits.run_bytes`; [`CODE_CONTENT_STORE_FULL`] when all runs'
    /// would pass `limits.total_bytes`; the content store's and the
    /// store's. Nothing is kept on any of them.
    pub fn put_bot_content(
        &self,
        new: &NewContent<'_>,
        limits: ContentLimits,
    ) -> Result<ContentRow> {
        if !self.keeps_bot_content() {
            return Err(CoreError::invalid(
                CODE_NO_CONTENT_STORE,
                "this instance keeps no content for ClaxBot runs",
            ));
        }
        let size = u64::try_from(new.bytes.len()).unwrap_or(u64::MAX);
        if size > limits.item_bytes {
            return Err(CoreError::invalid(
                CODE_CONTENT_TOO_LARGE,
                format!(
                    "an item of content is {} bytes at the most; this is {size}",
                    limits.item_bytes
                ),
            ));
        }
        let sha256 = clax_core::audit::sha256_hex(new.bytes);
        let at = self.now();
        self.with_tx(|tx| {
            room_in(tx, new.run_id, limits)?.fits(size, limits)?;
            tx.execute(
                "INSERT INTO bot_content (id, run_id, surface, registration_id, name, sha256,
                    bytes, media_type, width, height, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    new.id,
                    new.run_id,
                    new.surface,
                    new.registration_id,
                    new.name,
                    sha256,
                    size,
                    new.media_type,
                    new.width,
                    new.height,
                    at
                ],
            )?;
            self.block_on(self.content().put_blob(&sha256, new.bytes))?;
            content_in(tx, new.id)?.ok_or(CoreError::NotFound)
        })
    }

    /// The room left for an item of `run_id` under `limits`, as the
    /// store holds now: what an upload is checked against before its
    /// bytes are read. [`Store::put_bot_content`] checks again in the
    /// transaction that writes the row.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_content_room(&self, run_id: &str, limits: ContentLimits) -> Result<ContentRoom> {
        self.with_read(|c| room_in(c, run_id, limits))
    }

    /// The item `id`, whichever run holds it; `None` when no row is.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_content(&self, id: &str) -> Result<Option<ContentRow>> {
        self.with_read(|c| content_in(c, id))
    }

    /// The item `id` when `run_id` holds it; `None` otherwise, whoever
    /// else does.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_content_of(&self, run_id: &str, id: &str) -> Result<Option<ContentRow>> {
        Ok(self.bot_content(id)?.filter(|row| row.run_id == run_id))
    }

    /// The bytes of `row`'s item, from its blob; `None` when the blob is
    /// not there.
    ///
    /// # Errors
    /// The content store's.
    pub fn bot_content_bytes(&self, row: &ContentRow) -> Result<Option<Vec<u8>>> {
        if !is_blob_name(&row.sha256) {
            return Ok(None);
        }
        self.block_on(self.content().read_blob(&row.sha256))
    }

    /// Records what first named the item `id` in a call's answer: the
    /// operation, the call's key and step, and the provider's `meta`. A
    /// field already recorded is kept.
    ///
    /// # Errors
    /// The store's.
    pub fn note_bot_content(
        &self,
        id: &str,
        op: &str,
        key: &str,
        step_n: i64,
        meta: Option<&Value>,
    ) -> Result<()> {
        let meta = meta.map(Value::to_string);
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE bot_content SET op = COALESCE(op, ?2), key = COALESCE(key, ?3),
                    step_n = COALESCE(step_n, ?4), meta_json = COALESCE(meta_json, ?5)
                 WHERE id = ?1",
                params![id, op, key, step_n, meta],
            )?;
            Ok(())
        })
    }

    /// Records that the model was shown the items `ids` of `run_id`.
    ///
    /// # Errors
    /// The store's.
    pub fn mark_bot_content_shown(&self, run_id: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.with_tx(|tx| {
            for id in ids {
                tx.execute(
                    "UPDATE bot_content SET shown = 1 WHERE id = ?1 AND run_id = ?2",
                    params![id, run_id],
                )?;
            }
            Ok(())
        })
    }

    /// The calls of `row`'s run to the registration `registration_id`
    /// whose arguments name the item's ID anywhere in their text: each
    /// one's definition hash, operation and arguments as they were sent.
    /// Whether one of them passed a handle of the item is for whoever
    /// reads the arguments by the operation's declared types
    /// (`clax_http::bot_surfaces::Surfaces::passed`): the text alone
    /// cannot tell a `Content` or `Image` argument from a copy of one in
    /// an `unknown` argument or a handle's `meta`. Pending calls are
    /// pruned with the content, so these are there for as long as the
    /// item is.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_calls_naming(
        &self,
        row: &ContentRow,
        registration_id: &str,
    ) -> Result<Vec<SentCall>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT def_hash, op, args_json FROM bot_pending_calls
                 WHERE run_id = ?1 AND registration_id = ?2 AND instr(args_json, ?3) > 0
                 ORDER BY created_at, id",
            )?;
            let calls = q.query_map(params![row.run_id, registration_id, row.id], |r| {
                Ok(SentCall {
                    def_hash: r.get(0)?,
                    op: r.get(1)?,
                    args: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
                })
            })?;
            Ok(calls.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Whether the registration `registration_id` has a call of `run_id`
    /// that is not answered yet: what lets its host upload content for
    /// the run.
    ///
    /// # Errors
    /// The store's.
    pub fn serves_open_call(&self, run_id: &str, registration_id: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM bot_pending_calls
                 WHERE run_id = ?1 AND registration_id = ?2 AND answered_at IS NULL)",
            )?
            .query_row(params![run_id, registration_id], |r| r.get(0))?)
        })
    }

    /// Removes the blobs `released`, which no row names, in the write
    /// transaction that released them ([`prune_content_in`]).
    /// Removes each blob no row of `bot_content` names: those `released`
    /// (the ones a prune's committed transaction left unnamed) and any
    /// other the content store keeps ([`clax_core::content::ContentStore::blob_names`]:
    /// one a crash left between an earlier prune's commit and its
    /// deletions). Called after the prune commits, never inside its
    /// transaction, so a prune rolled back keeps every blob its rows
    /// name. Each blob is checked again and removed in a write
    /// transaction of its own making, which an upload of the same bytes
    /// (whose row and blob are written in one) cannot fall between. How
    /// many were removed.
    ///
    /// # Errors
    /// The store's, the content store's.
    pub(crate) fn release_blobs(&self, released: Vec<String>) -> Result<usize> {
        if !self.content().keeps_blobs() {
            return Ok(0);
        }
        let mut candidates = released;
        candidates.extend(self.block_on(self.content().blob_names())?);
        candidates.sort_unstable();
        candidates.dedup();
        self.with_tx(|tx| {
            let mut removed = 0;
            for sha in &candidates {
                let named: bool = tx
                    .prepare_cached("SELECT EXISTS (SELECT 1 FROM bot_content WHERE sha256 = ?1)")?
                    .query_row(params![sha], |r| r.get(0))?;
                if !named {
                    self.block_on(self.content().delete_blob(sha))?;
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_runs::{DriverId, NewRun, RequestKind, StepCommit, StepKind};
    use crate::credits::KeySource;
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, artifact, store_with_clock};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        match r {
            Err(CoreError::Invalid { code, .. }) => code,
            other => panic!("not refused: {other:?}"),
        }
    }

    fn run(st: &Store) -> String {
        let aid = artifact(st, None).to_string();
        let run = st
            .create_run(
                DAEMON,
                NewRun {
                    artifact_id: aid,
                    person: OWNER.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec!["t1".into()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                    web: String::new(),
                },
            )
            .unwrap();
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap().unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Script).unwrap();
        run.id
    }

    /// Ends `run`, whose step 1 is open, with a finish step.
    fn end(st: &Store, run: &str) {
        let d = DriverId::named("d1");
        st.commit_bot_step(DAEMON, run, &d, 1, StepCommit::default())
            .unwrap();
        st.begin_step(run, &d, 2, StepKind::Finish).unwrap();
        st.commit_bot_step(DAEMON, run, &d, 2, StepCommit::default())
            .unwrap();
    }

    fn new<'a>(id: &'a str, run: &'a str, bytes: &'a [u8]) -> NewContent<'a> {
        NewContent {
            id,
            run_id: run,
            surface: "desk",
            registration_id: Some("reg"),
            name: Some("desk.png"),
            media_type: "image/png",
            width: Some(4),
            height: Some(3),
            bytes,
        }
    }

    #[test]
    fn an_item_is_kept_by_its_hash_and_read_only_through_its_run() {
        let (_d, st) = crate::test_util::store();
        let (a, b) = (run(&st), run(&st));
        let row = st
            .put_bot_content(&new("c1", &a, b"pixels"), ContentLimits::default())
            .unwrap();
        assert_eq!(row.sha256, clax_core::audit::sha256_hex(b"pixels"));
        assert_eq!(row.bytes, 6);
        assert_eq!(
            row.handle(),
            json!({"id": "c1", "media_type": "image/png", "bytes": 6, "sha256": row.sha256,
                   "name": "desk.png", "width": 4, "height": 3})
        );
        assert_eq!(st.bot_content_of(&a, "c1").unwrap(), Some(row.clone()));
        assert_eq!(st.bot_content_of(&b, "c1").unwrap(), None);
        assert_eq!(st.bot_content_bytes(&row).unwrap().unwrap(), b"pixels");
        st.note_bot_content("c1", "desk.snapshot", "k:1:0", 1, Some(&json!({"at": 1})))
            .unwrap();
        st.note_bot_content("c1", "desk.other", "k:1:1", 1, None)
            .unwrap();
        let noted = st.bot_content("c1").unwrap().unwrap();
        assert_eq!(noted.op.as_deref(), Some("desk.snapshot"));
        assert_eq!(noted.meta, Some(json!({"at": 1})));
        assert!(!noted.shown);
        st.mark_bot_content_shown(&b, &["c1".into()]).unwrap();
        assert!(!st.bot_content("c1").unwrap().unwrap().shown);
        st.mark_bot_content_shown(&a, &["c1".into()]).unwrap();
        assert!(st.bot_content("c1").unwrap().unwrap().shown);
    }

    #[test]
    fn an_item_and_a_runs_items_are_held_to_their_limits() {
        let (_d, st) = crate::test_util::store();
        let a = run(&st);
        let limits = ContentLimits {
            item_bytes: 4,
            run_bytes: 6,
            total_bytes: 9,
        };
        assert_eq!(
            code(st.put_bot_content(&new("c1", &a, b"12345"), limits)),
            CODE_CONTENT_TOO_LARGE
        );
        st.put_bot_content(&new("c2", &a, b"1234"), limits).unwrap();
        assert_eq!(
            code(st.put_bot_content(&new("c3", &a, b"123"), limits)),
            CODE_RUN_CONTENT_FULL
        );
        st.put_bot_content(&new("c4", &a, b"12"), limits).unwrap();
        assert_eq!(st.bot_content("c1").unwrap(), None);
        assert_eq!(st.bot_content("c3").unwrap(), None);
        // All runs' items together are 9 bytes at the most, and 6 are
        // held: another run's item of 4 is refused, and the room says so
        // before any byte is read.
        let b = run(&st);
        let room = st.bot_content_room(&b, limits).unwrap();
        assert_eq!(
            room,
            ContentRoom {
                item: 4,
                run: 6,
                total: 3
            }
        );
        assert_eq!(room.most(), (3, CODE_CONTENT_STORE_FULL));
        assert_eq!(
            code(st.put_bot_content(&new("c5", &b, b"1234"), limits)),
            CODE_CONTENT_STORE_FULL
        );
        st.put_bot_content(&new("c6", &b, b"123"), limits).unwrap();
        assert_eq!(st.bot_content("c5").unwrap(), None);
    }

    /// A prune whose transaction rolls back removes no blob, so every row
    /// it leaves keeps its bytes; a blob no row names, as a crash between
    /// a prune's commit and its deletions leaves one, is removed by the
    /// next prune.
    #[test]
    fn a_prune_rolled_back_keeps_its_blobs_and_a_later_one_sweeps_what_no_row_names() {
        let clock = Arc::new(ManualClock::at("2026-11-11T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let gone = run(&st);
        let only = st
            .put_bot_content(&new("c1", &gone, b"only"), ContentLimits::default())
            .unwrap();
        end(&st, &gone);
        clock.advance(24 * 3600 + 1);
        // A statement after the content's fails, and the transaction
        // rolls back.
        st.with_write(|c| {
            c.execute_batch("ALTER TABLE bot_messages RENAME TO bot_messages_away")?;
            Ok(())
        })
        .unwrap();
        assert!(st.prune_ended_runs().is_err());
        assert!(st.bot_content("c1").unwrap().is_some());
        assert_eq!(st.bot_content_bytes(&only).unwrap().unwrap(), b"only");
        st.with_write(|c| {
            c.execute_batch("ALTER TABLE bot_messages_away RENAME TO bot_messages")?;
            Ok(())
        })
        .unwrap();
        // A blob that no row names, left by a crash.
        let orphan = clax_core::audit::sha256_hex(b"orphan");
        st.block_on(st.content().put_blob(&orphan, b"orphan"))
            .unwrap();
        let pruned = st.prune_ended_runs().unwrap();
        assert_eq!((pruned.contents, pruned.blobs), (1, 2));
        assert_eq!(st.bot_content_bytes(&only).unwrap(), None);
        assert_eq!(st.block_on(st.content().read_blob(&orphan)).unwrap(), None);
    }

    #[test]
    fn prune_removes_an_ended_runs_rows_and_the_blobs_no_row_names() {
        let clock = Arc::new(ManualClock::at("2026-11-11T10:00:00Z"));
        let (_d, st) = store_with_clock(clock.clone());
        let (gone, kept) = (run(&st), run(&st));
        let limits = ContentLimits::default();
        let only = st
            .put_bot_content(&new("c1", &gone, b"only"), limits)
            .unwrap();
        let shared = st
            .put_bot_content(&new("c2", &gone, b"shared"), limits)
            .unwrap();
        st.put_bot_content(&new("c3", &kept, b"shared"), limits)
            .unwrap();
        end(&st, &gone);
        clock.advance(24 * 3600);
        let pruned = st.prune_ended_runs().unwrap();
        assert_eq!(
            (pruned.contents, pruned.blobs),
            (0, 0),
            "a day, to the second"
        );
        clock.advance(1);
        let pruned = st.prune_ended_runs().unwrap();
        assert_eq!((pruned.contents, pruned.blobs), (2, 1));
        assert_eq!(st.bot_content("c1").unwrap(), None);
        assert_eq!(st.bot_content("c2").unwrap(), None);
        assert_eq!(st.bot_content_bytes(&only).unwrap(), None);
        // Another run's item of the same bytes keeps the blob.
        assert_eq!(st.bot_content_bytes(&shared).unwrap().unwrap(), b"shared");
        assert!(st.bot_content("c3").unwrap().is_some());
    }
}
