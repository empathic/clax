//! Approvals (spec 2026-10-08-multi-agent-multi-person §9): "agents of
//! `approver` may see `author`'s content at `scope`"; the requests that
//! ask an approver for one (made by routing a thread to their agent, M18,
//! or by an agent's `ask`), batched per (approver, author, artifact,
//! thread) (M19), decided or withdrawn; and the visibility generation
//! (M17), which every change to what an agent may see bumps.

use super::Store;
use super::threads::{AUTHOR_VIEWER, thread_in};
use crate::visibility::Visibility;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::feedback::Touched;
use clax_core::model::Thread;
use clax_core::visibility::placeholder::ApprovalState;
use clax_core::visibility::{self, Author, Reader};
use clax_core::{CoreError, Result, is_public_id, new_ulid};
use rusqlite::{OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use clax_core::visibility::{OWNER, UNCONNECTED};

/// What an approval covers (§9.1, M13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// One thread, by ID, including the author's later replies in it (R1).
    Thread(String),
    /// One artifact, by ID.
    Artifact(String),
    /// Every artifact.
    Everywhere,
}

/// The kind of an approval's scope, as stored and served.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKind {
    Thread,
    Artifact,
    Everywhere,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Thread => "thread",
            ScopeKind::Artifact => "artifact",
            ScopeKind::Everywhere => "everywhere",
        }
    }

    fn parse(s: &str) -> Option<ScopeKind> {
        match s {
            "thread" => Some(ScopeKind::Thread),
            "artifact" => Some(ScopeKind::Artifact),
            "everywhere" => Some(ScopeKind::Everywhere),
            _ => None,
        }
    }
}

/// Where an approval was made (`approvals.created_via`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    /// The shell (inbox or People).
    Shell,
    /// The Clax extension.
    Extension,
    /// `clax connect`'s question about the owner (M15).
    Connect,
    /// Migration 25's `*unconnected` approval.
    Migration,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Shell => "shell",
            Via::Extension => "extension",
            Via::Connect => "connect",
            Via::Migration => "migration",
        }
    }
}

/// An approval row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Approval {
    pub id: String,
    /// The person whose agents see; `None`: the owner.
    pub approver: Option<String>,
    /// A public ID, or [`UNCONNECTED`].
    pub author: String,
    pub scope: ScopeKind,
    pub artifact_id: Option<String>,
    pub thread_id: Option<String>,
    pub created_at: String,
    pub created_via: String,
    pub request_id: Option<String>,
    pub revoked_at: Option<String>,
}

const COLUMNS: &str = "id, approver, author, scope, artifact_id, thread_id, created_at,
    created_via, request_id, revoked_at";

fn row_to_approval(r: &Row<'_>) -> rusqlite::Result<Approval> {
    let scope: String = r.get(3)?;
    Ok(Approval {
        id: r.get(0)?,
        approver: r.get(1)?,
        author: r.get(2)?,
        scope: ScopeKind::parse(&scope).unwrap_or(ScopeKind::Thread),
        artifact_id: r.get(4)?,
        thread_id: r.get(5)?,
        created_at: r.get(6)?,
        created_via: r.get(7)?,
        request_id: r.get(8)?,
        revoked_at: r.get(9)?,
    })
}

/// `approval.grant` or `approval.revoke` for `a` (§20.2); its artifact
/// and thread go in the record's IDs, and where it was made is
/// `created_via`, since `via` is the event envelope's channel.
fn approval_record(kind: AuditKind, at: &str, a: &Approval) -> AuditRecord {
    let mut rec = AuditRecord::new(kind, at)
        .with("approval_id", a.id.as_str())
        .with("approver", a.approver.as_deref())
        .with("author", a.author.as_str())
        .with("scope", a.scope.as_str())
        .with("created_via", a.created_via.as_str());
    rec.ids.artifact = a.artifact_id.clone();
    rec.ids.thread = a.thread_id.clone();
    rec
}

/// Whether `?4`'s authors (a JSON array of keys) have an open or a
/// declined request of approver `?1` (`NULL`: the owner) at artifact `?2`,
/// for thread `?3` or the whole artifact: through the index
/// `approval_requests_by_target`, so the requests a busy pair piles up
/// (withdrawn and approved ones) are never walked. A decline is in force
/// until an approval of its approver and author covering thread `?3` is
/// made or revoked after it, the rule a thread's delivery state reads
/// declines by (`crate::feedback::DECLINES`).
pub(crate) const REQUEST_STATE: &str = "SELECT COALESCE(MAX(status = 'open'), 0),
            COALESCE(MAX(status = 'declined' AND NOT EXISTS (SELECT 1 FROM approvals a
               WHERE COALESCE(a.approver, '') = COALESCE(?1, '')
                 AND a.author = approval_requests.author
                 AND (a.scope = 'everywhere' OR (a.scope = 'artifact' AND a.artifact_id = ?2)
                      OR (a.scope = 'thread' AND a.thread_id = ?3))
                 AND (a.created_at > approval_requests.decided_at
                      OR a.revoked_at > approval_requests.decided_at))), 0)
       FROM approval_requests
      WHERE COALESCE(approver, '') = COALESCE(?1, '') AND artifact_id = ?2
        AND author IN (SELECT value FROM json_each(?4))
        AND status IN ('open', 'declined')
        AND (thread_id = ?3 OR thread_id IS NULL)";

/// [`Store::request_state`] on connection `c`.
pub(crate) fn request_state_in(
    c: &rusqlite::Connection,
    approver: Option<&str>,
    artifact: &str,
    thread: &str,
    authors: &[String],
) -> Result<ApprovalState> {
    let authors = serde_json::to_string(authors).expect("strings serialise");
    let (open, declined): (bool, bool) = c
        .prepare_cached(REQUEST_STATE)?
        .query_row(params![approver, artifact, thread, authors], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok(if open {
        ApprovalState::Requested
    } else if declined {
        ApprovalState::Declined
    } else {
        ApprovalState::None
    })
}

/// After approval `a` was made in `tx`: clears `placeholder_at` on each
/// undelivered row of its approver's sessions in its scope whose comment
/// body the approver's agents may now see, every author of it weighed
/// (spec §8.4, M16), so the row reads `sent` until a take delivers it; a
/// row another author still withholds keeps it, and is not told again.
/// Each such row's live session and thread go in `touched`, so their
/// waits, pushes and states learn of it.
fn release_told(tx: &Transaction<'_>, a: &Approval, touched: &mut Touched) -> Result<()> {
    let reader = match &a.approver {
        Some(p) => Reader::Person(p.clone()),
        None => Reader::Owner,
    };
    let vis = Visibility::for_reader_in(tx, &reader, 0)?;
    let rows: Vec<(String, String, String, String, String, bool, bool)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT f.id, f.comment_id, f.thread_id, t.artifact_id, f.target_session_id,
                    f.placeholder_at IS NOT NULL, s.ended_at IS NULL
             FROM feedback f
             JOIN sessions s ON s.id = f.target_session_id
             JOIN threads t ON t.id = f.thread_id
             WHERE f.delivered_at IS NULL AND COALESCE(s.person, '') = COALESCE(?1, '')
               AND (?2 = 'everywhere'
                    OR (?2 = 'artifact' AND t.artifact_id = ?3)
                    OR (?2 = 'thread' AND f.thread_id = ?4))",
        )?;
        stmt.query_map(
            params![a.approver, a.scope.as_str(), a.artifact_id, a.thread_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut threads: HashMap<String, Option<Thread>> = HashMap::new();
    for (fid, cid, tid, aid, sid, told, live) in rows {
        if !threads.contains_key(&tid) {
            threads.insert(tid.clone(), thread_in(tx, &tid)?);
        }
        let seen = threads[&tid].as_ref().is_some_and(|t| {
            t.comments.iter().any(|c| {
                c.id == cid && vis.sees(&visibility::authors_of_comment(c), &aid, Some(&tid))
            })
        });
        if !seen {
            continue;
        }
        if told {
            tx.execute(
                "UPDATE feedback SET placeholder_at = NULL WHERE id = ?1",
                params![fid],
            )?;
        }
        if live {
            touched.targets.insert(sid);
        }
        touched.threads.insert((aid, tid));
    }
    Ok(())
}

/// Revokes live approval `id` at `now` in `tx`, recording `approval.revoke`
/// under `ctx` and marking the visibility change. Undelivered rows it let
/// through are held again by the next take, which tells their sessions of
/// them once (spec §9.4): they were never told while visible, and rows
/// another author withheld all along keep the telling they had.
pub(super) fn revoke_in(
    st: &Store,
    tx: &rusqlite::Transaction<'_>,
    ctx: &AuditCtx,
    id: &str,
    now: &str,
) -> Result<Approval> {
    let a = tx
        .query_row(
            &format!(
                "UPDATE approvals SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL
                 RETURNING {COLUMNS}"
            ),
            params![id, now],
            row_to_approval,
        )
        .optional()?
        .ok_or(CoreError::NotFound)?;
    st.record_audit(tx, ctx, approval_record(AuditKind::ApprovalRevoke, now, &a))?;
    st.visibility_changed();
    Ok(a)
}

/// Records, once per home, `approval.grant` for migration 25's approval of
/// `*unconnected` (made by the migration, so outside any request), as Clax
/// itself (`system:daemon`), at the time the migration made it. Runs as the
/// store opens, after the audit backfill, so the journal grants what a
/// later switch-off revokes.
pub(super) fn record_migration_approval(conn: &mut rusqlite::Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let done: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM install WHERE k = 'migration_approval')",
        [],
        |r| r.get(0),
    )?;
    if done {
        return Ok(());
    }
    let a = tx
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM approvals WHERE created_via = 'migration'
                 ORDER BY created_at, id LIMIT 1"
            ),
            [],
            row_to_approval,
        )
        .optional()?;
    if let Some(a) = a {
        super::audit::insert_at_open(
            &tx,
            &AuditCtx::DAEMON,
            approval_record(AuditKind::ApprovalGrant, &a.created_at, &a),
        )?;
    }
    tx.execute(
        "INSERT INTO install (k, v) VALUES ('migration_approval', '1')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// The installed generation listener, cloned out of its lock to be called.
type Listener = Arc<dyn Fn(u64) + Send + Sync>;

/// The visibility generation (M17): a counter every committed change to
/// what an agent may see bumps once, the listener told of each new value,
/// and the cached answer to "does the owner's agent see everything" (§8.7's
/// fast path) for one generation.
pub(super) struct VisState {
    generation: Arc<AtomicU64>,
    /// Set by [`Store::visibility_changed`] in the write under way; read
    /// and cleared by the writer once it commits. Only the writer touches it.
    changed: AtomicBool,
    /// `((generation + 1) << 1) | sees_all`, or 0 before the first answer.
    fast: AtomicU64,
    /// Who is connected, read at the generation it is tagged with.
    connected: std::sync::Mutex<Option<(u64, Arc<crate::visibility::Connected>)>>,
    listener: RwLock<Option<Listener>>,
}

impl Default for VisState {
    fn default() -> Self {
        VisState {
            generation: Arc::new(AtomicU64::new(0)),
            changed: AtomicBool::new(false),
            fast: AtomicU64::new(0),
            connected: std::sync::Mutex::new(None),
            listener: RwLock::new(None),
        }
    }
}

impl VisState {
    /// Starts a write, forgetting a change marked by one that did not commit.
    pub(super) fn begin(&self) {
        self.changed.store(false, Ordering::Relaxed);
    }

    /// After a commit: bumps the generation when the write changed
    /// visibility, then tells the listener.
    pub(super) fn committed(&self) {
        if !self.changed.swap(false, Ordering::Relaxed) {
            return;
        }
        let g = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let listen = self
            .listener
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(l) = listen
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| l(g))).is_err()
        {
            tracing::error!("the visibility listener panicked; the generation is {g}");
        }
    }
}

impl Store {
    /// The visibility generation: bumped once by every committed write
    /// that changes what an agent may see (M17): an approval made or
    /// revoked; a person's rights or a grant's narrowing set; a grant
    /// ended (revoked, disconnected, its token revoked, or expired); a
    /// person made (an invite, an approved request) or revoked, or
    /// returned to a plain viewer; and a viewer claim, which folds one
    /// viewer's authorship into another.
    pub fn visibility_generation(&self) -> u64 {
        self.visibility.generation.load(Ordering::Acquire)
    }

    /// The counter behind [`Store::visibility_generation`], shared, for
    /// the daemon's state.
    pub fn visibility_counter(&self) -> Arc<AtomicU64> {
        self.visibility.generation.clone()
    }

