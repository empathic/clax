//! The audit journal's table (spec 2026-10-06-toolpath-audit-design §5.1):
//! recording an event inside a mutation's own transaction, reading events
//! back in `seq` order, and the install ID.
//!
//! Recording costs one `INSERT` in the caller's transaction. When that
//! transaction commits, [`Store::with_tx`] fires the audit nudge, which
//! tells the journal appender there is something new; the nudge must never
//! block.

use super::Store;
use super::sessions::with_for_actor;
use crate::CoreError;
use crate::Result;
use crate::audit::{AgentActor, AuditCtx, AuditIds, AuditKind, AuditRecord, SystemReason};
use crate::live::PageKey;
use crate::toolpath::project::{self, ArtifactInfo, Export, ExportEnv, Scope, Source};
use crate::working::{StopReason, Transition};
use rusqlite::{OptionalExtension, Row, params};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// What [`Store::set_audit_nudge`] installs.
type Nudge = Arc<dyn Fn() + Send + Sync>;

/// A store's audit recording state.
#[derive(Default)]
pub(super) struct AuditState {
    /// Set by [`Store::record_audit`]; read and cleared by the writer once
    /// the transaction commits. Only the writer touches it.
    recorded: AtomicBool,
    /// Whether the writer is running a [`Store::with_tx`] job, the only
    /// place an event may be recorded (so its nudge is never lost).
    in_tx: AtomicBool,
    nudge: Mutex<Option<Nudge>>,
    install_id: OnceLock<String>,
}

/// Marks the writer as running a [`Store::with_tx`] job until dropped.
pub(super) struct InTx<'a>(&'a AuditState);

impl Drop for InTx<'_> {
    fn drop(&mut self) {
        self.0.in_tx.store(false, Ordering::Relaxed);
    }
}

impl AuditState {
    /// Starts a [`Store::with_tx`] job, forgetting a record left by a
    /// transaction that did not commit. Held on the writer for the job.
    pub(super) fn begin(&self) -> InTx<'_> {
        self.recorded.store(false, Ordering::Relaxed);
        self.in_tx.store(true, Ordering::Relaxed);
        InTx(self)
    }

    /// Whether the transaction that just committed recorded an event.
    pub(super) fn take_recorded(&self) -> bool {
        self.recorded.swap(false, Ordering::Relaxed)
    }

    /// Fires the nudge, when one is set. The write has committed, so a
    /// panicking nudge is logged rather than failing the caller.
    pub(super) fn nudge(&self) {
        let nudge = self.nudge.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(n) = nudge
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| n())).is_err()
        {
            tracing::error!("the audit nudge panicked; the event is recorded");
        }
    }
}

/// One stored audit event. `kind`, `actor` and `body` are kept as stored,
/// so a reader can report a row it cannot interpret instead of failing.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRow {
    pub seq: i64,
    pub at: String,
    /// The kind's dotted name (spec §6).
    pub kind: String,
    /// The actor, as JSON (spec §5.2).
    pub actor: String,
    pub ids: AuditIds,
    /// The record body, as JSON: the envelope plus the kind's fields.
    pub body: String,
    pub backfilled: bool,
}

pub(super) const EVENTS_AFTER: &str =
    "SELECT seq, at, kind, actor, artifact_id, artifact2_id, thread_id, session_id,
        question_id, call_id, origin, body, backfilled
     FROM audit_events WHERE seq > ?1 ORDER BY seq LIMIT ?2";

pub(super) const EVENTS_FOR_CALL: &str =
    "SELECT seq, at, kind, actor, artifact_id, artifact2_id, thread_id, session_id,
        question_id, call_id, origin, body, backfilled
     FROM audit_events WHERE call_id = ?1 ORDER BY seq";

pub(super) const NEWEST_SEQ: &str = "SELECT MAX(seq) FROM audit_events";

fn row_to_event(r: &Row<'_>) -> rusqlite::Result<AuditRow> {
    Ok(AuditRow {
        seq: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        actor: r.get(3)?,
        ids: AuditIds {
            artifact: r.get(4)?,
            artifact2: r.get(5)?,
            thread: r.get(6)?,
            session: r.get(7)?,
            question: r.get(8)?,
            call: r.get(9)?,
            origin: r.get(10)?,
        },
        body: r.get(11)?,
        backfilled: r.get(12)?,
    })
}

