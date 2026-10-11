//! Approvals (spec 2026-10-08-multi-agent-multi-person §9.1): "agents of
//! `approver` may see `author`'s content at `scope`", and the visibility
//! generation (M17), which every change to what an agent may see bumps.
//!
//! Requests for approvals, deciding them and their limits are separate
//! (§9.2–§9.3); this module makes, revokes and lists the approvals
//! themselves.

use super::Store;
use crate::audit::{AuditCtx, AuditKind, AuditRecord};
use crate::visibility::placeholder::ApprovalState;
use crate::{CoreError, Result, is_public_id, new_ulid};
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// The author class of every viewer who is neither the owner nor a
/// connected person, and of anonymous callers (M14).
pub const UNCONNECTED: &str = "*unconnected";

/// The owner as an author, where a public ID is not used (`doc_authors`,
/// `comments.page_person`, `versions.file_authors_json`).
pub const OWNER: &str = "*owner";

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
        .prepare_cached(
            "SELECT COALESCE(MAX(status = 'open'), 0), COALESCE(MAX(status = 'declined'), 0)
             FROM approval_requests
             WHERE COALESCE(approver, '') = COALESCE(?1, '') AND artifact_id = ?2
               AND (thread_id = ?3 OR thread_id IS NULL)
               AND author IN (SELECT value FROM json_each(?4))",
        )?
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

/// Clears `placeholder_at` on the undelivered rows `a` (an approval made
/// or revoked) bears on: rows of its approver's sessions in its scope
/// whose comment its author wrote (or a page wrote, or anyone when it is
/// of `*unconnected`). A revoked approval's rows are held again and told
/// once more (spec §9.4), and rows withheld for other authors are not told
/// again; an approved author's rows read `sent` until a take delivers
/// them.
pub(super) fn reset_placeholders(tx: &rusqlite::Transaction<'_>, a: &Approval) -> Result<()> {
    tx.execute(
        "UPDATE feedback SET placeholder_at = NULL
         WHERE delivered_at IS NULL AND placeholder_at IS NOT NULL
           AND target_session_id IN (SELECT id FROM sessions WHERE COALESCE(person, '') = COALESCE(?1, ''))
           AND (?2 = 'everywhere'
                OR (?2 = 'artifact' AND thread_id IN (SELECT id FROM threads WHERE artifact_id = ?3))
                OR (?2 = 'thread' AND thread_id = ?4))
           AND (?5 = '*unconnected' OR comment_id IN (
                SELECT id FROM comments WHERE author_public_id = ?5 OR via_page = 1))",
        params![
            a.approver,
            a.scope.as_str(),
            a.artifact_id,
            a.thread_id,
            a.author
        ],
    )?;
    Ok(())
}

/// Revokes live approval `id` at `now` in `tx`, recording `approval.revoke`
/// under `ctx` and marking the visibility change.
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
    reset_placeholders(tx, &a)?;
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
    listener: RwLock<Option<Listener>>,
}

impl Default for VisState {
    fn default() -> Self {
        VisState {
            generation: Arc::new(AtomicU64::new(0)),
            changed: AtomicBool::new(false),
            fast: AtomicU64::new(0),
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

    /// Approves `author`'s content reaching the agents of `approver` (a
    /// person's public ID; `None`: the owner) at `scope`, or finds the live
    /// approval that already does. A new approval is recorded as
    /// `approval.grant` under `ctx` and bumps the visibility generation.
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
        self.with_tx(|tx| {
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
            if let Some(a) = existing {
                return Ok(a);
            }
            let a = Approval {
                id: new_ulid(),
                approver: approver.map(str::to_string),
                author: author.to_string(),
                scope: kind,
                artifact_id,
                thread_id,
                created_at: self.now(),
                created_via: via.as_str().to_string(),
                request_id: None,
                revoked_at: None,
            };
            tx.execute(
                "INSERT INTO approvals (id, approver, author, scope, artifact_id, thread_id, created_at, created_via)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![a.id, a.approver, a.author, a.scope.as_str(), a.artifact_id, a.thread_id, a.created_at, a.created_via],
            )?;
            self.record_audit(tx, ctx, approval_record(AuditKind::ApprovalGrant, &a.created_at, &a))?;
            reset_placeholders(tx, &a)?;
            self.visibility_changed();
            Ok(a)
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Via as ClaimVia;
    use crate::store::people::InvitePerson;
    use crate::store::people::tests::{KEY_B, approved, code, pending, store};
    use crate::store::test_util::{DAEMON, anchor, artifact};
    use crate::store::threads::NewThread;

    /// A person named `name`, connected by invite; their public ID.
    fn person(st: &Store, name: &str, key: u8) -> String {
        let (gid, _, _) = approved(st, name, [key; 32]);
        st.viewer_of_grant(&gid).unwrap().public_id
    }

    /// A LAN viewer who is not connected; its public ID.
    fn lan(st: &Store, cookie: &str, name: &str) -> String {
        st.upsert_viewer(cookie, Some(name)).unwrap().public_id
    }

    fn thread(st: &Store, aid: &crate::ArtifactId) -> String {
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
        let reader = crate::rights::Rights::preset("clax:reader").unwrap();
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
        st.narrow_grant(DAEMON, &gid, Some(reader)).unwrap();
        bumps(&st, "a grant's narrowing set again", 0);
        // A second invite for someone already a person changes nothing.
        st.create_invite(DAEMON, InvitePerson::Viewer(sam.clone()), None)
            .unwrap();
        bumps(&st, "a second invite for a person", 0);
        // Grants ending, each way.
        st.revoke_token(DAEMON, &gid, manage.expose()).unwrap();
        bumps(&st, "a grant's token revoked", 1);
        st.revoke_grant(DAEMON, &gid, crate::store::people::EndedBy::Owner)
            .unwrap();
        bumps(&st, "an ended grant revoked again", 0);
        st.revoke_grant(DAEMON, &req.id, crate::store::people::EndedBy::Owner)
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
}