    /// Installs `f`, called with each new generation once the write that
    /// bumped it has committed (replacing any earlier listener). It runs on
    /// the writer, before the next write starts, so generations reach it in
    /// commit order: it must be quick and must not write to the store.
    pub fn set_visibility_listener(&self, f: impl Fn(u64) + Send + Sync + 'static) {
        *self
            .visibility
            .listener
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// Marks the write under way (in [`Store::with_tx`]) as changing what
    /// agents may see: the generation is bumped once it commits.
    pub(crate) fn visibility_changed(&self) {
        self.visibility.changed.store(true, Ordering::Relaxed);
    }

    /// Whether any viewer is a person (connected now, or revoked): while
    /// none is, the owner's agents see everything the `*unconnected`
    /// approval lets through (§8.7).
    pub fn people_present(&self) -> Result<bool> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS (SELECT 1 FROM viewers WHERE person = 1)",
                [],
                |r| r.get(0),
            )?)
        })
    }

    /// §8.7's fast path: whether the owner's agents see everything, which
    /// holds while no viewer is a person and the owner's `*unconnected`
    /// approval everywhere is live. Answered from a cache for the current
    /// generation (one atomic load) once asked in it.
    pub fn owner_sees_all(&self) -> Result<bool> {
        let g = self.visibility_generation();
        let cached = self.visibility.fast.load(Ordering::Acquire);
        if cached != 0 && (cached >> 1) == g + 1 {
            return Ok(cached & 1 == 1);
        }
        let all = self.with_read(|c| {
            Ok(c.query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM viewers WHERE person = 1)
                    AND EXISTS (SELECT 1 FROM approvals WHERE COALESCE(approver, '') = ''
                                  AND author = ?1 AND scope = 'everywhere' AND revoked_at IS NULL)",
                params![UNCONNECTED],
                |r| r.get::<_, bool>(0),
            )?)
        })?;
        // Tagged with the generation read before the query: a change that
        // committed since makes the tag stale, and the next call asks again.
        self.visibility
            .fast
            .store(((g + 1) << 1) | u64::from(all), Ordering::Release);
        Ok(all)
    }

    /// Who is connected (the owner and every person, revoked ones
    /// included), for marking unconnected authors' names (spec §9.6):
    /// cached for the current generation, which every change to a
    /// viewer's person flag or the owner's row bumps.
    pub fn connected(&self) -> Result<Arc<crate::visibility::Connected>> {
        let g = self.visibility_generation();
        if let Some((at, c)) = self
            .visibility
            .connected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            && *at == g
        {
            return Ok(c.clone());
        }
        // Tagged with the generation read before the query, as `fast` is.
        let c = Arc::new(self.with_read(crate::visibility::Connected::read)?);
        *self
            .visibility
            .connected
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((g, c.clone()));
        Ok(c)
    }

    /// Whether [`Store::connected`] has been read since this store opened:
    /// some agent's view may hold who was connected then.
    pub(crate) fn connected_was_read(&self) -> bool {
        self.visibility
            .connected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Approves `author`'s content reaching the agents of `approver` (a
    /// person's public ID; `None`: the owner) at `scope`, or finds the live
    /// approval that already does. A new approval is recorded as
    /// `approval.grant` under `ctx` and bumps the visibility generation.
    /// Either way, the approver's open requests for `author` that it covers
    /// are closed as approved ([`Store::decide_request`]).
    ///
    /// # Errors
    /// `not_a_person` when `approver` names no connected person (a revoked
    /// one approves nothing); `invalid_author` for
    /// an author that is not a known viewer's public ID, that is the
    /// approver, or that is [`UNCONNECTED`] for anyone but the owner (§9.6);
    /// `unknown_thread` or `unknown_artifact` for a scope naming none.
    pub fn approve(
        &self,
        ctx: &AuditCtx,
        approver: Option<&str>,
        author: &str,
        scope: Scope,
        via: Via,
    ) -> Result<Approval> {
        self.approve_touched(ctx, approver, author, scope, via)
            .map(|(a, _)| a)
    }

    /// [`Store::approve`], also returning what it let through: each live
    /// session of the approver holding an undelivered row whose body it
    /// may now see, and that row's thread, for their waits, pushes and
    /// states.
    ///
    /// # Errors
    /// As [`Store::approve`].
    pub fn approve_touched(
        &self,
        ctx: &AuditCtx,
        approver: Option<&str>,
        author: &str,
        scope: Scope,
        via: Via,
    ) -> Result<(Approval, Touched)> {
        self.with_tx(|tx| {
            let mut touched = Touched::default();
            let a = self.approve_in(tx, ctx, approver, author, scope, via, None, &mut touched)?;
            Ok((a, touched))
        })
    }

    /// [`Store::approve_touched`] in `tx`, made for request `request` when
    /// it decides one.
    #[allow(clippy::too_many_arguments)]
    fn approve_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        approver: Option<&str>,
        author: &str,
        scope: Scope,
        via: Via,
        request: Option<&str>,
        touched: &mut Touched,
    ) -> Result<Approval> {
        if let Some(p) = approver {
            let person: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM viewers WHERE public_id = ?1 AND person = 1
                   AND person_revoked_at IS NULL AND owner = 0)",
                params![p],
                |r| r.get(0),
            )?;
            if !person {
                return Err(CoreError::invalid(
                    "not_a_person",
                    "only the owner or a connected person approves",
                ));
            }
        }
        let owner: Option<String> = tx
            .query_row("SELECT public_id FROM viewers WHERE owner = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        if author == UNCONNECTED {
            if approver.is_some() {
                return Err(CoreError::invalid(
                    "invalid_author",
                    "only the owner approves viewers who are not connected",
                ));
            }
        } else {
            let known: bool = is_public_id(author)
                && tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM viewers WHERE public_id = ?1)",
                    params![author],
                    |r| r.get(0),
                )?;
            let own = match approver {
                Some(p) => p == author,
                None => owner.as_deref() == Some(author),
            };
            if !known || own {
                return Err(CoreError::invalid(
                    "invalid_author",
                    "the author is another viewer's public ID",
                ));
            }
        }
        let (kind, artifact_id, thread_id) = match scope {
            Scope::Everywhere => (ScopeKind::Everywhere, None, None),
            Scope::Artifact(aid) => {
                let live: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id = ?1 AND deleted_at IS NULL)",
                    params![aid],
                    |r| r.get(0),
                )?;
                if !live {
                    return Err(CoreError::invalid(
                        "unknown_artifact",
                        "the scope names no artifact",
                    ));
                }
                (ScopeKind::Artifact, Some(aid), None)
            }
            Scope::Thread(tid) => {
                let aid: Option<String> = tx
                    .query_row(
                        "SELECT artifact_id FROM threads WHERE id = ?1",
                        params![tid],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(aid) = aid else {
                    return Err(CoreError::invalid(
                        "unknown_thread",
                        "the scope names no thread",
                    ));
                };
                (ScopeKind::Thread, Some(aid), Some(tid))
            }
        };
        // A thread approval is matched by its thread alone.
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM approvals
                     WHERE COALESCE(approver, '') = COALESCE(?1, '') AND author = ?2
                       AND scope = ?3 AND COALESCE(artifact_id, '') = COALESCE(?4, '')
                       AND COALESCE(thread_id, '') = COALESCE(?5, '') AND revoked_at IS NULL"
                ),
                params![approver, author, kind.as_str(), artifact_id, thread_id],
                row_to_approval,
            )
            .optional()?;
        let now = self.now();
        let a = match existing {
            Some(a) => a,
            None => {
                let a = Approval {
                    id: new_ulid(),
                    approver: approver.map(str::to_string),
                    author: author.to_string(),
                    scope: kind,
                    artifact_id,
                    thread_id,
                    created_at: now.clone(),
                    created_via: via.as_str().to_string(),
                    request_id: request.map(str::to_string),
                    revoked_at: None,
                };
                tx.execute(
                    "INSERT INTO approvals (id, approver, author, scope, artifact_id, thread_id, created_at, created_via, request_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![a.id, a.approver, a.author, a.scope.as_str(), a.artifact_id, a.thread_id, a.created_at, a.created_via, a.request_id],
                )?;
                self.record_audit(
                    tx,
                    ctx,
                    approval_record(AuditKind::ApprovalGrant, &a.created_at, &a),
                )?;
                release_told(tx, &a, touched)?;
                self.visibility_changed();
                a
            }
        };
        close_covered(self, tx, ctx, &a, &now)?;
        Ok(a)
    }

    /// Revokes the live approval `id` of `approver` (`None`: the owner's),
    /// recorded as `approval.revoke` under `ctx`; the visibility generation
    /// is bumped. Delivered content stays delivered (§9.4).
    ///
    /// # Errors
    /// `NotFound` unless `id` is a live approval of `approver`.
    pub fn revoke_approval(
        &self,
        ctx: &AuditCtx,
        id: &str,
        approver: Option<&str>,
    ) -> Result<Approval> {
        self.with_tx(|tx| {
            let now = self.now();
            let mine: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM approvals WHERE id = ?1
                   AND COALESCE(approver, '') = COALESCE(?2, '') AND revoked_at IS NULL)",
                params![id, approver],
                |r| r.get(0),
            )?;
            if !mine {
                return Err(CoreError::NotFound);
            }
            revoke_in(self, tx, ctx, id, &now)
        })
    }

    /// The live approvals of `approver` (`None`: the owner), oldest first.
    pub fn live_approvals(&self, approver: Option<&str>) -> Result<Vec<Approval>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM approvals
                 WHERE COALESCE(approver, '') = COALESCE(?1, '') AND revoked_at IS NULL
                 ORDER BY created_at, id"
            ))?;
            Ok(stmt
                .query_map(params![approver], row_to_approval)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Where `approver`'s (`None`: the owner) approval of any of `authors`
    /// (public IDs, or [`UNCONNECTED`]) stands at thread `thread` of
    /// artifact `artifact` (spec §8.3): `requested` while a request is open
    /// there (for the thread, or for the artifact outside threads), else
    /// `declined` when one was declined, else `none`.
    pub fn request_state(
        &self,
        approver: Option<&str>,
        artifact: &str,
        thread: &str,
        authors: &[String],
    ) -> Result<ApprovalState> {
        self.with_read(|c| request_state_in(c, approver, artifact, thread, authors))
    }
}

/// Most open requests one approver holds (spec §9.2): beyond, `ask` is 429
/// `limit_reached` and routing holds the content without a request.
pub const MAX_OPEN_REQUESTS: u32 = 100;

/// Most open requests one author holds with one approver (§9.2): past
/// it, that author's further routed content is held without a request
/// (and counts toward People's banner), so no one author spends an
/// approver's [`MAX_OPEN_REQUESTS`] on everyone else.
pub const MAX_OPEN_PER_AUTHOR: u32 = 20;

/// How long after a decline a request there is not made again (§9.3).
pub const DECLINE_QUIET_SECS: i64 = 600;

/// What People shows an approver whose routed content waits without a
/// request (§9.2).
pub const OVERFLOW_BANNER: &str =
    "More is waiting for your approval than the inbox lists: review People";

/// Why a request was made (`approval_requests.cause`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cause {
    /// An agent's `ask`.
    Ask,
    /// A send that reached the approver's agent (M18).
    Routing,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::Ask => "ask",
            Cause::Routing => "routing",
        }
    }
}

/// Where a request stands (`approval_requests.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    Open,
    Approved,
    Declined,
    Withdrawn,
}

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RequestStatus::Open => "open",
            RequestStatus::Approved => "approved",
            RequestStatus::Declined => "declined",
            RequestStatus::Withdrawn => "withdrawn",
        }
    }

    fn parse(s: &str) -> RequestStatus {
        match s {
            "approved" => RequestStatus::Approved,
            "declined" => RequestStatus::Declined,
            "withdrawn" => RequestStatus::Withdrawn,
            _ => RequestStatus::Open,
        }
    }
}

/// A request for an approval (§9.2). It names the withheld content's
/// author, artifact and thread, never the content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    /// The person asked; `None`: the owner.
    pub approver: Option<String>,
    /// The withheld content's author, a public ID.
    pub author: String,
    pub artifact_id: String,
    /// `None`: content outside threads (a version, a document).
    pub thread_id: Option<String>,
    /// The agent it is about.
    pub session_id: Option<String>,
    pub cause: Cause,
    /// How many withheld comments (or, outside threads, fields) it covers.
    pub count: u32,
    pub status: RequestStatus,
    /// The scope it was approved at.
    pub decided_scope: Option<ScopeKind>,
    pub created_at: String,
    pub updated_at: String,
    pub decided_at: Option<String>,
}

const REQUEST_COLUMNS: &str = "id, approver, author, artifact_id, thread_id, session_id, cause,
    count, status, decided_scope, created_at, updated_at, decided_at";

fn row_to_request(r: &Row<'_>) -> rusqlite::Result<ApprovalRequest> {
    let cause: String = r.get(6)?;
    let status: String = r.get(8)?;
    let scope: Option<String> = r.get(9)?;
    Ok(ApprovalRequest {
        id: r.get(0)?,
        approver: r.get(1)?,
        author: r.get(2)?,
        artifact_id: r.get(3)?,
        thread_id: r.get(4)?,
        session_id: r.get(5)?,
        cause: if cause == "ask" {
            Cause::Ask
        } else {
            Cause::Routing
        },
        count: r.get(7)?,
        status: RequestStatus::parse(&status),
        decided_scope: scope.as_deref().and_then(ScopeKind::parse),
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        decided_at: r.get(12)?,
    })
}

/// What a person decides about a request (§9.3, R1): approve at a scope,
/// or decline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approve(ScopeKind),
    Decline,
}

impl Decision {
    /// `thread`, `artifact`, `everywhere` or `decline`.
    pub fn parse(s: &str) -> Option<Decision> {
        match s {
            "decline" => Some(Decision::Decline),
            s => ScopeKind::parse(s).map(Decision::Approve),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Decision::Approve(k) => k.as_str(),
            Decision::Decline => "decline",
        }
    }
}

/// What deciding a request did: the request as decided, the approval made
/// or found, and what that let through (as [`Store::approve_touched`]).
#[derive(Clone, Debug)]
pub struct Decided {
    pub request: ApprovalRequest,
    pub approval: Option<Approval>,
    pub touched: Touched,
}

/// How a send asks the people of the agents it reaches (§9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// A send naming an agent (the picker, `to`, `@name`, a batch or a
    /// page's `sendToClaude` naming one, handoff): makes or joins a request
    /// per author withheld from that agent's person (M18).
    Send,
    /// A send naming no agent, or a later comment on a sent thread: joins
    /// (counts into) the open requests there only; the agent's
    /// placeholder says how to ask.
    Join,
}

/// What [`Store::ask_approval`] asks for: the withheld content of the
/// session's reader at an artifact, or a thread, optionally of one author.
#[derive(Clone, Debug, Default)]
pub struct AskApproval {
    pub session_id: String,
    pub artifact_id: Option<String>,
    pub thread_id: Option<String>,
    /// A public ID.
    pub author: Option<String>,
}

/// What [`request_in`] did for one author.
enum Requested {
    Made(String),
    Joined(String),
    /// Declined there within [`DECLINE_QUIET_SECS`].
    Declined,
    /// The approver holds [`MAX_OPEN_REQUESTS`] open requests, or
    /// [`MAX_OPEN_PER_AUTHOR`] of this author's.
    Full,
    /// [`Route::Join`] with no open request there.
    None,
}

/// The overflow marker's `install` key for `approver`. Its value is the
/// JSON array of the `[author, artifact, thread]` places whose routed
/// content was held without a request.
fn overflow_key(approver: Option<&str>) -> String {
    format!("approval_overflow:{}", approver.unwrap_or(""))
}