/// A `working.start` event, without IDs (spec §6.5).
fn working_start(key: &str, message: &Option<String>, thread_ids: &[String]) -> AuditRecord {
    AuditRecord::new(AuditKind::WorkingStart, Store::now())
        .with("key", key)
        .with("message", message.clone())
        .with("thread_ids", thread_ids.to_vec())
}

/// A `working.stop` event, without IDs (spec §6.5).
fn working_stop(key: &str, reason: StopReason, duration_ms: i64) -> AuditRecord {
    AuditRecord::new(AuditKind::WorkingStop, Store::now())
        .with("key", key)
        .with("reason", reason.as_str())
        .with("duration_ms", duration_ms)
}

/// The stored body of `rec` under `ctx`: the record's fields plus the
/// envelope (spec §6): `v`, `via`, the recording build's `clax_version`
/// and `clax_commit`, and, when present, `git`, `git_capture` and `call`.
/// The envelope wins over a record field of the same name.
fn body_json(ctx: &AuditCtx, rec: AuditRecord) -> String {
    let mut body = rec.body;
    body.insert("v".into(), 1.into());
    body.insert("via".into(), ctx.via.as_str().into());
    body.insert("clax_version".into(), env!("CARGO_PKG_VERSION").into());
    body.insert("clax_commit".into(), crate::build_commit().into());
    if let Some(git) = ctx.git.context() {
        body.insert(
            "git".into(),
            serde_json::to_value(git).expect("serialisable git context"),
        );
    }
    if let Some(outcome) = ctx.git.capture() {
        body.insert("git_capture".into(), outcome.into());
    }
    if let Some(call) = &ctx.call {
        body.insert(
            "call".into(),
            serde_json::to_value(call).expect("serialisable call header"),
        );
    }
    serde_json::to_string(&body).expect("serialisable body")
}

/// Inserts one event row: the single INSERT behind [`Store::record_audit`]
/// and the backfill.
fn insert_row(
    tx: &rusqlite::Transaction<'_>,
    at: &str,
    kind: &str,
    actor: &str,
    ids: &AuditIds,
    body: &str,
    backfilled: bool,
) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO audit_events (at, kind, actor, artifact_id, artifact2_id, thread_id,
            session_id, question_id, call_id, origin, body, backfilled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?
    .execute(params![
        at,
        kind,
        actor,
        ids.artifact,
        ids.artifact2,
        ids.thread,
        ids.session,
        ids.question,
        ids.call,
        ids.origin,
        body,
        backfilled
    ])?;
    Ok(())
}

/// Inserts `rec` as a backfilled event (spec L12): `backfilled = 1`, made
/// by Clax itself (`system:backfill`) through the `daemon` channel, with no
/// git context and no tool call. For the backfill alone, which runs before
/// the store serves and so has no nudge to fire.
pub(super) fn insert_backfilled(tx: &rusqlite::Transaction<'_>, rec: AuditRecord) -> Result<()> {
    let ctx = AuditCtx::system(SystemReason::Backfill);
    let actor = serde_json::to_string(&ctx.actor).expect("serialisable actor");
    let kind = rec.kind.as_str();
    let at = rec.at.clone();
    let ids = AuditIds {
        call: None,
        ..rec.ids.clone()
    };
    let body = body_json(&ctx, rec);
    insert_row(tx, &at, kind, &actor, &ids, &body, true)
}