/// The places `approver`'s overflow marker holds, read on `c`.
fn overflow_places(
    c: &rusqlite::Connection,
    approver: Option<&str>,
) -> Result<Vec<(String, String, Option<String>)>> {
    let v: Option<String> = c
        .query_row(
            "SELECT v FROM install WHERE k = ?1",
            params![overflow_key(approver)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

/// Writes `places` as `approver`'s overflow marker, or removes it when
/// there are none.
fn set_overflow(
    tx: &Transaction<'_>,
    approver: Option<&str>,
    places: &[(String, String, Option<String>)],
) -> Result<()> {
    if places.is_empty() {
        tx.execute(
            "DELETE FROM install WHERE k = ?1",
            params![overflow_key(approver)],
        )?;
    } else {
        tx.execute(
            "INSERT INTO install (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = excluded.v",
            params![
                overflow_key(approver),
                serde_json::to_string(places).expect("strings serialise")
            ],
        )?;
    }
    Ok(())
}

/// Whether `author`'s content at thread `tid` of `aid` is still held from
/// `approver`'s agents without a request: no request of theirs there is
/// open or was declined (a decline is the approver's answer), an
/// undelivered row of a session of theirs is there, and its author is
/// still withheld.
fn held_without_request(
    tx: &Transaction<'_>,
    approver: Option<&str>,
    author: &str,
    aid: &str,
    tid: Option<&str>,
) -> Result<bool> {
    let Some(tid) = tid else {
        return Ok(false);
    };
    let answered: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM approval_requests
           WHERE COALESCE(approver, '') = COALESCE(?1, '') AND author = ?2
             AND artifact_id = ?3 AND thread_id = ?4 AND status IN ('open', 'declined'))",
        params![approver, author, aid, tid],
        |r| r.get(0),
    )?;
    let waiting: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM feedback f JOIN sessions s ON s.id = f.target_session_id
           WHERE f.thread_id = ?2 AND f.delivered_at IS NULL
             AND COALESCE(s.person, '') = COALESCE(?1, ''))",
        params![approver, tid],
        |r| r.get(0),
    )?;
    if answered || !waiting {
        return Ok(false);
    }
    let Some(t) = thread_in(tx, tid)? else {
        return Ok(false);
    };
    let reader = match approver {
        Some(p) => Reader::Person(p.to_string()),
        None => Reader::Owner,
    };
    let vis = Visibility::for_reader_in(tx, &reader, 0)?;
    let owner = owner_pid(tx)?;
    Ok(withheld_in_thread(&vis, &t, owner.as_deref())
        .iter()
        .any(|(a, _)| a == author))
}

/// The `approval.request` record of request `rid` (§20.2).
fn request_record(at: &str, r: &ApprovalRequest) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ApprovalRequest, at)
        .with("request_id", r.id.as_str())
        .with("approver", r.approver.as_deref())
        .with("author", r.author.as_str())
        .with("artifact_id", r.artifact_id.as_str())
        .with("thread_id", r.thread_id.as_deref())
        .with("cause", r.cause.as_str())
        .with("count", r.count);
    rec.ids.artifact = Some(r.artifact_id.clone());
    rec.ids.thread = r.thread_id.clone();
    rec.ids.session = r.session_id.clone();
    rec
}

/// The `approval.decide` record of request `r` decided as `scope`.
fn decide_record(at: &str, r: &ApprovalRequest, scope: &str) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::ApprovalDecide, at)
        .with("request_id", r.id.as_str())
        .with("scope", scope);
    rec.ids.artifact = Some(r.artifact_id.clone());
    rec.ids.thread = r.thread_id.clone();
    rec
}

/// Request `rid`, read on `c`.
fn request_by_id(c: &rusqlite::Connection, rid: &str) -> Result<Option<ApprovalRequest>> {
    Ok(c.query_row(
        &format!("SELECT {REQUEST_COLUMNS} FROM approval_requests WHERE id = ?1"),
        params![rid],
        row_to_request,
    )
    .optional()?)
}

/// The display name of viewer `pid`, for an item's index entry.
fn name_of(c: &rusqlite::Connection, pid: &str) -> Result<String> {
    let raw: Option<String> = c
        .query_row(
            "SELECT display_name FROM viewers WHERE public_id = ?1",
            params![pid],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(clax_core::feedback::display_name(
        raw.as_deref().unwrap_or(""),
    ))
}

/// Makes or joins the request of `approver` for `author`'s withheld
/// content at artifact `aid`, thread `tid` (one per (approver, author,
/// artifact, thread), M19), about session `sid`, covering `count`
/// withheld items. An open one there is joined: when `count` grew, it
/// takes the new count, its item is unread again (re-announced) and an
/// `approval.request` records it; a routing send turns an asking request
/// into a routing one, so the asking session's end does not withdraw it.
/// Otherwise ([`Route::Send`]) a new one is made, recorded and announced
/// in the approver's inbox, unless one was declined there within
/// [`DECLINE_QUIET_SECS`], or the approver holds [`MAX_OPEN_REQUESTS`]
/// open requests or [`MAX_OPEN_PER_AUTHOR`] of this author's (then, for
/// routed content, the place joins the overflow marker behind People's
/// banner).
#[allow(clippy::too_many_arguments)]
fn request_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    approver: Option<&str>,
    author: &str,
    aid: &str,
    tid: Option<&str>,
    sid: Option<&str>,
    cause: Cause,
    route: Route,
    count: u32,
    detail: Option<&Value>,
    now: &str,
) -> Result<Requested> {
    let open = tx
        .query_row(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM approval_requests
                 WHERE COALESCE(approver, '') = COALESCE(?1, '') AND author = ?2
                   AND artifact_id = ?3 AND COALESCE(thread_id, '') = COALESCE(?4, '')
                   AND status = 'open'"
            ),
            params![approver, author, aid, tid],
            row_to_request,
        )
        .optional()?;
    if let Some(mut r) = open {
        let grew = count > r.count;
        let routed = cause == Cause::Routing && r.cause == Cause::Ask;
        if grew || routed {
            r.count = r.count.max(count);
            if routed {
                r.cause = Cause::Routing;
            }
            r.updated_at = now.to_string();
            tx.execute(
                "UPDATE approval_requests SET count = ?2, cause = ?3, updated_at = ?4 WHERE id = ?1",
                params![r.id, r.count, r.cause.as_str(), now],
            )?;
            if grew {
                super::inbox::approval_reannounced(tx, &r.id, detail)?;
                st.record_audit(tx, ctx, request_record(now, &r))?;
            }
        }
        return Ok(Requested::Joined(r.id));
    }
    if route == Route::Join {
        return Ok(Requested::None);
    }
    let quiet =
        clax_core::time::stamp(st.now_utc() - chrono::Duration::seconds(DECLINE_QUIET_SECS));
    let declined: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM approval_requests
           WHERE COALESCE(approver, '') = COALESCE(?1, '') AND author = ?2
             AND artifact_id = ?3 AND COALESCE(thread_id, '') = COALESCE(?4, '')
             AND status = 'declined' AND decided_at > ?5)",
        params![approver, author, aid, tid, quiet],
        |r| r.get(0),
    )?;
    if declined {
        return Ok(Requested::Declined);
    }
    let held: u32 = tx.query_row(
        "SELECT count(*) FROM approval_requests INDEXED BY approval_requests_open
         WHERE COALESCE(approver, '') = COALESCE(?1, '') AND status = 'open'",
        params![approver],
        |r| r.get(0),
    )?;
    let theirs: u32 = tx.query_row(
        "SELECT count(*) FROM approval_requests INDEXED BY approval_requests_open
         WHERE COALESCE(approver, '') = COALESCE(?1, '') AND author = ?2 AND status = 'open'",
        params![approver, author],
        |r| r.get(0),
    )?;
    if held >= MAX_OPEN_REQUESTS || theirs >= MAX_OPEN_PER_AUTHOR {
        // Routed content waits without a request: People's banner says so.
        if cause == Cause::Routing {
            let mut places = overflow_places(tx, approver)?;
            let place = (author.to_string(), aid.to_string(), tid.map(str::to_string));
            if !places.contains(&place) {
                places.push(place);
                set_overflow(tx, approver, &places)?;
            }
        }
        return Ok(Requested::Full);
    }
    let r = ApprovalRequest {
        id: new_ulid(),
        approver: approver.map(str::to_string),
        author: author.to_string(),
        artifact_id: aid.to_string(),
        thread_id: tid.map(str::to_string),
        session_id: sid.map(str::to_string),
        cause,
        count,
        status: RequestStatus::Open,
        decided_scope: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
        decided_at: None,
    };
    tx.execute(
        "INSERT INTO approval_requests (id, approver, author, artifact_id, thread_id, session_id,
            cause, count, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9, ?9)",
        params![
            r.id,
            r.approver,
            r.author,
            r.artifact_id,
            r.thread_id,
            r.session_id,
            r.cause.as_str(),
            r.count,
            now
        ],
    )?;
    let name = name_of(tx, author)?;
    super::inbox::note_approval(
        tx,
        &r.id,
        approver,
        aid,
        tid,
        sid,
        &name,
        detail.cloned(),
        now,
    )?;
    st.record_audit(tx, ctx, request_record(now, &r))?;
    Ok(Requested::Made(r.id))
}

/// Closes, as approved at its scope, every open request of `a`'s approver
/// for `a`'s author that `a` covers, each recorded as `approval.decide`
/// under `ctx` and its item read.
fn close_covered(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    a: &Approval,
    now: &str,
) -> Result<()> {
    let closed: Vec<ApprovalRequest> = {
        let mut stmt = tx.prepare_cached(&format!(
            "UPDATE approval_requests SET status = 'approved', decided_scope = ?3, decided_at = ?6,
                    updated_at = ?6
             WHERE status = 'open' AND COALESCE(approver, '') = COALESCE(?1, '') AND author = ?2
               AND (?3 = 'everywhere'
                    OR (?3 = 'artifact' AND artifact_id = ?4)
                    OR (?3 = 'thread' AND thread_id = ?5))
             RETURNING {REQUEST_COLUMNS}"
        ))?;
        stmt.query_map(
            params![
                a.approver,
                a.author,
                a.scope.as_str(),
                a.artifact_id,
                a.thread_id,
                now
            ],
            row_to_request,
        )?
        .collect::<rusqlite::Result<_>>()?
    };
    for r in &closed {
        super::inbox::approval_closed(tx, &r.id, now)?;
        st.record_audit(tx, ctx, decide_record(now, r, a.scope.as_str()))?;
    }
    Ok(())
}

/// The key a request names `author` by: a public ID (the owner's for
/// [`Author::Owner`]); `None` for authors no request can name (anonymous
/// viewers, whom only the owner's switch covers, and Clax itself).
fn request_key(a: &Author, owner: Option<&str>) -> Option<String> {
    match a {
        Author::Owner => owner.map(str::to_string),
        Author::Person(p) => Some(p.clone()),
        Author::Unconnected | Author::System => None,
    }
}

/// The owner's public ID, read on `c`.
fn owner_pid(c: &rusqlite::Connection) -> Result<Option<String>> {
    Ok(
        c.query_row("SELECT public_id FROM viewers WHERE owner = 1", [], |r| {
            r.get(0)
        })
        .optional()?,
    )
}

/// The reader whose agents session `sid` is, read on `c`; `None` for a
/// session that does not exist.
fn reader_in(c: &rusqlite::Connection, sid: &str) -> Result<Option<(Reader, bool)>> {
    let row: Option<(Option<String>, bool)> = c
        .query_row(
            "SELECT person, ended_at IS NULL FROM sessions WHERE id = ?1",
            params![sid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(p, live)| {
        (
            match p {
                Some(p) => Reader::Person(p),
                None => Reader::Owner,
            },
            live,
        )
    }))
}

/// The approver a reader's requests go to.
fn approver_of(r: &Reader) -> Option<&str> {
    match r {
        Reader::Owner => None,
        Reader::Person(p) => Some(p),
    }
}

/// The withheld authors of thread `t`'s comment bodies for `vis`, each
/// with how many of its comments they withhold, in first-seen order.
fn withheld_in_thread(vis: &Visibility, t: &Thread, owner: Option<&str>) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for c in &t.comments {
        for a in vis.withheld_authors(
            &visibility::authors_of_comment(c),
            &t.artifact_id,
            Some(&t.id),
        ) {
            let Some(key) = request_key(&a, owner) else {
                continue;
            };
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += 1,
                None => out.push((key, 1)),
            }
        }
    }
    out
}

/// The detail of a page's send of thread `t` (§9.2: "<publisher>'s page,
/// via <viewer>'s browser"): the newest comment a page wrote, its page's
/// authors and its viewer.
fn page_detail(t: &Thread) -> Option<Value> {
    let c = t.comments.iter().rev().find(|c| c.via_page)?;
    Some(json!({"page": {
        "authors": if c.page_authors.is_empty() { vec![OWNER.to_string()] } else { c.page_authors.clone() },
        "viewer": c.author_public_id,
    }}))
}

/// The requests a send of thread `thread_id` to `sessions` makes or joins
/// (M18, §9.2): for each session's person (`None`: the owner), once per
/// person, one per author whose comments in the thread that person's
/// agents may not see, counting them; a page's send (`page`) names the
/// page and viewer on the item. A [`Route::Join`] only counts into open
/// requests. Content of an author at a full approver is held without a
/// request. The IDs of the requests made or joined.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    thread_id: &str,
    sessions: &[String],
    route: Route,
    page: bool,
    now: &str,
) -> Result<Vec<String>> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let Some(t) = thread_in(tx, thread_id)? else {
        return Ok(Vec::new());
    };
    if !t.comments.iter().any(|c| c.author_kind == AUTHOR_VIEWER) {
        return Ok(Vec::new());
    }
    let owner = owner_pid(tx)?;
    let detail = if page { page_detail(&t) } else { None };
    let mut by_reader: BTreeMap<Option<String>, String> = BTreeMap::new();
    for sid in sessions {
        if let Some((r, true)) = reader_in(tx, sid)? {
            by_reader
                .entry(approver_of(&r).map(str::to_string))
                .or_insert_with(|| sid.clone());
        }
    }
    let mut ids = Vec::new();
    for (approver, sid) in by_reader {
        let reader = match &approver {
            Some(p) => Reader::Person(p.clone()),
            None => Reader::Owner,
        };
        let vis = Visibility::for_reader_in(tx, &reader, 0)?;
        for (author, count) in withheld_in_thread(&vis, &t, owner.as_deref()) {
            match request_in(
                st,
                tx,
                ctx,
                approver.as_deref(),
                &author,
                &t.artifact_id,
                Some(&t.id),
                Some(&sid),
                Cause::Routing,
                route,
                count,
                detail.as_ref(),
                now,
            )? {
                Requested::Made(id) | Requested::Joined(id) => ids.push(id),
                Requested::Declined | Requested::Full | Requested::None => {}
            }
        }
    }
    Ok(ids)
}