impl Store {
    /// Records `rec`, made under `ctx`, in `tx`: the event commits or rolls
    /// back with the change it describes. Returns its `seq`. The event's
    /// `call_id` is `rec`'s, or else the call `ctx` was made under; its
    /// `session_id` is `rec`'s, or else the Clax session of `ctx`'s agent.
    ///
    /// `tx` must be the transaction of a [`Store::with_tx`] job, which fires
    /// the audit nudge once it commits. Under any other transaction the
    /// nudge would be lost, so recording fails with `audit_outside_tx`.
    pub fn record_audit(
        &self,
        tx: &rusqlite::Transaction<'_>,
        ctx: &AuditCtx,
        rec: AuditRecord,
    ) -> Result<i64> {
        if !self.audit.in_tx.load(Ordering::Relaxed) {
            return Err(crate::CoreError::invalid(
                "audit_outside_tx",
                "audit events are recorded only in Store::with_tx",
            ));
        }
        let actor = serde_json::to_string(&ctx.actor).expect("serialisable actor");
        let kind = rec.kind.as_str();
        let at = rec.at.clone();
        let ids = rec.ids.clone();
        let call_id = ids
            .call
            .or_else(|| ctx.call.as_ref().map(|c| c.call_id.clone()));
        let session_id = ids.session.or_else(|| match &ctx.actor {
            crate::audit::Actor::Agent(a) => a.session_id.clone(),
            _ => None,
        });
        let body = body_json(ctx, rec);
        let ids = AuditIds {
            session: session_id,
            call: call_id,
            ..ids
        };
        insert_row(tx, &at, kind, &actor, &ids, &body, false)?;
        self.audit.recorded.store(true, Ordering::Relaxed);
        Ok(tx.last_insert_rowid())
    }

    /// Installs `nudge`, called after each commit that recorded an audit
    /// event, on the committing thread once the writer is released. It must
    /// return at once (a `try_send`, say). Replaces an earlier nudge.
    pub fn set_audit_nudge(&self, nudge: impl Fn() + Send + Sync + 'static) {
        *self.audit.nudge.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(nudge));
    }

    /// This install's opaque ID: 128 random bits in lowercase hex, minted
    /// once by the audit migration.
    pub fn install_id(&self) -> Result<String> {
        if let Some(id) = self.audit.install_id.get() {
            return Ok(id.clone());
        }
        let id: String = self.with_read(|c| {
            Ok(c.query_row("SELECT v FROM install WHERE k = 'id'", [], |r| r.get(0))?)
        })?;
        Ok(self.audit.install_id.get_or_init(|| id).clone())
    }

    /// Up to `limit` events with a `seq` above `seq`, in `seq` (commit)
    /// order.
    pub fn events_after(&self, seq: i64, limit: u32) -> Result<Vec<AuditRow>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(EVENTS_AFTER)?;
            let rows = q
                .query_map(params![seq, limit], row_to_event)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Every event recorded under tool call `call_id`, in `seq` order.
    pub fn events_for_call(&self, call_id: &str) -> Result<Vec<AuditRow>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(EVENTS_FOR_CALL)?;
            let rows = q
                .query_map(params![call_id], row_to_event)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Records the starts and ends of working records in `changes` (spec
    /// §6.5), in one transaction: `working.start {key, message,
    /// thread_ids}` and `working.stop {key, reason, duration_ms}`, each on
    /// its artifact and session. Working state lives in memory, so these
    /// are recorded once the registry has changed; nothing is written when
    /// there is nothing to record. An end for [`StopReason::Ttl`] is
    /// recorded as `system:ttl`, any other event under `ctx`; a system actor
    /// names the session's agent in `for_actor`.
    ///
    /// [`StopReason::Ttl`]: crate::working::StopReason::Ttl
    pub fn record_working(&self, ctx: &AuditCtx, changes: &[Transition]) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let ttl = AuditCtx::system(SystemReason::Ttl);
        self.with_tx(|tx| {
            for t in changes {
                let (ctx, rec, sid, aid, fallback) = match t {
                    Transition::Started {
                        session_id,
                        artifact_id,
                        key,
                        message,
                        thread_ids,
                    } => (
                        ctx,
                        working_start(key, message, thread_ids),
                        session_id,
                        artifact_id,
                        AgentActor::default(),
                    ),
                    Transition::Stopped {
                        session_id,
                        artifact_id,
                        key,
                        harness,
                        agent,
                        reason,
                        duration_ms,
                    } => (
                        if *reason == StopReason::Ttl {
                            &ttl
                        } else {
                            ctx
                        },
                        working_stop(key, *reason, *duration_ms),
                        session_id,
                        artifact_id,
                        AgentActor {
                            harness: Some(harness.clone()),
                            agent_handle: Some(agent.clone()),
                            ..AgentActor::default()
                        },
                    ),
                };
                let mut rec = with_for_actor(tx, ctx, rec, sid, || AgentActor {
                    session_id: Some(sid.clone()),
                    ..fallback
                })?;
                rec.ids.artifact = Some(aid.clone());
                rec.ids.session = Some(sid.clone());
                self.record_audit(tx, ctx, rec)?;
            }
            Ok(())
        })
    }

    /// The highest `seq` recorded, or 0 when there is none.
    pub fn newest_seq(&self) -> Result<i64> {
        self.with_read(|c| {
            Ok(c.query_row(NEWEST_SEQ, [], |r| r.get::<_, Option<i64>>(0))?
                .unwrap_or(0))
        })
    }

    /// Runs `f` over the recorded history as one [`Source`], in one read
    /// transaction, so every row and artifact `f` reads comes from one
    /// snapshot. The read has no time limit (an export streams for as long
    /// as its reader takes); it stops with [`CoreError::TaskFailed`] once
    /// the store shuts down.
    pub fn select<T>(&self, f: impl FnOnce(&dyn Source) -> Result<T>) -> Result<T> {
        self.readers.run(false, |c| {
            f(&StoreSource {
                c,
                shut_down: &self.shut_down,
            })
        })
    }

    /// Writes the export `req` to `w` (spec §8): [`project::export`] over
    /// [`Store::select`].
    pub fn export(&self, req: &Export, env: &ExportEnv, w: &mut dyn std::io::Write) -> Result<()> {
        self.select(|src| project::export(src, req, env, w))
    }
}