/// Withdraws, at `now`, the open requests `filter` (a `WHERE` condition
/// on `approval_requests` over `args`) selects, each recorded as
/// `approval.withdraw {request_id, reason}` under `ctx` and its item read.
fn withdraw_where(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
    reason: &str,
    now: &str,
) -> Result<Vec<String>> {
    let ids: Vec<ApprovalRequest> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT {REQUEST_COLUMNS} FROM approval_requests WHERE status = 'open' AND ({filter})"
        ))?;
        stmt.query_map(args, row_to_request)?
            .collect::<rusqlite::Result<_>>()?
    };
    for r in &ids {
        tx.execute(
            "UPDATE approval_requests SET status = 'withdrawn', decided_at = ?2, updated_at = ?2
             WHERE id = ?1",
            params![r.id, now],
        )?;
        super::inbox::approval_closed(tx, &r.id, now)?;
        let mut rec = AuditRecord::new(AuditKind::ApprovalWithdraw, now)
            .with("request_id", r.id.as_str())
            .with("reason", reason);
        rec.ids.artifact = Some(r.artifact_id.clone());
        rec.ids.thread = r.thread_id.clone();
        st.record_audit(tx, ctx, rec)?;
    }
    Ok(ids.into_iter().map(|r| r.id).collect())
}

/// Withdraws the open requests about thread `tid` (§9.2: it was resolved
/// or deleted), for `reason`.
pub(crate) fn withdraw_thread_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    tid: &str,
    reason: &str,
    now: &str,
) -> Result<()> {
    withdraw_where(st, tx, ctx, "thread_id = ?1", &[&tid], reason, now).map(drop)
}

/// Withdraws the open requests about artifact `aid` (§9.2: it was deleted).
pub(crate) fn withdraw_artifact_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    aid: &str,
    now: &str,
) -> Result<()> {
    withdraw_where(
        st,
        tx,
        ctx,
        "artifact_id = ?1",
        &[&aid],
        "artifact_deleted",
        now,
    )
    .map(drop)
}

/// Withdraws the open requests session `sid` asked for (§9.2, §14: its
/// agent ended), returning their IDs; routing requests stay.
pub(crate) fn withdraw_session_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    sid: &str,
    now: &str,
) -> Result<Vec<String>> {
    withdraw_where(
        st,
        tx,
        ctx,
        "session_id = ?1 AND cause = 'ask'",
        &[&sid],
        "session_end",
        now,
    )
}

/// Withdraws session `sid`'s open `ask` requests for thread `tid`, whose
/// held rows a take or handoff removed (spec 2026-10-08 §12): approving
/// them would release nothing to it.
pub(crate) fn withdraw_taken_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    tid: &str,
    sid: &str,
    now: &str,
) -> Result<()> {
    withdraw_where(
        st,
        tx,
        ctx,
        "thread_id = ?1 AND session_id = ?2 AND cause = 'ask'",
        &[&tid, &sid],
        "thread_taken",
        now,
    )
    .map(drop)
}

/// Withdraws the open requests of person `pid`, asked of them or about
/// their content, once they are revoked: a revoked person approves
/// nothing, and their content stays withheld (§6.4).
pub(crate) fn withdraw_person_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    pid: &str,
    now: &str,
) -> Result<()> {
    withdraw_where(
        st,
        tx,
        ctx,
        "approver = ?1 OR author = ?1",
        &[&pid],
        "person_revoked",
        now,
    )
    .map(drop)
}

/// The authors withheld from `vis` at artifact `aid` outside its threads
/// (§9.2's `thread_id` NULL): its text fields, its current version's
/// label, note and files, its documents and its assets, each counted once
/// per field that holds them.
fn withheld_outside_threads(
    tx: &Transaction<'_>,
    vis: &Visibility,
    aid: &str,
    owner: Option<&str>,
) -> Result<Vec<(String, u32)>> {
    let Some(a) = super::artifacts::live_artifact_in(tx, aid)? else {
        return Err(CoreError::NotFound);
    };
    let mut fields: Vec<Vec<Author>> = vec![visibility::authors_of_artifact_text(&a)];
    if let Some(v) = tx
        .query_row(
            &format!(
                "{} WHERE artifact_id = ?1 AND n = ?2",
                super::artifacts::SELECT_VERSION
            ),
            params![aid, a.current_version],
            super::artifacts::row_to_version,
        )
        .optional()?
        .transpose()?
    {
        fields.push(visibility::authors_of_version(&v));
        for path in v.files.keys() {
            fields.push(visibility::authors_of_file(&v, path));
        }
    }
    let docs: Vec<String> = {
        let mut stmt = tx.prepare_cached(
            "SELECT DISTINCT author FROM doc_authors WHERE artifact_id = ?1 ORDER BY author",
        )?;
        stmt.query_map(params![aid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for d in docs {
        fields.push(vec![Author::of_token(Some(&d))]);
    }
    let uploaders: Vec<Option<String>> = {
        let mut stmt = tx.prepare_cached(
            "SELECT DISTINCT person FROM assets WHERE artifact_id = ?1 ORDER BY person",
        )?;
        stmt.query_map(params![aid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for u in uploaders {
        fields.push(vec![Author::of_token(u.as_deref())]);
    }
    let mut out: Vec<(String, u32)> = Vec::new();
    for f in fields {
        for x in vis.withheld_authors(&f, aid, None) {
            let Some(key) = request_key(&x, owner) else {
                continue;
            };
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => *n += 1,
                None => out.push((key, 1)),
            }
        }
    }
    Ok(out)
}

impl Store {
    /// Every approver (`None`: the owner) a request at thread `tid` named,
    /// whatever became of it.
    ///
    /// # Errors
    /// The store's.
    pub fn request_approvers_at(&self, tid: &str) -> Result<Vec<Option<String>>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT DISTINCT approver FROM approval_requests WHERE thread_id = ?1",
            )?;
            Ok(st
                .query_map(params![tid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The people whose agents a send of `threads` (of artifact `aid`)
    /// naming one of them would ask first (§10, "Sam approves first"),
    /// as `asker` may learn it (§18, decided 2026-10-13 "Only about their
    /// content"): among the people with a live agent that owns or watches
    /// `aid` (the agents its Send menu lists), each whose agents may not
    /// see an author of one of the threads' comments that a routing
    /// request names (§9.2, M18). For the owner (`asker` `None`) every
    /// such person; for a connected person P, P whenever P's agents would
    /// hold any author, and any other person (the owner included) only
    /// when their agents would hold content P authored there (P's
    /// comments, or a page P published): never another person's approval
    /// of a third author. By public ID (`None`: the owner), in the order
    /// their agents started. Empty while no viewer is a person (§19).
    ///
    /// # Errors
    /// The store's.
    pub fn approves_first(
        &self,
        aid: &clax_core::ArtifactId,
        threads: &[Thread],
        asker: Option<&str>,
    ) -> Result<Vec<Option<String>>> {
        let mut out = Vec::new();
        let threads: Vec<&Thread> = threads
            .iter()
            .filter(|t| {
                t.artifact_id == aid.as_str()
                    && t.comments.iter().any(|c| c.author_kind == AUTHOR_VIEWER)
            })
            .collect();
        if threads.is_empty() || !self.people_present()? {
            return Ok(out);
        }
        let (owner, people) = self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT s.person FROM sessions s
                  WHERE s.ended_at IS NULL
                    AND (s.id = (SELECT owner_session_id FROM artifacts WHERE id = ?1)
                         OR EXISTS (SELECT 1 FROM watches w WHERE w.session_id = s.id AND w.artifact_id = ?1))
                  GROUP BY s.person ORDER BY MIN(s.started_at), s.person",
            )?;
            let people = st
                .query_map(params![aid.as_str()], |r| r.get::<_, Option<String>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((owner_pid(c)?, people))
        })?;
        let generation = self.visibility_generation();
        for person in people {
            let reader = match &person {
                Some(p) => Reader::Person(p.clone()),
                None => Reader::Owner,
            };
            let vis = Visibility::for_reader(self, &reader, generation)?;
            // Whose content this asker may weigh for this person: any
            // author for the owner and for the asker's own agents, else
            // the asker's own content alone.
            let all = asker.is_none() || person.as_deref() == asker;
            if threads.iter().any(|t| {
                withheld_in_thread(&vis, t, owner.as_deref())
                    .iter()
                    .any(|(author, _)| all || Some(author.as_str()) == asker)
            }) {
                out.push(person);
            }
        }
        Ok(out)
    }

    /// Asks session `q.session_id`'s person for the content withheld from
    /// its agent at `q.thread_id` (its comments' bodies), or else at
    /// `q.artifact_id` outside threads, of `q.author` alone when given
    /// (§9.2 "Asking"): makes or joins one request per such author
    /// (`cause: ask`), recorded as `approval.request` under `ctx`.
    /// Returns the requests, oldest author first.
    ///
    /// # Errors
    /// `NotFound` for a session, thread or artifact that does not exist
    /// (or a thread of another artifact than the one named);
    /// `unknown_session` for an ended session; `invalid_args` naming
    /// neither; `nothing_withheld` when nothing there is withheld from the
    /// agent (of that author); `recently_declined` when every request
    /// there was declined within [`DECLINE_QUIET_SECS`]; `limit_reached`
    /// when the person holds [`MAX_OPEN_REQUESTS`] open requests.
    pub fn ask_approval(&self, ctx: &AuditCtx, q: &AskApproval) -> Result<Vec<ApprovalRequest>> {
        self.with_tx(|tx| {
            let Some((reader, live)) = reader_in(tx, &q.session_id)? else {
                return Err(CoreError::NotFound);
            };
            if !live {
                return Err(CoreError::invalid(
                    "unknown_session",
                    "no live session has this ID",
                ));
            }
            let now = self.now();
            let owner = owner_pid(tx)?;
            let vis = Visibility::for_reader_in(tx, &reader, 0)?;
            let (aid, tid, withheld) = match (&q.thread_id, &q.artifact_id) {
                (Some(tid), named) => {
                    let t = thread_in(tx, tid)?.ok_or(CoreError::NotFound)?;
                    if named.as_ref().is_some_and(|a| *a != t.artifact_id)
                        || super::artifacts::live_artifact_in(tx, &t.artifact_id)?.is_none()
                    {
                        return Err(CoreError::NotFound);
                    }
                    let w = withheld_in_thread(&vis, &t, owner.as_deref());
                    (t.artifact_id.clone(), Some(t.id.clone()), w)
                }
                (None, Some(aid)) => {
                    let w = withheld_outside_threads(tx, &vis, aid, owner.as_deref())?;
                    (aid.clone(), None, w)
                }
                (None, None) => {
                    return Err(CoreError::invalid(
                        "invalid_args",
                        "name a thread_id or an artifact",
                    ));
                }
            };
            let withheld: Vec<(String, u32)> = withheld
                .into_iter()
                .filter(|(a, _)| q.author.as_ref().is_none_or(|w| w == a))
                .collect();
            if withheld.is_empty() {
                return Err(CoreError::invalid(
                    "nothing_withheld",
                    "nothing there is withheld from you",
                ));
            }
            let approver = approver_of(&reader);
            let (mut ids, mut declined, mut full) = (Vec::new(), false, false);
            for (author, count) in withheld {
                match request_in(
                    self,
                    tx,
                    ctx,
                    approver,
                    &author,
                    &aid,
                    tid.as_deref(),
                    Some(&q.session_id),
                    Cause::Ask,
                    Route::Send,
                    count,
                    None,
                    &now,
                )? {
                    Requested::Made(id) | Requested::Joined(id) => ids.push(id),
                    Requested::Declined => declined = true,
                    Requested::Full => full = true,
                    Requested::None => {}
                }
            }
            if ids.is_empty() {
                return Err(if full {
                    CoreError::invalid(
                        "limit_reached",
                        format!(
                            "your person has as many requests waiting as they may hold ({MAX_OPEN_REQUESTS}, or {MAX_OPEN_PER_AUTHOR} for one author); ask again once they decide some"
                        ),
                    )
                } else {
                    debug_assert!(declined);
                    CoreError::invalid(
                        "recently_declined",
                        "your person declined this less than ten minutes ago",
                    )
                });
            }
            ids.iter()
                .map(|id| request_by_id(tx, id)?.ok_or(CoreError::NotFound))
                .collect()
        })
    }

    /// What author `author` (a public ID; the owner's stands for the owner)
    /// holds at artifact `aid` outside its threads, for an approval card
    /// (§9.5): the current version's files they wrote, the documents they
    /// hold fields of (by path; a document without recorded authors is the
    /// owner's), and their uploads (`[asset ID, content type, size]`).
    ///
    /// # Errors
    /// `NotFound` for an artifact that is gone.
    #[allow(clippy::type_complexity)]
    pub fn authored_outside_threads(
        &self,
        aid: &str,
        author: &str,
    ) -> Result<(Vec<String>, Vec<String>, Vec<(String, String, i64)>)> {
        self.with_read(|c| {
            let a = super::artifacts::live_artifact_in(c, aid)?.ok_or(CoreError::NotFound)?;
            let owner = owner_pid(c)?;
            let is_owner = owner.as_deref() == Some(author);
            let who = if is_owner {
                Author::Owner
            } else {
                Author::Person(author.to_string())
            };
            let mut files = Vec::new();
            if let Some(v) = c
                .query_row(
                    &format!(
                        "{} WHERE artifact_id = ?1 AND n = ?2",
                        super::artifacts::SELECT_VERSION
                    ),
                    params![aid, a.current_version],
                    super::artifacts::row_to_version,
                )
                .optional()?
                .transpose()?
            {
                for path in v.files.keys() {
                    let by =
                        visibility::normalise(&visibility::authors_of_file(&v, path), owner.as_deref());
                    if by.contains(&who) {
                        files.push(path.clone());
                    }
                }
            }
            let docs: Vec<String> = c
                .prepare(
                    "SELECT d.path FROM docs d WHERE d.artifact_id = ?1
                       AND (EXISTS (SELECT 1 FROM doc_authors x WHERE x.artifact_id = d.artifact_id
                                      AND x.path = d.path AND (x.author = ?2 OR (?3 AND x.author = '*owner')))
                            OR (?3 AND NOT EXISTS (SELECT 1 FROM doc_authors x
                                      WHERE x.artifact_id = d.artifact_id AND x.path = d.path)))
                     ORDER BY d.path",
                )?
                .query_map(params![aid, author, is_owner], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let assets: Vec<(String, String, i64)> = c
                .prepare(
                    "SELECT id, content_type, size FROM assets WHERE artifact_id = ?1
                       AND (person = ?2 OR (?3 AND (person IS NULL OR person = '*owner')))
                     ORDER BY created_at, id",
                )?
                .query_map(params![aid, author, is_owner], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok((files, docs, assets))
        })
    }

    /// Request `rid` of `approver` (`None`: the owner).
    ///
    /// # Errors
    /// `NotFound` unless `rid` is a request of `approver`.
    pub fn approval_request(&self, rid: &str, approver: Option<&str>) -> Result<ApprovalRequest> {
        self.with_read(|c| match request_by_id(c, rid)? {
            Some(r) if r.approver.as_deref() == approver => Ok(r),
            _ => Err(CoreError::NotFound),
        })
    }

    /// The requests whose rows are `rowids` (as the request listener hears
    /// of them), oldest first.
    pub fn approval_requests_by_rowid(&self, rowids: &[i64]) -> Result<Vec<ApprovalRequest>> {
        let ids = serde_json::to_string(rowids).expect("integers serialise");
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {REQUEST_COLUMNS} FROM approval_requests
                 WHERE rowid IN (SELECT value FROM json_each(?1)) ORDER BY rowid"
            ))?;
            Ok(stmt
                .query_map(params![ids], row_to_request)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The requests of `approver` (`None`: the owner): the open ones only
    /// when `open`, newest first, at most `limit`; the number open; and
    /// whether routed content waits beyond [`MAX_OPEN_REQUESTS`] without a
    /// request (People's banner, [`OVERFLOW_BANNER`]).
    pub fn approval_requests(
        &self,
        approver: Option<&str>,
        open: bool,
        limit: u32,
    ) -> Result<(Vec<ApprovalRequest>, u32, bool)> {
        self.with_read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {REQUEST_COLUMNS} FROM approval_requests
                 WHERE COALESCE(approver, '') = COALESCE(?1, '') AND (NOT ?2 OR status = 'open')
                 ORDER BY created_at DESC, id DESC LIMIT ?3"
            ))?;
            let rows = stmt
                .query_map(params![approver, open, limit], row_to_request)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let n: u32 = c.query_row(
                "SELECT count(*) FROM approval_requests INDEXED BY approval_requests_open
                 WHERE COALESCE(approver, '') = COALESCE(?1, '') AND status = 'open'",
                params![approver],
                |r| r.get(0),
            )?;
            let overflow: bool = c.query_row(
                "SELECT EXISTS (SELECT 1 FROM install WHERE k = ?1)",
                params![overflow_key(approver)],
                |r| r.get(0),
            )?;
            Ok((rows, n, overflow))
        })
    }

    /// `approver` (`None`: the owner) decides open request `rid` (§9.3):
    /// approving makes the approval at the scope (or finds the live one;
    /// [`Store::approve_touched`]), which closes it and the approver's
    /// other open requests for that author the scope covers, each as
    /// `approval.decide`, and bumps the visibility generation; declining
    /// marks it declined (`approval.decide` with scope `decline`), keeps
    /// its rows held and bumps the generation too. Either way its item is
    /// read. Then each place the approver's overflow marker holds is
    /// dropped once its content is no longer held without a request
    /// (`held_without_request`), and the marker goes when none is left.
    ///
    /// # Errors
    /// `NotFound` unless `rid` is a request of `approver`;
    /// `request_closed` unless it is open; `invalid_scope` for a thread
    /// scope on a request outside threads; [`Store::approve`]'s.
    pub fn decide_request(
        &self,
        ctx: &AuditCtx,
        rid: &str,
        approver: Option<&str>,
        decision: Decision,
        via: Via,
    ) -> Result<Decided> {
        self.with_tx(|tx| {
            let r = match request_by_id(tx, rid)? {
                Some(r) if r.approver.as_deref() == approver => r,
                _ => return Err(CoreError::NotFound),
            };
            if r.status != RequestStatus::Open {
                return Err(CoreError::invalid(
                    "request_closed",
                    format!("the request is {} already", r.status.as_str()),
                ));
            }
            let now = self.now();
            let mut touched = Touched::default();
            let approval = match decision {
                Decision::Approve(kind) => {
                    let scope = match kind {
                        ScopeKind::Everywhere => Scope::Everywhere,
                        ScopeKind::Artifact => Scope::Artifact(r.artifact_id.clone()),
                        ScopeKind::Thread => match &r.thread_id {
                            Some(t) => Scope::Thread(t.clone()),
                            None => {
                                return Err(CoreError::invalid(
                                    "invalid_scope",
                                    "the request is about no thread: allow the page or everywhere",
                                ));
                            }
                        },
                    };
                    let a = self.approve_in(
                        tx,
                        ctx,
                        approver,
                        &r.author,
                        scope,
                        via,
                        Some(rid),
                        &mut touched,
                    )?;
                    Some(a)
                }
                Decision::Decline => {
                    tx.execute(
                        "UPDATE approval_requests SET status = 'declined', decided_at = ?2, updated_at = ?2
                         WHERE id = ?1",
                        params![rid, now],
                    )?;
                    super::inbox::approval_closed(tx, rid, &now)?;
                    self.record_audit(tx, ctx, decide_record(&now, &r, decision.as_str()))?;
                    // Its thread waits for this approval no longer: its
                    // state is announced again.
                    if let Some(tid) = &r.thread_id {
                        touched.threads.insert((r.artifact_id.clone(), tid.clone()));
                    }
                    None
                }
            };
            // Every decision bumps the generation (M17): a decline changes
            // what the withheld rows' placeholders say.
            self.visibility_changed();
            // The banner stays while routed content it counted is still
            // held without a request.
            let places = overflow_places(tx, approver)?;
            if !places.is_empty() {
                let mut left = Vec::new();
                for p in places {
                    if held_without_request(tx, approver, &p.0, &p.1, p.2.as_deref())? {
                        left.push(p);
                    }
                }
                set_overflow(tx, approver, &left)?;
            }
            let request = request_by_id(tx, rid)?.ok_or(CoreError::NotFound)?;
            Ok(Decided {
                request,
                approval,
                touched,
            })
        })
    }

    /// Revokes the live approval `id` of `approver` and returns it with
    /// the live sessions of the approver holding undelivered rows in its
    /// scope, whose waits learn of it. As [`Store::revoke_approval`].
    ///
    /// # Errors
    /// As [`Store::revoke_approval`].
    pub fn revoke_approval_touched(
        &self,
        ctx: &AuditCtx,
        id: &str,
        approver: Option<&str>,
    ) -> Result<(Approval, Touched)> {
        let a = self.revoke_approval(ctx, id, approver)?;
        let mut touched = Touched::default();
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT DISTINCT f.target_session_id FROM feedback f
                 JOIN sessions s ON s.id = f.target_session_id
                 JOIN threads t ON t.id = f.thread_id
                 WHERE f.delivered_at IS NULL AND s.ended_at IS NULL
                   AND COALESCE(s.person, '') = COALESCE(?1, '')
                   AND (?2 = 'everywhere'
                        OR (?2 = 'artifact' AND t.artifact_id = ?3)
                        OR (?2 = 'thread' AND f.thread_id = ?4))",
            )?;
            for sid in stmt.query_map(
                params![a.approver, a.scope.as_str(), a.artifact_id, a.thread_id],
                |r| r.get::<_, String>(0),
            )? {
                touched.targets.insert(sid?);
            }
            Ok(())
        })?;
        Ok((a, touched))
    }

    /// The live approval `id` of `approver` (`None`: the owner).
    ///
    /// # Errors
    /// `NotFound` unless it is a live approval of `approver`.
    pub fn live_approval(&self, id: &str, approver: Option<&str>) -> Result<Approval> {
        self.with_read(|c| {
            c.query_row(
                &format!(
                    "SELECT {COLUMNS} FROM approvals WHERE id = ?1
                       AND COALESCE(approver, '') = COALESCE(?2, '') AND revoked_at IS NULL"
                ),
                params![id, approver],
                row_to_approval,
            )
            .optional()?
            .ok_or(CoreError::NotFound)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people::InvitePerson;
    use crate::people::tests::{KEY_B, approved, code, pending, store};
    use crate::test_util::{DAEMON, anchor, artifact};
    use crate::threads::NewThread;
    use clax_core::audit::Via as ClaimVia;

    /// A person named `name`, connected by invite; their public ID.
    fn person(st: &Store, name: &str, key: u8) -> String {
        let (gid, _, _) = approved(st, name, [key; 32]);
        st.viewer_of_grant(&gid).unwrap().public_id
    }

    /// A LAN viewer who is not connected; its public ID.
    fn lan(st: &Store, cookie: &str, name: &str) -> String {
        st.upsert_viewer(cookie, Some(name)).unwrap().public_id
    }

    fn thread(st: &Store, aid: &clax_core::ArtifactId) -> String {
        st.create_thread(
            DAEMON,
            aid,
            NewThread {
                version_n: 1,
                anchor: anchor(),
                author_name: "Lan".into(),
                author_public_id: None,
                body: "b".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn migration_25_gives_the_owner_the_unconnected_approval_everywhere() {
        let (_d, _c, st) = store();
        let live = st.live_approvals(None).unwrap();
        assert_eq!(live.len(), 1, "{live:?}");
        let a = &live[0];
        assert_eq!(
            (
                a.approver.as_deref(),
                a.author.as_str(),
                a.scope,
                a.created_via.as_str()
            ),
            (None, UNCONNECTED, ScopeKind::Everywhere, "migration")
        );
        assert!(a.artifact_id.is_none() && a.thread_id.is_none());
        assert!(st.owner_sees_all().unwrap());
        assert!(!st.people_present().unwrap());
    }

    #[test]
    fn approving_makes_or_finds_the_live_approval_and_revoking_ends_it() {
        let (_d, _c, st) = store();
        let sam = person(&st, "Sam", 1);
        let mia = person(&st, "Mia", 2);
        let aid = artifact(&st, None);
        let tid = thread(&st, &aid);
        let a = st
            .approve(
                DAEMON,
                Some(&sam),
                &mia,
                Scope::Thread(tid.clone()),
                Via::Shell,
            )
            .unwrap();
        assert_eq!(a.scope, ScopeKind::Thread);
        assert_eq!(
            a.artifact_id.as_deref(),
            Some(aid.as_str()),
            "the thread's artifact"
        );
        assert_eq!(a.thread_id.as_deref(), Some(tid.as_str()));
        assert_eq!(a.approver.as_deref(), Some(sam.as_str()));
        let again = st
            .approve(
                DAEMON,
                Some(&sam),
                &mia,
                Scope::Thread(tid.clone()),
                Via::Extension,
            )
            .unwrap();
        assert_eq!(again, a, "the live one is found, not a second made");
        let wide = st
            .approve(
                DAEMON,
                Some(&sam),
                &mia,
                Scope::Artifact(aid.to_string()),
                Via::Shell,
            )
            .unwrap();
        assert_ne!(wide.id, a.id);
        assert_eq!(
            st.live_approvals(Some(&sam)).unwrap(),
            vec![a.clone(), wide.clone()]
        );
        assert_eq!(st.live_approvals(Some(&mia)).unwrap(), vec![]);
        // Only its approver revokes it.
        let not_found = |r: Result<Approval>| matches!(r, Err(CoreError::NotFound));
        assert!(not_found(st.revoke_approval(DAEMON, &a.id, Some(&mia))));
        assert!(not_found(st.revoke_approval(DAEMON, &a.id, None)));
        let gone = st.revoke_approval(DAEMON, &a.id, Some(&sam)).unwrap();
        assert!(gone.revoked_at.is_some());
        assert!(not_found(st.revoke_approval(DAEMON, &a.id, Some(&sam))));
        assert_eq!(st.live_approvals(Some(&sam)).unwrap(), vec![wide]);
        let anew = st
            .approve(DAEMON, Some(&sam), &mia, Scope::Thread(tid), Via::Shell)
            .unwrap();
        assert_ne!(anew.id, a.id, "a revoked approval stays revoked");
    }

    #[test]
    fn approve_refuses_what_no_approval_can_say() {
        let (_d, _c, st) = store();
        let sam = person(&st, "Sam", 1);
        let lan_pid = lan(&st, "01ARZ3NDEKTSV4RRFFQ69G5FAV", "Lan");
        let owner = st.owner_viewer(true).unwrap().public_id;
        let ok = |approver: Option<&str>, author: &str, scope: Scope| match st.approve(
            DAEMON,
            approver,
            author,
            scope,
            Via::Shell,
        ) {
            Ok(_) => "ok",
            e => code(e),
        };
        assert_eq!(
            ok(Some(&lan_pid), &owner, Scope::Everywhere),
            "not_a_person"
        );
        assert_eq!(ok(Some(&owner), &sam, Scope::Everywhere), "not_a_person");
        assert_eq!(
            ok(Some(&sam), UNCONNECTED, Scope::Everywhere),
            "invalid_author"
        );
        assert_eq!(ok(Some(&sam), &sam, Scope::Everywhere), "invalid_author");
        assert_eq!(ok(None, &owner, Scope::Everywhere), "invalid_author");
        assert_eq!(
            ok(Some(&sam), "u_0000000000000000000000", Scope::Everywhere),
            "invalid_author"
        );
        assert_eq!(
            ok(Some(&sam), "*owner", Scope::Everywhere),
            "invalid_author"
        );
        assert_eq!(
            ok(Some(&sam), &owner, Scope::Thread("t9".into())),
            "unknown_thread"
        );
        assert_eq!(
            ok(Some(&sam), &owner, Scope::Artifact("zzzzzzzzzzzz".into())),
            "unknown_artifact"
        );
        // What may be said: a person approving the owner, or a LAN viewer.
        assert_eq!(ok(Some(&sam), &owner, Scope::Everywhere), "ok");
        assert_eq!(ok(Some(&sam), &lan_pid, Scope::Everywhere), "ok");
        assert_eq!(ok(None, &sam, Scope::Everywhere), "ok");
    }

    #[test]
    fn approvals_are_recorded_with_their_scope_and_never_more() {
        let (_d, _c, st) = store();
        let sam = person(&st, "Sam", 1);
        let mia = person(&st, "Mia", 2);
        let aid = artifact(&st, None);
        let tid = thread(&st, &aid);
        let before = st.events_after(0, 10_000).unwrap().last().unwrap().seq;
        let a = st
            .approve(
                DAEMON,
                Some(&sam),
                &mia,
                Scope::Thread(tid.clone()),
                Via::Shell,
            )
            .unwrap();
        st.approve(
            DAEMON,
            Some(&sam),
            &mia,
            Scope::Thread(tid.clone()),
            Via::Shell,
        )
        .unwrap();
        st.revoke_approval(DAEMON, &a.id, Some(&sam)).unwrap();
        let rows = st.events_after(before, 10_000).unwrap();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["approval.grant", "approval.revoke"],
            "finding records nothing"
        );
        for r in &rows {
            let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            assert_eq!(body["approval_id"], a.id.as_str());
            assert_eq!(body["approver"], sam.as_str());
            assert_eq!(body["author"], mia.as_str());
            assert_eq!(body["scope"], "thread");
            assert_eq!(body["created_via"], "shell");
            assert_eq!(r.ids.artifact.as_deref(), Some(aid.as_str()));
            assert_eq!(r.ids.thread.as_deref(), Some(tid.as_str()));
        }
    }

    /// Every source M17 lists bumps the generation once; what changes
    /// nothing bumps nothing.
    #[test]
    fn every_visibility_change_bumps_the_generation_once() {
        let (_d, _c, st) = store();
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = heard.clone();
        st.set_visibility_listener(move |g| h.lock().unwrap().push(g));
        let g = |st: &Store| st.visibility_generation();
        let mut last = g(&st);
        let mut bumps = |st: &Store, what: &str, n: u64| {
            let now = g(st);
            assert_eq!(now - last, n, "{what}");
            last = now;
        };
        // A person made by invite (and a viewer made one).
        let (gid, _, manage) = approved(&st, "Sam", [1; 32]);
        bumps(&st, "an invite making a person", 1);
        let sam = st.viewer_of_grant(&gid).unwrap().public_id;
        let lan_pid = lan(&st, "01ARZ3NDEKTSV4RRFFQ69G5FAV", "Lan");
        bumps(&st, "a LAN viewer arriving", 0);
        let (invite, _) = st
            .create_invite(DAEMON, InvitePerson::Viewer(lan_pid.clone()), None)
            .unwrap();
        bumps(&st, "an invite making a viewer a person", 1);
        st.revoke_invite(DAEMON, &invite.id).unwrap();
        bumps(&st, "a lapsed person returning to a viewer", 1);
        st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        bumps(
            &st,
            "an invite for the owner's machine making the owner's row",
            1,
        );
        st.create_invite(DAEMON, InvitePerson::Owner, None).unwrap();
        bumps(&st, "an invite for the owner's own machine", 0);
        // A request approved into a new person.
        let (req, _, user_code) = pending(&st, KEY_B);
        bumps(&st, "a pending request", 0);
        st.approve_grant(DAEMON, &req.id, user_code.expose(), "Mia", None)
            .unwrap();
        bumps(&st, "a request approved into a person", 1);
        let mia = st.viewer_of_grant(&req.id).unwrap().public_id;
        // Approvals.
        let a = st
            .approve(DAEMON, Some(&sam), &mia, Scope::Everywhere, Via::Shell)
            .unwrap();
        bumps(&st, "an approval", 1);
        st.approve(DAEMON, Some(&sam), &mia, Scope::Everywhere, Via::Shell)
            .unwrap();
        bumps(&st, "finding the live approval", 0);
        st.approve(DAEMON, Some(&sam), &sam, Scope::Everywhere, Via::Shell)
            .unwrap_err();
        bumps(&st, "a refused approval", 0);
        st.revoke_approval(DAEMON, &a.id, Some(&sam)).unwrap();
        bumps(&st, "an approval revoked", 1);
        // Rights and narrowing.
        let reader = clax_core::rights::Rights::preset("clax:reader").unwrap();
        st.set_person_rights(DAEMON, &sam, None).unwrap();
        bumps(&st, "a person's rights set to what they were", 0);
        st.set_person_rights(DAEMON, &sam, Some(reader.clone()))
            .unwrap();
        bumps(&st, "a person's rights", 1);
        st.set_person_rights(DAEMON, &sam, Some(reader.clone()))
            .unwrap();
        bumps(&st, "a person's rights set again", 0);
        st.narrow_grant(DAEMON, &gid, None).unwrap();
        bumps(&st, "a grant's narrowing set to what it was", 0);
        st.narrow_grant(DAEMON, &gid, Some(reader.clone())).unwrap();
        bumps(&st, "a grant's narrowing", 1);
        st.narrow_grant(DAEMON, &gid, Some(reader.clone())).unwrap();
        bumps(&st, "a grant's narrowing set again", 0);
        // One agent's narrowing and elevation, and its end clearing them.
        let agent = st
            .register_session(
                DAEMON,
                crate::sessions::RegisterSession {
                    harness: "claude".into(),
                    cwd: "/w".into(),
                    grant_id: Some(gid.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        bumps(&st, "an agent registering", 0);
        let full = clax_core::rights::Rights::full();
        st.narrow_agent(DAEMON, &agent.id, Some(full.clone()))
            .unwrap_err();
        bumps(&st, "a refused agent narrowing", 0);
        st.narrow_agent(DAEMON, &agent.id, Some(reader.clone()))
            .unwrap();
        bumps(&st, "an agent's narrowing", 1);
        st.narrow_agent(DAEMON, &agent.id, Some(reader)).unwrap();
        bumps(&st, "an agent's narrowing set again", 0);
        st.elevate_agent(DAEMON, &agent.id, None, Some(full.clone()))
            .unwrap();
        bumps(&st, "an agent's elevation", 1);
        st.elevate_agent(DAEMON, &agent.id, None, Some(full))
            .unwrap();
        bumps(&st, "an agent's elevation set again", 0);
        st.end_session_by_person(DAEMON, &agent.id).unwrap();
        bumps(
            &st,
            "an agent's end clearing its narrowing and elevation",
            1,
        );
        // A second invite for someone already a person changes nothing.
        st.create_invite(DAEMON, InvitePerson::Viewer(sam.clone()), None)
            .unwrap();
        bumps(&st, "a second invite for a person", 0);
        // Grants ending, each way.
        st.revoke_token(DAEMON, &gid, manage.expose()).unwrap();
        bumps(&st, "a grant's token revoked", 1);
        st.revoke_grant(DAEMON, &gid, crate::people::EndedBy::Owner)
            .unwrap();
        bumps(&st, "an ended grant revoked again", 0);
        st.revoke_grant(DAEMON, &req.id, crate::people::EndedBy::Owner)
            .unwrap();
        bumps(&st, "a grant revoked", 1);
        st.expire_grants().unwrap();
        bumps(&st, "an expiry with nothing to expire", 0);
        // A person revoked.
        st.revoke_person(DAEMON, &sam).unwrap();
        bumps(&st, "a person revoked", 1);
        // A viewer claim folds authorship.
        let cookie = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        st.mint_viewer(cookie, true).unwrap();
        st.owner_viewer(true).unwrap();
        st.claim_for_owner(ClaimVia::Shell, cookie).unwrap();
        bumps(&st, "a viewer claim", 1);
        st.claim_for_owner(ClaimVia::Shell, cookie).unwrap();
        bumps(&st, "a claim claiming nothing", 0);
        let heard = heard.lock().unwrap().clone();
        assert_eq!(
            heard,
            (1..=g(&st)).collect::<Vec<_>>(),
            "the listener hears each, in order"
        );
    }

    #[test]
    fn the_owners_fast_path_follows_people_and_the_switch() {
        let (_d, _c, st) = store();
        assert!(st.owner_sees_all().unwrap());
        assert!(st.owner_sees_all().unwrap(), "cached");
        let (invite, _) = st
            .create_invite(DAEMON, InvitePerson::New("Sam".into()), None)
            .unwrap();
        assert!(st.people_present().unwrap());
        assert!(!st.owner_sees_all().unwrap(), "a person is present");
        st.revoke_invite(DAEMON, &invite.id).unwrap();
        assert!(!st.people_present().unwrap());
        assert!(st.owner_sees_all().unwrap(), "back to a plain viewer");
        let id = st.live_approvals(None).unwrap()[0].id.clone();
        st.revoke_approval(DAEMON, &id, None).unwrap();
        assert!(!st.owner_sees_all().unwrap(), "the switch is off");
        st.approve(DAEMON, None, UNCONNECTED, Scope::Everywhere, Via::Shell)
            .unwrap();
        assert!(st.owner_sees_all().unwrap(), "the switch is on again");
    }

    /// Sam and Mia connected; Sam's agent's session watching an artifact
    /// the owner published, where Mia comments.
    struct Req {
        _dir: tempfile::TempDir,
        clock: Arc<clax_core::working::ManualClock>,
        st: Store,
        sam: String,
        mia: String,
        lan: String,
        aid: clax_core::ArtifactId,
        /// Sam's agent's session.
        sid: String,
        /// The owner's agent's session.
        owner_sid: String,
    }

    fn req_world() -> Req {
        let (dir, clock, st) = store();
        st.owner_viewer(true).unwrap();
        let (sam_grant, _, _) = approved(&st, "Sam", [1; 32]);
        let sam = st.viewer_of_grant(&sam_grant).unwrap().public_id;
        let (mia_grant, _, _) = approved(&st, "Mia", [2; 32]);
        let mia = st.viewer_of_grant(&mia_grant).unwrap().public_id;
        let lan = lan(&st, "01ARZ3NDEKTSV4RRFFQ69G5FAV", "Lan");
        let register = |grant: Option<String>, h: &str| {
            st.register_session(
                DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some(h.into()),
                    cwd: "/w".into(),
                    grant_id: grant,
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        };
        let sid = register(Some(sam_grant), "sam-h1");
        let owner_sid = register(None, "owner-h1");
        let aid = artifact(&st, None);
        st.ensure_watch(DAEMON, &sid, &aid).unwrap();
        Req {
            _dir: dir,
            clock,
            st,
            sam,
            mia,
            lan,
            aid,
            sid,
            owner_sid,
        }
    }

    impl Req {
        /// A thread `by` (a viewer's public ID) starts on the artifact.
        fn thread_by(&self, by: &str, body: &str) -> String {
            self.st
                .create_thread(
                    DAEMON,
                    &self.aid,
                    NewThread {
                        version_n: 1,
                        anchor: anchor(),
                        author_name: "x".into(),
                        author_public_id: Some(by.to_string()),
                        body: body.into(),
                        clip: None,
                        via_page: false,
                    },
                )
                .unwrap()
                .id
        }

        fn comment_by(&self, tid: &str, by: &str, body: &str) {
            self.st
                .add_comment(
                    DAEMON,
                    tid,
                    crate::threads::NewComment {
                        author_public_id: Some(by.to_string()),
                        author_kind: crate::threads::AUTHOR_VIEWER,
                        author_name: "x".into(),
                        via_session_id: None,
                        body: body.into(),
                        via_page: false,
                    },
                )
                .unwrap();
        }

        /// Sends `tid` to Sam's agent alone.
        fn send_to_sam(&self, tid: &str) -> Vec<String> {
            self.st
                .send_routed(
                    DAEMON,
                    tid,
                    crate::SendTarget::Agent(&self.sid),
                    Route::Send,
                    false,
                )
                .unwrap()
                .2
        }

        fn open(&self, approver: Option<&str>) -> Vec<ApprovalRequest> {
            self.st.approval_requests(approver, true, 500).unwrap().0
        }

        fn unread_approvals(&self, approver: Option<&str>) -> usize {
            let q = crate::inbox::InboxQuery {
                recipient: approver.map(str::to_string),
                kinds: vec![crate::inbox::Kind::Approval],
                read: crate::inbox::ReadFilter::Unread,
                ..Default::default()
            };
            self.st.inbox_count(&q, 1000).unwrap() as usize
        }

        fn kinds_since(&self, seq: i64) -> Vec<String> {
            self.st
                .events_after(seq, 10_000)
                .unwrap()
                .into_iter()
                .map(|r| r.kind)
                .filter(|k| k.starts_with("approval."))
                .collect()
        }

        fn last_seq(&self) -> i64 {
            self.st
                .events_after(0, 100_000)
                .unwrap()
                .last()
                .unwrap()
                .seq
        }
    }

    #[test]
    fn routing_makes_one_request_per_author_and_joins_it_after() {
        let w = req_world();
        let tid = w.thread_by(&w.mia, "please fix the chart");
        w.comment_by(&tid, &w.lan, "and the colours");
        let before = w.last_seq();
        let made = w.send_to_sam(&tid);
        assert_eq!(made.len(), 2, "Mia and Lan: {made:?}");
        let open = w.open(Some(&w.sam));
        let mia = open.iter().find(|r| r.author == w.mia).unwrap();
        assert_eq!(
            (
                mia.thread_id.as_deref(),
                mia.artifact_id.as_str(),
                mia.cause,
                mia.count,
                mia.session_id.as_deref()
            ),
            (
                Some(tid.as_str()),
                w.aid.as_str(),
                Cause::Routing,
                1,
                Some(w.sid.as_str())
            )
        );
        assert_eq!(w.unread_approvals(Some(&w.sam)), 2);
        assert!(
            w.open(None).is_empty(),
            "the owner's agents were not sent it"
        );
        // Sending again joins: nothing new, nothing re-announced.
        let again = w.send_to_sam(&tid);
        assert_eq!(again.len(), 2);
        assert_eq!(w.open(Some(&w.sam)).len(), 2);
        // Open requests' items stay unread whoever marks them (spec §9.5).
        let q = crate::inbox::InboxQuery {
            recipient: Some(w.sam.clone()),
            ..Default::default()
        };
        w.st.inbox_mark_all(&q).unwrap();
        assert_eq!(w.unread_approvals(Some(&w.sam)), 2);
        // Mia's later reply on the sent thread counts into her request and
        // re-announces its item: one read before that rule is unread again.
        w.st.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE inbox_items SET read_at = 'x' WHERE kind = 'approval'",
                [],
            )?)
        })
        .unwrap();
        assert_eq!(w.unread_approvals(Some(&w.sam)), 0);
        w.comment_by(&tid, &w.mia, "and the axis");
        w.st.send_to_agent(DAEMON, &tid).unwrap();
        let open = w.open(Some(&w.sam));
        assert_eq!(open.len(), 2);
        assert_eq!(open.iter().find(|r| r.author == w.mia).unwrap().count, 2);
        assert_eq!(w.unread_approvals(Some(&w.sam)), 1, "Mia's re-announced");
        assert_eq!(
            w.kinds_since(before),
            ["approval.request", "approval.request", "approval.request"],
            "made twice, joined with more once"
        );
        let rows = w.st.events_after(before, 10_000).unwrap();
        let send = rows.iter().find(|r| r.kind == "thread.send").unwrap();
        let body: serde_json::Value = serde_json::from_str(&send.body).unwrap();
        assert_eq!(body["approval_request_ids"].as_array().unwrap().len(), 2);
        // Another thread of Mia's is a request of its own.
        let t2 = w.thread_by(&w.mia, "another");
        w.send_to_sam(&t2);
        assert_eq!(w.open(Some(&w.sam)).len(), 3);
    }

    #[test]
    fn a_later_comment_alone_never_makes_a_request() {
        let w = req_world();
        let tid = w.thread_by(&w.sam, "Sam's own");
        assert!(w.send_to_sam(&tid).is_empty(), "nothing withheld");
        w.comment_by(&tid, &w.mia, "Mia joins in");
        let (_, _, made) =
            w.st.send_routed(DAEMON, &tid, crate::SendTarget::Thread, Route::Join, false)
                .unwrap();
        assert!(made.is_empty() && w.open(Some(&w.sam)).is_empty());
    }

    #[test]
    fn a_routed_row_reads_withheld_before_any_take() {
        let w = req_world();
        let tid = w.thread_by(&w.mia, "please");
        w.send_to_sam(&tid);
        let s = w.st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(s.state, clax_core::FeedbackPhase::Withheld);
        assert_eq!(s.approver.as_deref(), Some("Sam"));
        // Declined: still held, waiting for nothing; by Sam, to the owner,
        // Sam and Mia; to Sam's agent, its plain "sent" (no take yet); to a
        // visitor, nothing.
        let rid = w.open(Some(&w.sam))[0].id.clone();
        w.st.decide_request(DAEMON, &rid, Some(&w.sam), Decision::Decline, Via::Shell)
            .unwrap();
        let s = w.st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(s.state, clax_core::FeedbackPhase::Declined);
        assert_eq!(s.approver.as_deref(), Some("Sam"));
        let d =
            w.st.deliveries(std::slice::from_ref(&tid), false)
                .unwrap()
                .remove(&tid)
                .unwrap();
        use crate::DeliveryViewer as V;
        let agent = V::Agent {
            session: Some(w.sid.clone()),
            every: false,
        };
        let a = d.view(&agent).unwrap();
        assert_eq!(a.state, clax_core::FeedbackPhase::Sent);
        assert!(a.approver.is_none());
        let sam = d.view(&V::Person(w.sam.clone())).unwrap();
        assert!(sam.mine && sam.state == clax_core::FeedbackPhase::Declined);
        let mia = d.view(&V::Person(w.mia.clone())).unwrap();
        assert!(!mia.mine && mia.approver.as_deref() == Some("Sam"));
        assert_eq!(d.view(&V::Visitor), None);
    }

    #[test]
    fn only_the_approver_decides_and_only_once() {
        let w = req_world();
        let tid = w.thread_by(&w.mia, "please");
        w.send_to_sam(&tid);
        let rid = w.open(Some(&w.sam))[0].id.clone();
        let decide = |approver: Option<&str>, d: Decision| {
            w.st.decide_request(DAEMON, &rid, approver, d, Via::Shell)
        };
        let everywhere = Decision::Approve(ScopeKind::Everywhere);
        assert!(matches!(decide(None, everywhere), Err(CoreError::NotFound)));
        assert!(matches!(
            decide(Some(&w.mia), everywhere),
            Err(CoreError::NotFound)
        ));
        let d = decide(Some(&w.sam), Decision::Approve(ScopeKind::Thread)).unwrap();
        assert_eq!(d.request.status, RequestStatus::Approved);
        assert_eq!(d.request.decided_scope, Some(ScopeKind::Thread));
        let a = d.approval.unwrap();
        assert_eq!(
            (a.scope, a.thread_id.as_deref(), a.request_id.as_deref()),
            (ScopeKind::Thread, Some(tid.as_str()), Some(rid.as_str()))
        );
        assert_eq!(
            code(decide(Some(&w.sam), Decision::Decline)),
            "request_closed"
        );
        assert_eq!(w.unread_approvals(Some(&w.sam)), 0, "decided: read");
    }

    #[test]
    fn approving_everywhere_closes_the_authors_other_requests() {
        let w = req_world();
        let t1 = w.thread_by(&w.mia, "one");
        let t2 = w.thread_by(&w.mia, "two");
        let t3 = w.thread_by(&w.lan, "three");
        for t in [&t1, &t2, &t3] {
            w.send_to_sam(t);
        }
        let other = w.st.create_thread(
            DAEMON,
            &artifact(&w.st, None),
            NewThread {
                version_n: 1,
                anchor: anchor(),
                author_name: "x".into(),
                author_public_id: Some(w.mia.clone()),
                body: "elsewhere".into(),
                clip: None,
                via_page: false,
            },
        );
        let other = other.unwrap();
        w.st.ensure_watch(
            DAEMON,
            &w.sid,
            &clax_core::ArtifactId::parse(&other.artifact_id).unwrap(),
        )
        .unwrap();
        w.send_to_sam(&other.id);
        let rid = |tid: &str| {
            w.open(Some(&w.sam))
                .into_iter()
                .find(|r| r.thread_id.as_deref() == Some(tid))
                .unwrap()
                .id
        };
        // A thread approval closes that thread's request alone.
        let r1 = rid(&t1);
        w.st.decide_request(
            DAEMON,
            &r1,
            Some(&w.sam),
            Decision::Approve(ScopeKind::Artifact),
            Via::Shell,
        )
        .unwrap();
        let open: Vec<_> = w
            .open(Some(&w.sam))
            .into_iter()
            .map(|r| r.thread_id.unwrap())
            .collect();
        assert!(
            !open.contains(&t1) && !open.contains(&t2),
            "the artifact covers both: {open:?}"
        );
        assert!(open.contains(&t3) && open.contains(&other.id), "{open:?}");
        let before = w.last_seq();
        let r = rid(&other.id);
        w.st.decide_request(
            DAEMON,
            &r,
            Some(&w.sam),
            Decision::Approve(ScopeKind::Everywhere),
            Via::Shell,
        )
        .unwrap();
        let open: Vec<_> = w.open(Some(&w.sam)).into_iter().map(|r| r.author).collect();
        assert_eq!(open, std::slice::from_ref(&w.lan), "Lan's stays open");
        assert_eq!(
            w.kinds_since(before),
            ["approval.grant", "approval.decide"],
            "{:?}",
            w.kinds_since(before)
        );
        // An approval made directly closes what it covers too.
        w.st.approve(
            DAEMON,
            Some(&w.sam),
            &w.lan,
            Scope::Thread(t3.clone()),
            Via::Shell,
        )
        .unwrap();
        assert!(w.open(Some(&w.sam)).is_empty());
        assert_eq!(w.unread_approvals(Some(&w.sam)), 0);
    }

    #[test]
    fn decline_keeps_rows_held_and_asks_again_only_after_ten_minutes() {
        let w = req_world();
        let tid = w.thread_by(&w.mia, "please");
        w.send_to_sam(&tid);
        let rid = w.open(Some(&w.sam))[0].id.clone();
        let g = w.st.visibility_generation();
        let before = w.last_seq();
        let d =
            w.st.decide_request(DAEMON, &rid, Some(&w.sam), Decision::Decline, Via::Shell)
                .unwrap();
        assert_eq!(d.request.status, RequestStatus::Declined);
        assert!(d.approval.is_none() && d.touched.targets.is_empty());
        // Its thread's state is announced again: it waits for nothing now.
        assert_eq!(d.touched.threads.len(), 1);
        assert!(d.touched.threads.iter().all(|(_, t)| *t == tid));
        assert_eq!(w.st.visibility_generation(), g + 1, "a decision bumps");
        assert_eq!(w.kinds_since(before), ["approval.decide"]);
        let body: serde_json::Value =
            serde_json::from_str(&w.st.events_after(before, 10).unwrap()[0].body).unwrap();
        assert_eq!(body["scope"], "decline");
        // Held: a take hands nothing over.
        let vis = Visibility::for_session(&w.st, &w.sid).unwrap();
        let q = crate::feedback::TakeFeedback {
            session_id: w.sid.clone(),
            tier: clax_core::feedback::Tier::Piggyback,
            artifact_id: None,
            include_resends: false,
        };
        let t = w.st.take_feedback(DAEMON, &q, "http://h", &vis).unwrap();
        assert!(t.items.is_empty());
        assert!(!t.placeholders[0].asked, "declined: not asked");
        let ask = AskApproval {
            session_id: w.sid.clone(),
            thread_id: Some(tid.clone()),
            ..Default::default()
        };
        // Within ten minutes: neither ask nor routing asks again.
        w.clock.advance(DECLINE_QUIET_SECS - 1);
        assert_eq!(code(w.st.ask_approval(DAEMON, &ask)), "recently_declined");
        assert!(w.send_to_sam(&tid).is_empty());
        assert!(w.open(Some(&w.sam)).is_empty());
        w.clock.advance(2);
        let asked = w.st.ask_approval(DAEMON, &ask).unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            (asked[0].cause, asked[0].status),
            (Cause::Ask, RequestStatus::Open)
        );
        assert_ne!(asked[0].id, rid);
    }

    #[test]
    fn a_decline_ends_for_the_request_state_once_an_approval_is_made_or_revoked_after_it() {
        let w = req_world();
        let tid = w.thread_by(&w.mia, "please");
        w.send_to_sam(&tid);
        let rid = w.open(Some(&w.sam))[0].id.clone();
        w.st.decide_request(DAEMON, &rid, Some(&w.sam), Decision::Decline, Via::Shell)
            .unwrap();
        let state = || {
            w.st.request_state(
                Some(&w.sam),
                w.aid.as_str(),
                &tid,
                std::slice::from_ref(&w.mia),
            )
            .unwrap()
        };
        assert_eq!(state(), ApprovalState::Declined);
        w.clock.advance(1);
        let a =
            w.st.approve(
                DAEMON,
                Some(&w.sam),
                &w.mia,
                Scope::Thread(tid.clone()),
                Via::Shell,
            )
            .unwrap();
        assert_eq!(state(), ApprovalState::None, "approved since");
        w.clock.advance(1);
        w.st.revoke_approval(DAEMON, &a.id, Some(&w.sam)).unwrap();
        assert_eq!(
            state(),
            ApprovalState::None,
            "revoked since: no decline in force"
        );
    }

    #[test]
    fn ask_requests_what_is_withheld_there() {
        let w = req_world();
        let tid = w.thread_by(&w.sam, "Sam's own");
        let ask = |thread: Option<&str>, artifact: Option<&str>, author: Option<&str>| {
            w.st.ask_approval(
                DAEMON,
                &AskApproval {
                    session_id: w.sid.clone(),
                    artifact_id: artifact.map(str::to_string),
                    thread_id: thread.map(str::to_string),
                    author: author.map(str::to_string),
                },
            )
        };
        assert_eq!(code(ask(Some(&tid), None, None)), "nothing_withheld");
        assert_eq!(code(ask(None, None, None)), "invalid_args");
        assert!(matches!(
            ask(Some("01ARZ3NDEKTSV4RRFFQ69G5FAZ"), None, None),
            Err(CoreError::NotFound)
        ));
        w.comment_by(&tid, &w.mia, "Mia");
        w.comment_by(&tid, &w.lan, "Lan");
        assert_eq!(
            code(ask(Some(&tid), None, Some(&w.sam))),
            "nothing_withheld"
        );
        let only = ask(Some(&tid), Some(w.aid.as_str()), Some(&w.mia)).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(
            (only[0].author.as_str(), only[0].count),
            (w.mia.as_str(), 1)
        );
        let both = ask(Some(&tid), None, None).unwrap();
        assert_eq!(both.len(), 2);
        assert_eq!(both[0].id, only[0].id, "joined");
        // Outside threads: the owner's artifact holds nothing withheld from
        // Sam... until he has not approved the owner.
        let outside = ask(None, Some(w.aid.as_str()), None).unwrap();
        assert_eq!(outside[0].thread_id, None);
        assert_eq!(
            outside[0].author,
            w.st.owner_viewer(true).unwrap().public_id
        );
        // A thread of another artifact than the one named is no thread.
        assert!(matches!(
            ask(Some(&tid), Some(artifact(&w.st, None).as_str()), None),
            Err(CoreError::NotFound)
        ));
    }

    /// `n` LAN viewers who are not connected, by public ID.
    fn lans(st: &Store, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                lan(
                    st,
                    &format!("01ARZ3NDEKTSV4RRFFQ69G5F{:02}", i + 10),
                    &format!("Lan{i}"),
                )
            })
            .collect()
    }

    #[test]
    fn the_hundred_and_first_request_is_refused_per_approver() {
        let w = req_world();
        // Five authors, each at their own share of Sam's requests.
        let authors = lans(&w.st, (MAX_OPEN_REQUESTS / MAX_OPEN_PER_AUTHOR) as usize);
        let mut tids = Vec::new();
        for a in &authors {
            for i in 0..MAX_OPEN_PER_AUTHOR {
                let t = w.thread_by(a, &format!("t{i}"));
                assert_eq!(w.send_to_sam(&t).len(), 1);
                tids.push(t);
            }
        }
        assert_eq!(
            w.st.approval_requests(Some(&w.sam), true, 1).unwrap().1,
            MAX_OPEN_REQUESTS
        );
        assert!(!w.st.approval_requests(Some(&w.sam), true, 1).unwrap().2);
        // Routing holds without a request, and says so.
        let t = w.thread_by(&w.mia, "one more");
        assert!(w.send_to_sam(&t).is_empty());
        let (_, n, overflow) = w.st.approval_requests(Some(&w.sam), true, 1).unwrap();
        assert_eq!((n, overflow), (MAX_OPEN_REQUESTS, true));
        let ask = AskApproval {
            session_id: w.sid.clone(),
            thread_id: Some(t.clone()),
            ..Default::default()
        };
        assert_eq!(code(w.st.ask_approval(DAEMON, &ask)), "limit_reached");
        // A request already open is still joined.
        w.comment_by(&tids[0], &authors[0], "more");
        w.st.send_to_agent(DAEMON, &tids[0]).unwrap();
        // The owner's own limit is the owner's.
        w.st.ensure_watch(DAEMON, &w.owner_sid, &w.aid).unwrap();
        // (Another thread of Mia's: sending Sam's to the owner's agent would
        // displace Sam's agent from it, spec 2026-10-08 §12.)
        let t2 = w.thread_by(&w.mia, "for the owner");
        let made =
            w.st.send_routed(
                DAEMON,
                &t2,
                crate::SendTarget::Agent(&w.owner_sid),
                Route::Send,
                false,
            )
            .unwrap()
            .2;
        assert_eq!(made.len(), 1, "another approver is not full");
        assert_eq!(w.open(None)[0].approver, None);
        // A decision frees a place, but Mia's held thread still has no
        // request: the banner stays.
        let rid = w.open(Some(&w.sam))[0].id.clone();
        w.st.decide_request(DAEMON, &rid, Some(&w.sam), Decision::Decline, Via::Shell)
            .unwrap();
        assert!(w.st.approval_requests(Some(&w.sam), true, 1).unwrap().2);
        // Once Mia's content is no longer held without a request (Sam
        // allows her), the next decision clears it.
        w.st.approve(
            DAEMON,
            Some(&w.sam),
            &w.mia,
            Scope::Thread(t.clone()),
            Via::Shell,
        )
        .unwrap();
        let rid = w.open(Some(&w.sam))[0].id.clone();
        w.st.decide_request(DAEMON, &rid, Some(&w.sam), Decision::Decline, Via::Shell)
            .unwrap();
        assert!(!w.st.approval_requests(Some(&w.sam), true, 1).unwrap().2);
    }

    #[test]
    fn one_author_cannot_spend_an_approvers_requests() {
        let w = req_world();
        for i in 0..MAX_OPEN_PER_AUTHOR {
            let t = w.thread_by(&w.mia, &format!("t{i}"));
            assert_eq!(w.send_to_sam(&t).len(), 1);
        }
        // Mia's twenty-first is held silently; it counts toward the banner.
        let held = w.thread_by(&w.mia, "one more of Mia's");
        assert!(w.send_to_sam(&held).is_empty());
        let (_, n, overflow) = w.st.approval_requests(Some(&w.sam), true, 1).unwrap();
        assert_eq!((n, overflow), (MAX_OPEN_PER_AUTHOR, true));
        // Another author is still asked for.
        let lans = w.thread_by(&w.lan, "Lan's");
        assert_eq!(w.send_to_sam(&lans).len(), 1);
        // A decision on one of Mia's leaves her held thread without a
        // request: the banner stays.
        let rid = w
            .open(Some(&w.sam))
            .into_iter()
            .find(|r| r.author == w.mia)
            .unwrap()
            .id;
        w.st.decide_request(DAEMON, &rid, Some(&w.sam), Decision::Decline, Via::Shell)
            .unwrap();
        assert!(w.st.approval_requests(Some(&w.sam), true, 1).unwrap().2);
    }

    #[test]
    fn only_a_send_naming_an_agent_asks() {
        let w = req_world();
        let t = w.thread_by(&w.mia, "untargeted");
        let (_, _, made) =
            w.st.send_routed(DAEMON, &t, crate::SendTarget::Everyone, Route::Send, false)
                .unwrap();
        assert!(made.is_empty() && w.open(Some(&w.sam)).is_empty());
        // A batch naming no agent asks no one either; one naming Sam's does.
        let t2 = w.thread_by(&w.mia, "batched");
        let batch = |to: Option<String>, tid: &str| {
            w.st.send_batch(
                DAEMON,
                &w.aid,
                crate::batches::SendBatch {
                    thread_ids: vec![tid.to_string()],
                    note: None,
                    sent_by: "Mia".into(),
                    sender: w.mia.clone(),
                    to,
                },
            )
            .unwrap()
        };
        batch(None, &t2);
        assert!(w.open(Some(&w.sam)).is_empty());
        let t3 = w.thread_by(&w.mia, "batched to Sam's");
        batch(Some(w.sid.clone()), &t3);
        assert_eq!(w.open(Some(&w.sam)).len(), 1);
        // A send naming Sam's agent asks.
        assert_eq!(w.send_to_sam(&t).len(), 1);
    }

    #[test]
    fn an_owner_page_comment_reads_withheld_while_its_request_is_open() {
        let w = req_world();
        let owner = w.st.owner_viewer(true).unwrap().public_id;
        // Sam allows Mia; the owner's page wrote her comment.
        w.st.approve(DAEMON, Some(&w.sam), &w.mia, Scope::Everywhere, Via::Shell)
            .unwrap();
        let tid = w.thread_by(&w.mia, "the owner's page asks");
        w.st.with_tx(|tx| {
            tx.execute(
                "UPDATE comments SET via_page = 1 WHERE thread_id = ?1",
                params![tid],
            )?;
            let cid: String = tx.query_row(
                "SELECT id FROM comments WHERE thread_id = ?1",
                params![tid],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO page_authors (kind, id, author) VALUES ('comment', ?1, '*owner')",
                params![cid],
            )?;
            Ok(())
        })
        .unwrap();
        let made = w.send_to_sam(&tid);
        assert_eq!(made.len(), 1);
        assert_eq!(w.open(Some(&w.sam))[0].author, owner);
        let s = w.st.feedback_state(&tid, false).unwrap().unwrap();
        assert_eq!(s.state, clax_core::FeedbackPhase::Withheld);
    }

    #[test]
    fn requests_are_withdrawn_when_their_cause_goes_away() {
        let w = req_world();
        let status = |rid: &str| w.st.approval_request(rid, Some(&w.sam)).unwrap().status;
        let routed = |body: &str| {
            let t = w.thread_by(&w.mia, body);
            w.send_to_sam(&t);
            let r = w
                .open(Some(&w.sam))
                .into_iter()
                .find(|r| r.thread_id.as_deref() == Some(t.as_str()))
                .unwrap();
            (t, r.id)
        };
        let before = w.last_seq();
        let (t1, r1) = routed("resolved");
        w.st.resolve_thread(DAEMON, &t1, "viewer:x").unwrap();
        assert_eq!(status(&r1), RequestStatus::Withdrawn);
        let (t2, r2) = routed("deleted");
        w.st.delete_thread(DAEMON, &t2).unwrap();
        assert_eq!(status(&r2), RequestStatus::Withdrawn);
        // The asking session's end withdraws what it asked, not what was routed.
        let (_, r3) = routed("routed");
        let t4 = w.thread_by(&w.sam, "Sam's");
        w.comment_by(&t4, &w.mia, "Mia");
        let r4 =
            w.st.ask_approval(
                DAEMON,
                &AskApproval {
                    session_id: w.sid.clone(),
                    thread_id: Some(t4.clone()),
                    ..Default::default()
                },
            )
            .unwrap()[0]
                .id
                .clone();
        w.st.end_session(DAEMON, &w.sid).unwrap();
        assert_eq!(status(&r4), RequestStatus::Withdrawn);
        assert_eq!(status(&r3), RequestStatus::Open);
        // The author revoked: nothing is left to ask.
        w.st.revoke_person(DAEMON, &w.mia).unwrap();
        assert_eq!(status(&r3), RequestStatus::Withdrawn);
        assert_eq!(w.unread_approvals(Some(&w.sam)), 0);
        let rows = w.st.events_after(before, 10_000).unwrap();
        let reasons: Vec<String> = rows
            .iter()
            .filter(|r| r.kind == "approval.withdraw")
            .map(|r| {
                serde_json::from_str::<serde_json::Value>(&r.body).unwrap()["reason"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            reasons,
            [
                "thread_resolved",
                "thread_deleted",
                "session_end",
                "person_revoked"
            ]
        );
    }

    #[test]
    fn a_deleted_artifact_withdraws_its_requests() {
        let w = req_world();
        let t = w.thread_by(&w.mia, "x");
        w.send_to_sam(&t);
        let rid = w.open(Some(&w.sam))[0].id.clone();
        w.st.delete_artifact(DAEMON, &w.aid).unwrap();
        assert_eq!(
            w.st.approval_request(&rid, Some(&w.sam)).unwrap().status,
            RequestStatus::Withdrawn
        );
    }

    #[test]
    fn an_approval_releases_exactly_the_rows_it_lets_through() {
        let w = req_world();
        // A page of Mia's: a comment its page wrote in the owner's browser
        // is the owner's and Mia's.
        let owner = w.st.owner_viewer(true).unwrap().public_id;
        let tid = w.thread_by(&w.mia, "Mia's");
        w.st.with_tx(|tx| {
            tx.execute(
                "UPDATE comments SET author_public_id = ?2, via_page = 1 WHERE thread_id = ?1",
                params![tid, owner],
            )?;
            let cid: String = tx.query_row(
                "SELECT id FROM comments WHERE thread_id = ?1",
                params![tid],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO page_authors (kind, id, author) VALUES ('comment', ?1, ?2)",
                params![cid, w.mia],
            )?;
            Ok(())
        })
        .unwrap();
        w.send_to_sam(&tid);
        let vis = || Visibility::for_session(&w.st, &w.sid).unwrap();
        let q = crate::feedback::TakeFeedback {
            session_id: w.sid.clone(),
            tier: clax_core::feedback::Tier::Piggyback,
            artifact_id: None,
            include_resends: false,
        };
        let told = || {
            w.st.with_read(|c| {
                Ok(c.query_row(
                    "SELECT placeholder_at IS NOT NULL FROM feedback WHERE thread_id = ?1",
                    params![tid],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .unwrap()
        };
        let t = w.st.take_feedback(DAEMON, &q, "http://h", &vis()).unwrap();
        assert_eq!(t.placeholders.len(), 1);
        assert!(told());
        // Approving the owner alone leaves the row held by Mia: still told.
        let (_, touched) =
            w.st.approve_touched(DAEMON, Some(&w.sam), &owner, Scope::Everywhere, Via::Shell)
                .unwrap();
        assert!(told(), "Mia still withholds it");
        assert!(touched.is_empty(), "nothing let through: {touched:?}");
        let t = w.st.take_feedback(DAEMON, &q, "http://h", &vis()).unwrap();
        assert!(t.placeholders.is_empty() && t.items.is_empty(), "told once");
        // Approving Mia lets it through.
        let (_, touched) =
            w.st.approve_touched(
                DAEMON,
                Some(&w.sam),
                &w.mia,
                Scope::Thread(tid.clone()),
                Via::Shell,
            )
            .unwrap();
        assert!(!told());
        assert!(touched.targets.contains(&w.sid), "{touched:?}");
        let t = w.st.take_feedback(DAEMON, &q, "http://h", &vis()).unwrap();
        assert_eq!(t.items.len(), 1);
    }

    #[test]
    fn a_revoked_approval_tells_its_rows_once_more() {
        let w = req_world();
        let a =
            w.st.approve(DAEMON, Some(&w.sam), &w.mia, Scope::Everywhere, Via::Shell)
                .unwrap();
        let tid = w.thread_by(&w.mia, "visible");
        w.send_to_sam(&tid);
        w.st.revoke_approval(DAEMON, &a.id, Some(&w.sam)).unwrap();
        let vis = Visibility::for_session(&w.st, &w.sid).unwrap();
        let q = crate::feedback::TakeFeedback {
            session_id: w.sid.clone(),
            tier: clax_core::feedback::Tier::Piggyback,
            artifact_id: None,
            include_resends: false,
        };
        let t = w.st.take_feedback(DAEMON, &q, "http://h", &vis).unwrap();
        assert!(t.items.is_empty());
        assert_eq!(t.placeholders.len(), 1, "held again and told");
    }

    /// The request state a take reads per held thread, and the declines a
    /// delivery state reads, each go through an index: a busy pair's
    /// piled-up requests (withdrawn by resolves, approved, declined) are
    /// never walked. Planned with and without statistics.
    #[test]
    fn request_reads_are_indexed_whatever_the_pile() {
        let (_d, _clock, st) = store();
        st.with_write(|c| {
            c.execute_batch("BEGIN")?;
            for i in 0..2000 {
                let status = ["withdrawn", "approved", "declined", "open"][i % 4];
                c.execute(
                    "INSERT INTO approval_requests (id, approver, author, artifact_id, thread_id, cause,
                         status, created_at, updated_at, decided_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'routing', ?6, 'x', 'x', 'x')",
                    rusqlite::params![
                        format!("r{i}"),
                        format!("u_{:022}", i % 3),
                        format!("u_{:022}", 10 + i % 5),
                        format!("a{}", i % 4),
                        // Open requests are unique per (approver, author,
                        // artifact, thread).
                        format!("t{i}"),
                        status
                    ],
                )?;
            }
            c.execute_batch("COMMIT")?;
            Ok(())
        })
        .unwrap();
        let plan =
            |c: &rusqlite::Connection, sql: &str, args: &[&dyn rusqlite::ToSql]| -> Vec<String> {
                let mut stmt = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
                stmt.query_map(args, |r| r.get::<_, String>(3))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            };
        st.with_write(|c| {
            for stats in [
                "DELETE FROM sqlite_stat1; ANALYZE sqlite_schema;",
                "ANALYZE",
            ] {
                c.execute_batch(stats)?;
                let authors = format!("[\"u_{:022}\"]", 10);
                for (sql, args, index) in [
                    (
                        REQUEST_STATE,
                        vec![
                            Box::new(format!("u_{:022}", 0)) as Box<dyn rusqlite::ToSql>,
                            Box::new("a0"),
                            Box::new("t0"),
                            Box::new(authors.clone()),
                        ],
                        // By target, or (with statistics) by thread: the
                        // thread or the whole artifact.
                        &["approval_requests_by_target", "approval_requests_by_thread"][..],
                    ),
                    (
                        crate::feedback::DECLINES,
                        vec![Box::new("[\"t0\",\"t1\"]") as Box<dyn rusqlite::ToSql>],
                        &["approval_requests_by_thread"][..],
                    ),
                ] {
                    let args: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
                    let plan = plan(c, sql, &args);
                    assert!(
                        plan.iter().any(|l| index
                            .iter()
                            .any(|i| l.contains(&format!("USING INDEX {i} ")))),
                        "{stats}: {plan:?}"
                    );
                    assert!(
                        !plan
                            .iter()
                            .any(|l| l.starts_with("SCAN approval_requests")
                                || l.starts_with("SCAN d")),
                        "{stats}: {plan:?}"
                    );
                }
            }
            Ok(())
        })
        .unwrap();
    }
}