const EVENT_COLUMNS: &str =
    "seq, at, kind, actor, artifact_id, artifact2_id, thread_id, session_id,
        question_id, call_id, origin, body, backfilled";

/// The store's history as a [`Source`], read on one connection inside one
/// read transaction.
struct StoreSource<'c> {
    c: &'c rusqlite::Connection,
    shut_down: &'c std::sync::atomic::AtomicBool,
}

impl StoreSource<'_> {
    fn each(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        f: &mut dyn FnMut(AuditRow) -> Result<bool>,
    ) -> Result<()> {
        let mut q = self.c.prepare_cached(sql)?;
        let mut rows = q.query(params)?;
        let mut n = 0u32;
        while let Some(r) = rows.next()? {
            n = n.wrapping_add(1);
            if n.is_multiple_of(1024) && self.shut_down.load(Ordering::SeqCst) {
                return Err(CoreError::TaskFailed);
            }
            if !f(row_to_event(r)?)? {
                break;
            }
        }
        Ok(())
    }
}

impl StoreSource<'_> {
    /// The rows on artifact `a`, in `seq` order: a merge of the rows whose
    /// `artifact_id` is `a` and those whose `artifact2_id` is, each read
    /// in `seq` order from its index, so no row is held to be sorted.
    fn artifact_rows(&self, a: &str, f: &mut dyn FnMut(AuditRow) -> Result<bool>) -> Result<()> {
        let mut first = self.c.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM audit_events WHERE artifact_id = ?1 ORDER BY seq"
        ))?;
        let mut second = self.c.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM audit_events
             WHERE artifact2_id = ?1 AND artifact_id IS NOT ?1 ORDER BY seq"
        ))?;
        let mut xs = first.query(params![a])?;
        let mut ys = second.query(params![a])?;
        let mut x = xs.next()?.map(row_to_event).transpose()?;
        let mut y = ys.next()?.map(row_to_event).transpose()?;
        let mut n = 0u32;
        loop {
            let take_x = match (&x, &y) {
                (None, None) => return Ok(()),
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(p), Some(q)) => p.seq < q.seq,
            };
            let row = if take_x {
                std::mem::replace(&mut x, xs.next()?.map(row_to_event).transpose()?)
            } else {
                std::mem::replace(&mut y, ys.next()?.map(row_to_event).transpose()?)
            };
            n = n.wrapping_add(1);
            if n.is_multiple_of(1024) && self.shut_down.load(Ordering::SeqCst) {
                return Err(CoreError::TaskFailed);
            }
            if !f(row.expect("one side holds a row"))? {
                return Ok(());
            }
        }
    }
}

impl Source for StoreSource<'_> {
    fn rows(&self, scope: Scope<'_>, f: &mut dyn FnMut(AuditRow) -> Result<bool>) -> Result<()> {
        match scope {
            Scope::Artifact(a) => self.artifact_rows(a, f),
            Scope::Install => self.each(
                &format!(
                    "SELECT {EVENT_COLUMNS} FROM audit_events
                     WHERE artifact_id IS NULL AND artifact2_id IS NULL ORDER BY seq"
                ),
                &[],
                f,
            ),
            Scope::All => self.each(
                &format!("SELECT {EVENT_COLUMNS} FROM audit_events ORDER BY seq"),
                &[],
                f,
            ),
            Scope::Mentioning(values) => {
                let mut sql = format!("SELECT {EVENT_COLUMNS} FROM audit_events WHERE 0");
                let mut params: Vec<&dyn rusqlite::ToSql> = Vec::new();
                for (i, v) in values.iter().enumerate() {
                    let n = i + 1;
                    sql.push_str(&format!(
                        " OR session_id = ?{n} OR instr(actor, ?{n}) > 0 OR instr(body, ?{n}) > 0"
                    ));
                    params.push(v);
                }
                sql.push_str(" ORDER BY seq");
                self.each(&sql, &params, f)
            }
        }
    }

    fn row(&self, seq: i64) -> Result<Option<AuditRow>> {
        Ok(self
            .c
            .prepare_cached(&format!(
                "SELECT {EVENT_COLUMNS} FROM audit_events WHERE seq = ?1"
            ))?
            .query_row(params![seq], row_to_event)
            .optional()?)
    }

    fn artifact_ids(&self) -> Result<Vec<String>> {
        let mut q = self.c.prepare_cached(
            "SELECT artifact_id FROM audit_events WHERE artifact_id IS NOT NULL
             UNION SELECT artifact2_id FROM audit_events WHERE artifact2_id IS NOT NULL
             ORDER BY 1",
        )?;
        let ids = q
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    fn artifact_known(&self, id: &str) -> Result<bool> {
        Ok(self.c.query_row(
            "SELECT EXISTS(SELECT 1 FROM artifacts WHERE id = ?1)
                 OR EXISTS(SELECT 1 FROM audit_events WHERE artifact_id = ?1)
                 OR EXISTS(SELECT 1 FROM audit_events WHERE artifact2_id = ?1)",
            params![id],
            |r| r.get(0),
        )?)
    }

    fn live_artifacts(&self, key: &PageKey) -> Result<Vec<String>> {
        let mut q = self.c.prepare_cached(
            "SELECT artifact_id FROM live_pages
              WHERE path = ?2
                AND origin IN (?1, COALESCE((SELECT site FROM live_sites WHERE origin = ?1), ?1))
             UNION
             SELECT artifact_id FROM live_merged_pages
              WHERE path = ?2
                AND origin IN (?1, COALESCE((SELECT site FROM live_sites WHERE origin = ?1), ?1))
             UNION
             SELECT artifact_id FROM audit_events
              WHERE kind = 'live.page' AND artifact_id IS NOT NULL
                AND origin IN (?1, COALESCE((SELECT site FROM live_sites WHERE origin = ?1), ?1))
                AND json_extract(body, '$.path') = ?2
             ORDER BY 1",
        )?;
        let ids = q
            .query_map(params![key.origin, key.path], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    fn artifact(&self, id: &str) -> Result<ArtifactInfo> {
        let info = self
            .c
            .prepare_cached(
                "SELECT a.title, a.kind, COALESCE(p.origin, m.origin), COALESCE(p.path, m.path)
                   FROM artifacts a
                   LEFT JOIN live_pages p ON p.artifact_id = a.id
                   LEFT JOIN live_merged_pages m ON m.artifact_id = a.id
                  WHERE a.id = ?1",
            )?
            .query_row(params![id], |r| {
                let origin: Option<String> = r.get(2)?;
                let path: Option<String> = r.get(3)?;
                Ok(ArtifactInfo {
                    title: r.get(0)?,
                    kind: r.get(1)?,
                    live: origin
                        .zip(path)
                        .map(|(origin, path)| PageKey { origin, path }),
                })
            })
            .optional()?;
        Ok(info.unwrap_or_default())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::CoreError;
    use crate::audit::{Actor, AuditKind, CallHeader, SystemReason, Via};
    use crate::gitctx::GitField;
    use crate::store::test_util::store;
    use crate::{Home, Store};
    use std::sync::mpsc;

    fn ctx() -> AuditCtx {
        AuditCtx {
            actor: Actor::Owner {
                public_id: "u_4be1".into(),
            },
            via: Via::Shell,
            git: GitField::Capture("not-a-repo"),
            call: None,
        }
    }

    fn rec(title: &str) -> AuditRecord {
        let mut r = AuditRecord::new(AuditKind::ArtifactUpdate, Store::now()).with("title", title);
        r.ids.artifact = Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into());
        r
    }

    #[test]
    fn working_events_under_a_system_actor_name_the_agent() {
        use crate::store::test_util::{DAEMON, session};
        use crate::working::{StopReason, Transition};
        let (_d, st) = store();
        let sid = session(&st, "codex", "cx-1");
        let start = Transition::Started {
            session_id: sid.clone(),
            artifact_id: "a1".into(),
            key: "k1".into(),
            message: None,
            thread_ids: vec!["t1".into()],
        };
        let stop = |reason| Transition::Stopped {
            session_id: sid.clone(),
            artifact_id: "a1".into(),
            key: "k1".into(),
            harness: "codex".into(),
            agent: "a_x".into(),
            reason,
            duration_ms: 5,
        };
        let seq = st.newest_seq().unwrap();
        st.record_working(DAEMON, &[start.clone(), stop(StopReason::Ttl)])
            .unwrap();
        st.record_working(&ctx(), &[start, stop(StopReason::Explicit)])
            .unwrap();
        let ev: Vec<(String, serde_json::Value, serde_json::Value)> = st
            .events_after(seq, 10)
            .unwrap()
            .into_iter()
            .map(|e| {
                assert_eq!(e.ids.session.as_deref(), Some(sid.as_str()));
                assert_eq!(e.ids.artifact.as_deref(), Some("a1"));
                (
                    e.kind,
                    serde_json::from_str(&e.actor).unwrap(),
                    serde_json::from_str(&e.body).unwrap(),
                )
            })
            .collect();
        let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "working.start",
                "working.stop",
                "working.start",
                "working.stop"
            ]
        );
        assert_eq!(ev[0].1["reason"], "daemon");
        assert_eq!(ev[1].1["reason"], "ttl");
        for e in &ev[..2] {
            assert_eq!(e.2["for_actor"]["session_id"], sid.as_str());
            assert_eq!(e.2["for_actor"]["harness_session_id"], "cx-1");
        }
        for e in &ev[2..] {
            assert_eq!(e.1["type"], "owner");
            assert!(e.2.get("for_actor").is_none());
        }
        // Nothing to record: no transaction, no event.
        st.record_working(DAEMON, &[]).unwrap();
        assert_eq!(st.events_after(seq, 10).unwrap().len(), 4);
    }

    #[test]
    fn install_id_is_stable_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("ax"));
        let first = Store::open(&home).unwrap().install_id().unwrap();
        assert!(is_install_id(&first), "{first}");
        let st = Store::open(&home).unwrap();
        assert_eq!(st.install_id().unwrap(), first);
        assert_eq!(st.install_id().unwrap(), first);
        let other = tempfile::tempdir().unwrap();
        let elsewhere = Store::open(&Home::at(other.path().join("ax")))
            .unwrap()
            .install_id()
            .unwrap();
        assert_ne!(elsewhere, first);
    }

    /// 128 random bits in lowercase hex: no timestamp, unlike a ULID.
    pub(crate) fn is_install_id(id: &str) -> bool {
        id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    #[test]
    fn concurrent_commits_get_increasing_seqs_and_one_nudge_each() {
        let (_d, st) = store();
        let st = Arc::new(st);
        let nudges = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = nudges.clone();
        st.set_audit_nudge(move || {
            n.fetch_add(1, Ordering::Relaxed);
        });
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let st = st.clone();
                std::thread::spawn(move || {
                    (0..10)
                        .map(|j| {
                            st.with_tx(|tx| st.record_audit(tx, &ctx(), rec(&format!("{i}.{j}"))))
                                .unwrap()
                        })
                        .collect::<Vec<i64>>()
                })
            })
            .collect();
        let mut all = Vec::new();
        for t in threads {
            let mine = t.join().unwrap();
            assert!(mine.windows(2).all(|w| w[0] < w[1]), "{mine:?}");
            all.extend(mine);
        }
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 80);
        assert_eq!(nudges.load(Ordering::Relaxed), 80);
        let read: Vec<i64> = st
            .events_after(0, 100)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(read, all);
    }

    #[test]
    fn record_audit_outside_with_tx_is_refused() {
        let (_d, st) = store();
        let r = st.with_write(|c| {
            let tx = c.unchecked_transaction()?;
            st.record_audit(&tx, &ctx(), rec("lost nudge"))
        });
        assert!(
            matches!(
                r,
                Err(CoreError::Invalid {
                    code: "audit_outside_tx",
                    ..
                })
            ),
            "{r:?}"
        );
        // A with_tx job that failed leaves no permission behind.
        let _ = st.with_tx(|_| Err::<(), _>(CoreError::NotFound));
        let r = st.with_write(|c| {
            let tx = c.unchecked_transaction()?;
            st.record_audit(&tx, &ctx(), rec("lost nudge"))
        });
        assert!(r.is_err());
        assert_eq!(st.newest_seq().unwrap(), 0);
    }

    #[test]
    fn a_panicking_nudge_does_not_fail_the_committed_write() {
        let (_d, st) = store();
        st.set_audit_nudge(|| panic!("nudge"));
        let seq = st
            .with_tx(|tx| st.record_audit(tx, &ctx(), rec("kept")))
            .unwrap();
        assert_eq!(st.newest_seq().unwrap(), seq);
    }

    #[test]
    fn record_audit_rolls_back_with_tx() {
        let (_d, st) = store();
        let (send, recv) = mpsc::channel();
        st.set_audit_nudge(move || send.send(()).unwrap());
        let r: Result<()> = st.with_tx(|tx| {
            st.record_audit(tx, &ctx(), rec("gone"))?;
            Err(CoreError::NotFound)
        });
        assert!(matches!(r, Err(CoreError::NotFound)));
        assert_eq!(st.newest_seq().unwrap(), 0);
        assert!(st.events_after(0, 10).unwrap().is_empty());
        assert!(recv.try_recv().is_err(), "a rolled-back event nudges");
        // A later commit that records nothing does not nudge either.
        st.with_tx(|_| Ok(())).unwrap();
        assert!(recv.try_recv().is_err());
    }

    #[test]
    fn seq_is_commit_ordered() {
        let (_d, st) = store();
        let st = Arc::new(st);
        let path = st.home().db_path();
        let (send, recv) = mpsc::channel();
        // The nudge sees the event committed: another connection reads it.
        st.set_audit_nudge(move || {
            let c = rusqlite::Connection::open(&path).unwrap();
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM audit_events", [], |r| r.get(0))
                .unwrap();
            send.send(n).unwrap();
        });
        let mut seqs = Vec::new();
        for i in 0..3 {
            let seq = st
                .with_tx(|tx| st.record_audit(tx, &ctx(), rec(&format!("t{i}"))))
                .unwrap();
            seqs.push(seq);
            assert_eq!(recv.try_recv().unwrap(), i + 1);
        }
        // A rolled-back insert's seq is never seen. SQLite rolls its
        // sequence back too, so the next commit takes that number; consumers
        // must not read meaning into gaps or their absence either way.
        let gone = st
            .with_tx(|tx| {
                let seq = st.record_audit(tx, &ctx(), rec("gone"))?;
                Err::<(), _>(CoreError::Invalid {
                    code: "x",
                    message: seq.to_string(),
                })
            })
            .unwrap_err();
        assert!(
            matches!(gone, CoreError::Invalid { message, .. } if message == (seqs[2] + 1).to_string())
        );
        let two = st
            .with_tx(|tx| {
                let a = st.record_audit(tx, &ctx(), rec("a"))?;
                let b = st.record_audit(tx, &ctx(), rec("b"))?;
                Ok([a, b])
            })
            .unwrap();
        assert_eq!(recv.try_recv().unwrap(), 5, "one nudge per commit");
        assert!(recv.try_recv().is_err());
        seqs.extend(two);
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
        assert_eq!(st.newest_seq().unwrap(), *seqs.last().unwrap());

        let all = st.events_after(0, 100).unwrap();
        assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), seqs);
        let titles: Vec<String> = all
            .iter()
            .map(|e| {
                serde_json::from_str::<serde_json::Value>(&e.body).unwrap()["title"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(titles, ["t0", "t1", "t2", "a", "b"]);
        let page = st.events_after(seqs[1], 2).unwrap();
        assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), &seqs[2..4]);
        assert!(st.events_after(seqs[4], 10).unwrap().is_empty());
    }

    #[test]
    fn record_stores_the_envelope_actor_and_ids() {
        let (_d, st) = store();
        let call = CallHeader {
            call_id: "01JB8Q2WXYZ0000000000000AA".into(),
            tool: "publish".into(),
            harness_tool: Some("mcp__clax__publish".into()),
            args_sha256: format!("sha256:{}", "c6".repeat(32)),
            started_at: "2026-10-06T10:00:00.000Z".into(),
            harness_call_id: None,
        };
        let under = AuditCtx {
            call: Some(call.clone()),
            ..ctx()
        };
        st.with_tx(|tx| {
            st.record_audit(tx, &under, rec("x").with("v", 7))?;
            st.record_audit(tx, &AuditCtx::system(SystemReason::Ttl), rec("y"))
        })
        .unwrap();
        let rows = st.events_after(0, 10).unwrap();
        let e = &rows[0];
        assert_eq!(e.kind, "artifact.update");
        assert!(!e.backfilled);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&e.actor).unwrap(),
            serde_json::json!({"type": "owner", "public_id": "u_4be1"})
        );
        assert_eq!(
            e.ids.artifact.as_deref(),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")
        );
        assert_eq!(e.ids.call.as_deref(), Some(call.call_id.as_str()));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&e.body).unwrap(),
            serde_json::json!({
                "v": 1,
                "via": "shell",
                "clax_version": env!("CARGO_PKG_VERSION"),
                "clax_commit": crate::build_commit(),
                "git_capture": "not-a-repo",
                "title": "x",
                "call": serde_json::to_value(&call).unwrap(),
            })
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rows[1].body).unwrap(),
            serde_json::json!({"v": 1, "via": "daemon", "title": "y",
                "clax_version": env!("CARGO_PKG_VERSION"), "clax_commit": crate::build_commit()})
        );
        assert_eq!(rows[1].ids.call, None);
        let under_call = st.events_for_call(&call.call_id).unwrap();
        assert_eq!(under_call, vec![e.clone()]);
        assert!(
            st.events_for_call("01JB8Q2WXYZ0000000000000AB")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_agent_actors_session_fills_the_session_column() {
        let (_d, st) = store();
        let agent = AuditCtx {
            actor: Actor::Agent(crate::audit::AgentActor {
                session_id: Some("01JB8Q2WXYZ0000000000000SS".into()),
                ..Default::default()
            }),
            via: Via::Mcp,
            git: GitField::Absent,
            call: None,
        };
        let sessionless = AuditCtx {
            actor: Actor::Agent(crate::audit::AgentActor::default()),
            ..agent.clone()
        };
        let mut named = rec("n");
        named.ids.session = Some("01JB8Q2WXYZ0000000000000TT".into());
        st.with_tx(|tx| {
            st.record_audit(tx, &agent, rec("a"))?;
            st.record_audit(tx, &sessionless, rec("s"))?;
            st.record_audit(tx, &ctx(), rec("o"))?;
            st.record_audit(tx, &agent, named)
        })
        .unwrap();
        let sessions: Vec<Option<String>> = st
            .events_after(0, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.ids.session)
            .collect();
        assert_eq!(
            sessions,
            vec![
                Some("01JB8Q2WXYZ0000000000000SS".into()),
                None,
                None,
                Some("01JB8Q2WXYZ0000000000000TT".into()),
            ]
        );
    }
}
