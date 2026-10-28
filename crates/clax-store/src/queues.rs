//! Report queues (spec 2026-10-27-clax-sdk-design §5.3, §7): a queue a
//! person makes, its members, its service grants and the reporter tokens
//! they mint, the reports filed through it (threads on external
//! artifacts, [`crate::external`]), and the leases agents claim them by.
//!
//! A report is a thread with `queue_id`. Its first comment is a viewer
//! comment carrying the reporter's ID; the report's `metadata` is the
//! thread's. A queue's rights reach its own threads alone: the artifact
//! holding them may hold other queues' reports too (D17).
//!
//! A claim is a lease of [`LEASE_SECS`]: `queue_claims` keeps one row per
//! session that held one on a thread, and the thread's current lease is the
//! row with the largest `fence`. A lease is live until `expires_at` on the
//! store's clock, while its session has not ended, and until it is
//! released; a released row is kept, so no fence is given twice. A claim makes its
//! session the thread's owner (`threads.target_session_id`); a lapsed
//! lease's owner is cleared by the next claim, renewal, release or check
//! of the thread.

use super::Store;
use super::external::{EXTERNAL_SELECT, NewExternal, row_to_external};
use super::threads::{
    AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread, check_not_taken_in, clip_problem,
};
use clax_core::anchor::Anchor;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::model::{Comment, Session, Thread};
use clax_core::queue::{
    AUTHOR_REPORTER, Claim, LEASE_SECS, Lease, MAX_ARTIFACT_METADATA_BYTES,
    MAX_THREAD_METADATA_BYTES, Queue, REPORT_PAGE, REPORTER_TOKEN_PREFIX, REPORTER_TOKEN_SECS,
    Report, ReportComment, ReportersSee, Role, SERVICE_TOKEN_PREFIX, State, check_queue_name,
    check_reporter_id, metadata_json, metadata_sha256, normalize_origin,
};
use clax_core::secret::Secret;
use clax_core::sig::KeyPair;
use clax_core::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// A queue to make ([`Store::create_queue`]).
#[derive(Clone, Debug)]
pub struct NewQueue {
    pub name: String,
    /// Origins as given; each is normalized ([`normalize_origin`]).
    pub origins: Vec<String>,
    pub reporters_see: ReportersSee,
}

/// A change to a queue ([`Store::update_queue`]): each field given is set.
#[derive(Clone, Debug, Default)]
pub struct QueuePatch {
    pub name: Option<String>,
    pub origins: Option<Vec<String>>,
    pub reporters_see: Option<ReportersSee>,
}

/// A live queue as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueRow {
    pub id: String,
    pub name: String,
    /// Its maker's public ID; `None` for the owner.
    pub person: Option<String>,
    pub origins: Vec<String>,
    pub reporters_see: ReportersSee,
    pub created_at: String,
}

impl QueueRow {
    /// As a member of `role` reads it.
    pub fn view(&self, role: Role) -> Queue {
        Queue {
            id: self.id.clone(),
            name: self.name.clone(),
            origins: self.origins.clone(),
            reporters_see: self.reporters_see,
            created_at: self.created_at.clone(),
            role,
        }
    }
}

/// A service grant of a queue: what the deployer's backend signs with to
/// mint reporter tokens. Its token and private key are shown once, when
/// it is made, and kept nowhere.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ServiceGrant {
    pub id: String,
    #[serde(skip)]
    pub queue_id: String,
    pub name: String,
    /// Its Ed25519 public key, unpadded base64url.
    #[serde(skip)]
    pub public_key: String,
    pub created_at: String,
}

/// A live reporter token ([`Store::reporter_by_token`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReporterToken {
    pub queue_id: String,
    pub service_grant_id: String,
    pub reporter_id: String,
    /// The origin it was minted for: its requests must come from it.
    pub origin: String,
    /// The reporter's Ed25519 public key, unpadded base64url.
    pub public_key: String,
    pub expires_at: String,
}

/// A report to file ([`Store::file_report_async`]).
#[derive(Clone, Debug)]
pub struct NewReport {
    /// The app's ID of the record; `None`: the page URL without its route.
    pub external_id: Option<String>,
    /// The page the report was filed on.
    pub url: String,
    pub title: String,
    pub anchor: Anchor,
    pub body: String,
    /// The first comment's author name.
    pub author_name: String,
    /// The thread's metadata; `Null` for none.
    pub metadata: serde_json::Value,
    /// The app's revision of the record.
    pub revision: Option<String>,
    /// A PNG clip ([`clip_problem`] says what is refused).
    pub clip: Option<Vec<u8>>,
}

/// Whose reports a read sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility<'a> {
    /// A member's: every report of the queue.
    All,
    /// A reporter token's: its reporter's own under `reporters_see: own`,
    /// every report under `all`.
    Reporter(&'a str),
}

/// Who writes to a report ([`Store::report_reply`], [`Store::resolve_report`]).
#[derive(Clone, Debug)]
pub enum ReportWriter<'a> {
    /// An agent's session: it writes only under its own live lease.
    Agent(&'a Session),
    /// A triage member themself (in a browser, or the owner by hand): no
    /// claim is needed. `public_id` is their viewer's; `None` writes as
    /// anonymous.
    Person {
        name: String,
        public_id: Option<String>,
    },
}

impl ReportWriter<'_> {
    /// The agent's session; `None` for a person.
    pub fn session(&self) -> Option<&str> {
        match self {
            ReportWriter::Agent(s) => Some(&s.id),
            ReportWriter::Person { .. } => None,
        }
    }
}

/// The reports of `?1` with their external artifact; callers append `AND ...`.
const REPORT_SELECT: &str = "SELECT t.id, t.queue_id, t.anchor_json, t.status, t.reporter_id,
    t.metadata_json, t.metadata_version, t.has_clip, t.created_at
    FROM threads t JOIN artifacts a ON a.id = t.artifact_id
    JOIN external_artifacts x ON x.artifact_id = t.artifact_id
    WHERE a.deleted_at IS NULL AND t.queue_id = ?1";

/// Whether thread `t.id`'s current lease is live at `?2`.
const LIVE_CLAIM: &str =
    "EXISTS (SELECT 1 FROM queue_claims c JOIN sessions s ON s.id = c.session_id
    WHERE c.thread_id = t.id AND c.expires_at > ?2 AND s.ended_at IS NULL AND NOT c.released
      AND c.fence = (SELECT MAX(fence) FROM queue_claims WHERE thread_id = t.id))";

fn queue_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<QueueRow> {
    let origins: String = r.get(3)?;
    let see: String = r.get(4)?;
    Ok(QueueRow {
        id: r.get(0)?,
        name: r.get(1)?,
        person: r.get(2)?,
        origins: serde_json::from_str(&origins).unwrap_or_default(),
        reporters_see: ReportersSee::parse(&see).unwrap_or(ReportersSee::Own),
        created_at: r.get(5)?,
    })
}

const QUEUE_SELECT: &str =
    "SELECT id, name, person, origins_json, reporters_see, created_at FROM queues";

fn queue_in(c: &Connection, qid: &str) -> Result<Option<QueueRow>> {
    Ok(c.prepare_cached(&format!(
        "{QUEUE_SELECT} WHERE id = ?1 AND deleted_at IS NULL"
    ))?
    .query_row(params![qid], queue_row)
    .optional()?)
}

/// `person`'s role on queue `q` (`None`: the owner): its maker's is admin.
fn role_in(c: &Connection, q: &QueueRow, person: Option<&str>) -> Result<Option<Role>> {
    if q.person.as_deref() == person {
        return Ok(Some(Role::Admin));
    }
    let Some(p) = person else {
        return Ok(None);
    };
    let role: Option<String> = c
        .query_row(
            "SELECT role FROM queue_members WHERE queue_id = ?1 AND person = ?2",
            params![q.id, p],
            |r| r.get(0),
        )
        .optional()?;
    Ok(role.as_deref().and_then(Role::parse))
}

fn normalize_origins(origins: &[String]) -> Result<Vec<String>> {
    if origins.len() > clax_core::queue::MAX_QUEUE_ORIGINS {
        return Err(CoreError::invalid(
            "invalid_origin",
            format!(
                "a queue lists at most {} origins",
                clax_core::queue::MAX_QUEUE_ORIGINS
            ),
        ));
    }
    let mut out = Vec::with_capacity(origins.len());
    for o in origins {
        let n = normalize_origin(o)?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

fn queue_record(kind: AuditKind, at: &str, qid: &str) -> AuditRecord {
    AuditRecord::new(kind, at).with("queue_id", qid)
}

/// `at` plus `secs`, as the store writes times.
fn later(at: chrono::DateTime<chrono::Utc>, secs: i64) -> String {
    clax_core::time::stamp(at + chrono::Duration::seconds(secs))
}

/// A thread's current lease: its session, lease ID, fence, expiry, and
/// whether it is live at `now`.
struct Current {
    session_id: String,
    lease_id: String,
    fence: u64,
    expires_at: String,
    live: bool,
    holder_name: String,
}

fn current_in(c: &Connection, tid: &str, now: &str) -> Result<Option<Current>> {
    Ok(c.query_row(
        "SELECT c.session_id, c.lease_id, c.fence, c.expires_at,
            c.expires_at > ?2 AND s.ended_at IS NULL AND s.id IS NOT NULL AND NOT c.released,
            COALESCE(s.name, s.harness, 'an agent')
         FROM queue_claims c LEFT JOIN sessions s ON s.id = c.session_id
         WHERE c.thread_id = ?1 ORDER BY c.fence DESC LIMIT 1",
        params![tid, now],
        |r| {
            Ok(Current {
                session_id: r.get(0)?,
                lease_id: r.get(1)?,
                fence: r.get(2)?,
                expires_at: r.get(3)?,
                live: r.get(4)?,
                holder_name: r.get(5)?,
            })
        },
    )
    .optional()?)
}

/// Clears thread `tid`'s owner when it is the session of its lapsed
/// current lease.
fn settle_in(st: &Store, tx: &Transaction<'_>, tid: &str, now: &str) -> Result<()> {
    if let Some(cur) = current_in(tx, tid, now)?
        && !cur.live
    {
        let n = tx.execute(
            "UPDATE threads SET target_session_id = NULL WHERE id = ?1 AND target_session_id = ?2",
            params![tid, cur.session_id],
        )?;
        if n > 0 {
            st.agents_pinged();
        }
    }
    Ok(())
}

/// `Ok` when session `sid` may write to thread `tid` as [`Store::check_lease`]
/// says.
fn check_lease_in(c: &Connection, tid: &str, sid: &str, now: &str) -> Result<()> {
    let cur = current_in(c, tid, now)?;
    let held: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM queue_claims WHERE thread_id = ?1 AND session_id = ?2
            AND NOT released)",
        params![tid, sid],
        |r| r.get(0),
    )?;
    match cur {
        Some(cur) if cur.live && cur.session_id == sid => Ok(()),
        Some(_) | None if held => Err(CoreError::LeaseLost),
        Some(cur) if cur.live => Err(CoreError::ThreadTaken {
            name: cur.holder_name,
        }),
        _ => Err(CoreError::ClaimRequired),
    }
}

/// Thread `tid` of queue `qid` exists on a live artifact, and its artifact.
fn report_artifact_in(c: &Connection, qid: &str, tid: &str) -> Result<String> {
    c.query_row(
        "SELECT t.artifact_id FROM threads t JOIN artifacts a ON a.id = t.artifact_id
         WHERE t.id = ?1 AND t.queue_id = ?2 AND a.deleted_at IS NULL",
        params![tid, qid],
        |r| r.get(0),
    )
    .optional()?
    .ok_or(CoreError::NotFound)
}

struct ReportRow {
    id: String,
    queue_id: String,
    anchor_json: String,
    status: String,
    reporter_id: Option<String>,
    metadata_json: Option<String>,
    metadata_version: u64,
    has_clip: bool,
    created_at: String,
}

fn report_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReportRow> {
    Ok(ReportRow {
        id: r.get(0)?,
        queue_id: r.get(1)?,
        anchor_json: r.get(2)?,
        status: r.get(3)?,
        reporter_id: r.get(4)?,
        metadata_json: r.get(5)?,
        metadata_version: r.get(6)?,
        has_clip: r.get(7)?,
        created_at: r.get(8)?,
    })
}

/// The report views of `rows`, read on `c` at `now`.
fn reports_of(c: &Connection, rows: Vec<ReportRow>, now: &str) -> Result<Vec<Report>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let artifact = c
            .prepare_cached(&format!(
                "{EXTERNAL_SELECT} WHERE x.artifact_id = (SELECT artifact_id FROM threads WHERE id = ?1)"
            ))?
            .query_row(params![row.id], row_to_external)?;
        let anchor: Anchor =
            serde_json::from_str(&row.anchor_json).map_err(|_| CoreError::Corrupt {
                artifact_id: artifact.artifact_id.clone(),
                column: "anchor_json",
                version: None,
            })?;
        let comments = c
            .prepare_cached(
                "SELECT id, author_kind, author_name, body, created_at, reporter_id FROM comments
                 WHERE thread_id = ?1 ORDER BY created_at, id",
            )?
            .query_map(params![row.id], |r| {
                let reporter: Option<String> = r.get(5)?;
                Ok(ReportComment {
                    id: r.get(0)?,
                    author_kind: match reporter {
                        Some(_) => AUTHOR_REPORTER.to_string(),
                        None => r.get(1)?,
                    },
                    author_name: r.get(2)?,
                    body: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let claim = current_in(c, &row.id, now)?
            .filter(|cur| cur.live)
            .map(|cur| Claim {
                holder_name: cur.holder_name,
                expires_at: cur.expires_at,
            });
        let state = if row.status == "resolved" {
            State::Resolved
        } else if claim.is_some() {
            State::Claimed
        } else {
            State::Open
        };
        out.push(Report {
            id: row.id,
            queue_id: row.queue_id,
            artifact: artifact.view(),
            anchor,
            status: row.status,
            state,
            reporter_id: row.reporter_id,
            metadata: row
                .metadata_json
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or(serde_json::Value::Null),
            metadata_version: row.metadata_version,
            has_clip: row.has_clip,
            comments,
            claim,
            created_at: row.created_at,
        });
    }
    Ok(out)
}

/// Report `tid` of queue `qid` as `vis` sees it, on `c` at `now`.
fn report_in(
    c: &Connection,
    q: &QueueRow,
    tid: &str,
    vis: Visibility<'_>,
    now: &str,
) -> Result<Option<Report>> {
    let (filter, reporter) = reporter_filter(q, vis);
    let row = c
        .prepare_cached(&format!("{REPORT_SELECT} AND t.id = ?3 {filter}"))?
        .query_row(params![q.id, now, tid, reporter], report_row)
        .optional()?;
    Ok(reports_of(c, row.into_iter().collect(), now)?.pop())
}

/// The SQL limiting reports to what `vis` sees (parameter `?4`), and its
/// value.
fn reporter_filter<'a>(q: &QueueRow, vis: Visibility<'a>) -> (&'static str, Option<&'a str>) {
    match vis {
        Visibility::Reporter(r) if q.reporters_see == ReportersSee::Own => {
            ("AND t.reporter_id = ?4", Some(r))
        }
        _ => ("AND ?4 IS NULL", None),
    }
}

impl Store {
    /// Makes a queue of `person` (`None`: the owner), who is its admin,
    /// recorded as `queue.create {queue_id, name, origins, reporters_see}`
    /// under `ctx`.
    ///
    /// # Errors
    /// `invalid_name` or `invalid_origin` for bad input.
    pub fn create_queue(&self, ctx: &AuditCtx, person: Option<&str>, q: NewQueue) -> Result<Queue> {
        let name = check_queue_name(&q.name)?;
        let origins = normalize_origins(&q.origins)?;
        let id = new_ulid();
        let now = self.now();
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO queues (id, name, person, origins_json, reporters_see, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    name,
                    person,
                    serde_json::to_string(&origins).expect("strings serialise"),
                    q.reporters_see.as_str(),
                    now
                ],
            )?;
            let rec = queue_record(AuditKind::QueueCreate, &now, &id)
                .with("name", name.as_str())
                .with("origins", origins.clone())
                .with("reporters_see", q.reporters_see.as_str());
            self.record_audit(tx, ctx, rec)?;
            let row = queue_in(tx, &id)?.ok_or(CoreError::NotFound)?;
            Ok(row.view(Role::Admin))
        })
    }

    /// The live queues `person` (`None`: the owner) made or is a member
    /// of, oldest first, each with their role.
    ///
    /// # Errors
    /// Database errors.
    pub fn queues_of(&self, person: Option<&str>) -> Result<Vec<Queue>> {
        self.with_read(|c| {
            let rows: Vec<QueueRow> = c
                .prepare_cached(&format!(
                    "{QUEUE_SELECT} WHERE deleted_at IS NULL AND (person IS ?1
                       OR (?1 IS NOT NULL AND id IN (SELECT queue_id FROM queue_members WHERE person = ?1)))
                     ORDER BY created_at, id"
                ))?
                .query_map(params![person], queue_row)?
                .collect::<rusqlite::Result<_>>()?;
            let mut out = Vec::with_capacity(rows.len());
            for q in rows {
                if let Some(role) = role_in(c, &q, person)? {
                    out.push(q.view(role));
                }
            }
            Ok(out)
        })
    }

    /// Live queue `qid`.
    ///
    /// # Errors
    /// Database errors.
    pub fn queue(&self, qid: &str) -> Result<Option<QueueRow>> {
        self.with_read(|c| queue_in(c, qid))
    }

    /// `person`'s role on live queue `qid` (`None`: the owner): its
    /// maker's is admin; `None` for a non-member or no such queue.
    ///
    /// # Errors
    /// Database errors.
    pub fn queue_role(&self, qid: &str, person: Option<&str>) -> Result<Option<Role>> {
        self.with_read(|c| match queue_in(c, qid)? {
            Some(q) => role_in(c, &q, person),
            None => Ok(None),
        })
    }

    /// Changes queue `qid` as `p` says, recorded as `queue.update {queue_id,
    /// op: "update", <each field set>}`; a patch that changes nothing
    /// records nothing.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `invalid_name` or
    /// `invalid_origin` for bad input.
    pub fn update_queue(&self, ctx: &AuditCtx, qid: &str, p: QueuePatch) -> Result<QueueRow> {
        let name = p.name.as_deref().map(check_queue_name).transpose()?;
        let origins = p.origins.as_deref().map(normalize_origins).transpose()?;
        self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let mut rec =
                queue_record(AuditKind::QueueUpdate, &self.now(), qid).with("op", "update");
            let mut changed = false;
            if let Some(n) = name.filter(|n| *n != q.name) {
                tx.execute("UPDATE queues SET name = ?2 WHERE id = ?1", params![qid, n])?;
                rec = rec.with("name", n);
                changed = true;
            }
            if let Some(o) = origins.filter(|o| *o != q.origins) {
                tx.execute(
                    "UPDATE queues SET origins_json = ?2 WHERE id = ?1",
                    params![qid, serde_json::to_string(&o).expect("strings serialise")],
                )?;
                rec = rec.with("origins", o);
                changed = true;
            }
            if let Some(s) = p.reporters_see.filter(|s| *s != q.reporters_see) {
                tx.execute(
                    "UPDATE queues SET reporters_see = ?2 WHERE id = ?1",
                    params![qid, s.as_str()],
                )?;
                rec = rec.with("reporters_see", s.as_str());
                changed = true;
            }
            if changed {
                self.record_audit(tx, ctx, rec)?;
            }
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)
        })
    }

    /// Deletes queue `qid`: it is gone from every listing, its service
    /// grants are revoked and its reporter tokens end; its reports stay on
    /// their artifacts. Recorded as `queue.update {queue_id, op: "delete"}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn delete_queue(&self, ctx: &AuditCtx, qid: &str) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            tx.execute(
                "UPDATE queues SET deleted_at = ?2 WHERE id = ?1",
                params![qid, now],
            )?;
            tx.execute(
                "UPDATE queue_service_grants SET revoked_at = ?2 WHERE queue_id = ?1 AND revoked_at IS NULL",
                params![qid, now],
            )?;
            tx.execute(
                "DELETE FROM queue_reporter_tokens WHERE queue_id = ?1",
                params![qid],
            )?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid).with("op", "delete");
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Sets `person`'s role on queue `qid` (`None` removes them), recorded
    /// as `queue.update {queue_id, op: "member", member, role}` (`role`
    /// null for a removal); a change to nothing records nothing. The
    /// queue's maker is its admin whatever this says.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn set_member(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        person: &str,
        role: Option<Role>,
    ) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let before: Option<String> = tx
                .query_row(
                    "SELECT role FROM queue_members WHERE queue_id = ?1 AND person = ?2",
                    params![qid, person],
                    |r| r.get(0),
                )
                .optional()?;
            if before.as_deref() == role.map(Role::as_str) {
                return Ok(());
            }
            match role {
                Some(r) => tx.execute(
                    "INSERT INTO queue_members (queue_id, person, role, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(queue_id, person) DO UPDATE SET role = excluded.role",
                    params![qid, person, r.as_str(), now],
                )?,
                None => tx.execute(
                    "DELETE FROM queue_members WHERE queue_id = ?1 AND person = ?2",
                    params![qid, person],
                )?,
            };
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "member")
                .with("member", person)
                .with("role", role.map(Role::as_str));
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Queue `qid`'s members other than its maker, by public ID, with
    /// their roles, in the order they were added.
    ///
    /// # Errors
    /// Database errors.
    pub fn queue_members(&self, qid: &str) -> Result<Vec<(String, Role)>> {
        self.with_read(|c| {
            let rows: Vec<(String, String)> = c
                .prepare_cached(
                    "SELECT person, role FROM queue_members WHERE queue_id = ?1
                     ORDER BY created_at, person",
                )?
                .query_map(params![qid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows
                .into_iter()
                .filter_map(|(p, r)| Role::parse(&r).map(|r| (p, r)))
                .collect())
        })
    }

    /// Makes a service grant of queue `qid` named `name`: its token
    /// ([`SERVICE_TOKEN_PREFIX`]) and a new Ed25519 key pair, of which only
    /// the token's SHA-256 and the public key are kept. Recorded as
    /// `queue.update {queue_id, op: "grant", grant_id, grant_name}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `invalid_name` for a
    /// bad name.
    pub fn create_service_grant(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        name: &str,
    ) -> Result<(ServiceGrant, Secret, KeyPair)> {
        let name = check_queue_name(name)?;
        let token = clax_core::secret::token(SERVICE_TOKEN_PREFIX);
        let key = KeyPair::generate();
        let public = clax_core::sig::encode_public(&key.public());
        let id = new_ulid();
        let now = self.now();
        let grant = ServiceGrant {
            id: id.clone(),
            queue_id: qid.to_string(),
            name: name.clone(),
            public_key: public.clone(),
            created_at: now.clone(),
        };
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            tx.execute(
                "INSERT INTO queue_service_grants (id, queue_id, name, token_sha256, public_key, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, qid, name, token.sha256_hex(), public, now],
            )?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "grant")
                .with("grant_id", id.as_str())
                .with("grant_name", name.as_str());
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })?;
        Ok((grant, token, key))
    }

    /// Queue `qid`'s service grants that are not revoked, oldest first.
    ///
    /// # Errors
    /// Database errors.
    pub fn service_grants(&self, qid: &str) -> Result<Vec<ServiceGrant>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT id, queue_id, name, public_key, created_at FROM queue_service_grants
                 WHERE queue_id = ?1 AND revoked_at IS NULL ORDER BY created_at, id",
            )?
            .query_map(params![qid], grant_row)?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The service grant whose token's SHA-256 (lowercase hex) is
    /// `sha256`, when it is not revoked and its queue is live.
    ///
    /// # Errors
    /// Database errors.
    pub fn service_grant_by_token(&self, sha256: &str) -> Result<Option<ServiceGrant>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT g.id, g.queue_id, g.name, g.public_key, g.created_at
                 FROM queue_service_grants g JOIN queues q ON q.id = g.queue_id
                 WHERE g.token_sha256 = ?1 AND g.revoked_at IS NULL AND q.deleted_at IS NULL",
            )?
            .query_row(params![sha256], grant_row)
            .optional()?)
        })
    }

    /// Revokes service grant `gid` of queue `qid`; every reporter token it
    /// minted ends with it. Recorded as `queue.update {queue_id, op:
    /// "revoke_grant", grant_id}`.
    ///
    /// # Errors
    /// `NotFound` for no such live grant of the queue.
    pub fn revoke_service_grant(&self, ctx: &AuditCtx, qid: &str, gid: &str) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            let n = tx.execute(
                "UPDATE queue_service_grants SET revoked_at = ?3
                 WHERE id = ?1 AND queue_id = ?2 AND revoked_at IS NULL",
                params![gid, qid, now],
            )?;
            if n == 0 {
                return Err(CoreError::NotFound);
            }
            tx.execute(
                "DELETE FROM queue_reporter_tokens WHERE service_grant_id = ?1",
                params![gid],
            )?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "revoke_grant")
                .with("grant_id", gid);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Mints a reporter token ([`REPORTER_TOKEN_PREFIX`]) of `grant` for
    /// reporter `reporter_id` on `origin`, bound to the reporter's Ed25519
    /// `public_key` (unpadded base64url), valid for
    /// [`REPORTER_TOKEN_SECS`]. Answers the token, kept only as its
    /// SHA-256, and its expiry. A token minting changes no history and
    /// records nothing. Tokens that expired a day before are dropped.
    ///
    /// # Errors
    /// [`CoreError::WrongOrigin`] for an origin the queue does not list;
    /// `invalid_origin`, `invalid_reporter_id` or `invalid_key` for bad
    /// input; `NotFound` when the grant or its queue is gone.
    pub fn mint_reporter_token(
        &self,
        grant: &ServiceGrant,
        reporter_id: &str,
        origin: &str,
        public_key: &str,
    ) -> Result<(Secret, String)> {
        check_reporter_id(reporter_id)?;
        let origin = normalize_origin(origin)?;
        clax_core::sig::decode_public(public_key)
            .map_err(|_| CoreError::invalid("invalid_key", "public_key is not an Ed25519 key"))?;
        let token = clax_core::secret::token(REPORTER_TOKEN_PREFIX);
        let now = self.now_utc();
        let expires_at = later(now, REPORTER_TOKEN_SECS);
        self.with_tx(|tx| {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_service_grants WHERE id = ?1 AND queue_id = ?2
                    AND revoked_at IS NULL)",
                params![grant.id, grant.queue_id],
                |r| r.get(0),
            )?;
            let q = queue_in(tx, &grant.queue_id)?;
            let (true, Some(q)) = (live, q) else {
                return Err(CoreError::NotFound);
            };
            if !q.origins.contains(&origin) {
                return Err(CoreError::WrongOrigin);
            }
            tx.execute(
                "DELETE FROM queue_reporter_tokens WHERE expires_at <= ?1",
                params![later(now, -86_400)],
            )?;
            tx.execute(
                "INSERT INTO queue_reporter_tokens (token_sha256, queue_id, service_grant_id,
                    reporter_id, origin, public_key, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    token.sha256_hex(),
                    grant.queue_id,
                    grant.id,
                    reporter_id,
                    origin,
                    public_key,
                    expires_at
                ],
            )?;
            Ok(())
        })?;
        Ok((token, expires_at))
    }

    /// The reporter token whose SHA-256 (lowercase hex) is `sha256`, when
    /// its grant is not revoked and its queue is live; `None` for no such
    /// token.
    ///
    /// # Errors
    /// [`CoreError::TokenExpired`] for a token past its expiry.
    pub fn reporter_by_token(&self, sha256: &str) -> Result<Option<ReporterToken>> {
        let now = self.now();
        let found = self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT r.queue_id, r.service_grant_id, r.reporter_id, r.origin, r.public_key, r.expires_at
                 FROM queue_reporter_tokens r
                 JOIN queue_service_grants g ON g.id = r.service_grant_id
                 JOIN queues q ON q.id = r.queue_id
                 WHERE r.token_sha256 = ?1 AND g.revoked_at IS NULL AND q.deleted_at IS NULL",
            )?
            .query_row(params![sha256], |r| {
                Ok(ReporterToken {
                    queue_id: r.get(0)?,
                    service_grant_id: r.get(1)?,
                    reporter_id: r.get(2)?,
                    origin: r.get(3)?,
                    public_key: r.get(4)?,
                    expires_at: r.get(5)?,
                })
            })
            .optional()?)
        })?;
        match found {
            Some(t) if t.expires_at <= now => Err(CoreError::TokenExpired),
            other => Ok(other),
        }
    }

    /// [`Store::file_report_async`], blocking on its content store.
    ///
    /// # Errors
    /// [`Store::file_report_async`]'s.
    pub fn file_report(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        reporter: Option<&str>,
        r: NewReport,
    ) -> Result<Report> {
        self.block_on(self.file_report_async(ctx, qid, reporter, r))
    }

    /// Files report `r` to queue `qid` by `reporter` (a reporter token's
    /// reporter ID; `None` otherwise). Finds or makes the external
    /// artifact `r.external_id` names, or the page URL without its route
    /// when it names none ([`Store::ensure_external_async`], as the queue's
    /// maker's), then makes the thread on its current version with the
    /// queue, the reporter and the metadata in the thread's transaction
    /// (`thread.open`, `comment.add`). The anchor's file is the artifact's
    /// one page, and a route it names is dropped: an external artifact is
    /// no live page.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `invalid_clip`,
    /// `metadata_too_large`, `invalid_reporter_id` and the errors of
    /// [`Store::ensure_external_async`] and [`Store::create_thread`] for
    /// bad input.
    pub async fn file_report_async(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        reporter: Option<&str>,
        r: NewReport,
    ) -> Result<Report> {
        let q = self.queue(qid)?.ok_or(CoreError::NotAMember)?;
        if let Some(rid) = reporter {
            check_reporter_id(rid)?;
        }
        if let Some(problem) = r.clip.as_deref().and_then(clip_problem) {
            return Err(CoreError::invalid("invalid_clip", problem));
        }
        let meta = match &r.metadata {
            serde_json::Value::Null => None,
            v => Some(metadata_json(v, MAX_THREAD_METADATA_BYTES)?),
        };
        let page = clax_core::live::parse_page_url(&r.url)?;
        let external_id = match r.external_id {
            Some(id) => id,
            None => page.key.page_url(),
        };
        let (x, _) = self
            .ensure_external_async(
                ctx,
                q.person.as_deref(),
                NewExternal {
                    external_id,
                    title: r.title.clone(),
                    url: page.key.page_url(),
                    revision: r.revision.clone(),
                },
            )
            .await?;
        let aid = ArtifactId::parse(&x.artifact_id)?;
        let version_n = self
            .get_artifact(&aid)?
            .ok_or(CoreError::NotFound)?
            .current_version;
        let mut anchor = r.anchor;
        anchor.route = None;
        if !anchor.is_whole() {
            anchor.file = clax_core::publish::INDEX.to_string();
        }
        let t = NewThread {
            version_n,
            anchor,
            author_name: r.author_name,
            author_public_id: None,
            body: r.body,
            clip: r.clip,
            via_page: false,
        };
        let qid_owned = qid.to_string();
        let reporter_owned = reporter.map(str::to_string);
        let thread = self
            .create_thread_then_async(ctx, &aid, t, move |tx, tid| {
                tx.execute(
                    "UPDATE threads SET queue_id = ?2, reporter_id = ?3, metadata_json = ?4,
                        metadata_version = 1 WHERE id = ?1",
                    params![tid, qid_owned, reporter_owned, meta],
                )?;
                if reporter_owned.is_some() {
                    tx.execute(
                        "UPDATE comments SET reporter_id = ?2 WHERE thread_id = ?1",
                        params![tid, reporter_owned],
                    )?;
                }
                Ok((None, Vec::new()))
            })
            .await?;
        self.report(qid, &thread.id, Visibility::All)?
            .ok_or(CoreError::NotFound)
    }

    /// A page of queue `qid`'s reports that `vis` sees, in order of arrival,
    /// after report `cursor` (`None`: from the first), in `state` (`None`:
    /// any), at most [`REPORT_PAGE`]; and the cursor of the next page when
    /// there is one.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn reports(
        &self,
        qid: &str,
        vis: Visibility<'_>,
        state: Option<State>,
        cursor: Option<&str>,
    ) -> Result<(Vec<Report>, Option<String>)> {
        let now = self.now();
        self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            let (filter, reporter) = reporter_filter(&q, vis);
            let state_sql = match state {
                None => String::new(),
                Some(State::Resolved) => "AND t.status = 'resolved'".into(),
                Some(State::Claimed) => format!("AND t.status = 'open' AND {LIVE_CLAIM}"),
                Some(State::Open) => format!("AND t.status = 'open' AND NOT {LIVE_CLAIM}"),
            };
            let sql = format!(
                "{REPORT_SELECT} {filter} {state_sql}
                 AND (?3 IS NULL OR (t.created_at, t.id) > (SELECT created_at, id FROM threads WHERE id = ?3))
                 ORDER BY t.created_at, t.id LIMIT ?5"
            );
            let mut rows: Vec<ReportRow> = c
                .prepare_cached(&sql)?
                .query_map(
                    params![qid, now, cursor, reporter, REPORT_PAGE as i64 + 1],
                    report_row,
                )?
                .collect::<rusqlite::Result<_>>()?;
            let next = if rows.len() > REPORT_PAGE {
                rows.truncate(REPORT_PAGE);
                rows.last().map(|r| r.id.clone())
            } else {
                None
            };
            Ok((reports_of(c, rows, &now)?, next))
        })
    }

    /// Report `tid` of queue `qid`, when `vis` sees it.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn report(&self, qid: &str, tid: &str, vis: Visibility<'_>) -> Result<Option<Report>> {
        let now = self.now();
        self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            report_in(c, &q, tid, vis, &now)
        })
    }

    /// Session `sid` claims report `tid` of queue `qid` for [`LEASE_SECS`]
    /// and becomes the thread's owner. A session that holds the live lease
    /// already gets it again, renewed; otherwise a new lease, with a fence
    /// larger than any before on the report. Recorded as `queue.claim
    /// {queue_id, thread_id, session, lease_id, fence, expires_at}`.
    ///
    /// # Errors
    /// `NotFound` for no such report of the queue, [`CoreError::NotAMember`]
    /// for no such queue; [`CoreError::ThreadTaken`] naming the holder when
    /// another session holds a live lease or owns the thread;
    /// `unknown_session` for a session that is not live.
    pub fn claim(&self, ctx: &AuditCtx, qid: &str, tid: &str, sid: &str) -> Result<Lease> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let expires_at = later(now_utc, LEASE_SECS);
        self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let aid = report_artifact_in(tx, qid, tid)?;
            live_session_in(tx, sid)?;
            settle_in(self, tx, tid, &now)?;
            let cur = current_in(tx, tid, &now)?;
            let (lease_id, fence) = match cur {
                Some(cur) if cur.live && cur.session_id == sid => {
                    tx.execute(
                        "UPDATE queue_claims SET expires_at = ?3 WHERE thread_id = ?1 AND session_id = ?2",
                        params![tid, sid, expires_at],
                    )?;
                    (cur.lease_id, cur.fence)
                }
                Some(cur) if cur.live => {
                    return Err(CoreError::ThreadTaken {
                        name: cur.holder_name,
                    });
                }
                cur => {
                    check_not_taken_in(tx, tid, sid)?;
                    let fence = cur.map_or(1, |c| c.fence + 1);
                    let lease_id = new_ulid();
                    tx.execute(
                        "INSERT INTO queue_claims (thread_id, session_id, queue_id, lease_id, fence, expires_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(thread_id, session_id) DO UPDATE SET lease_id = excluded.lease_id,
                            fence = excluded.fence, expires_at = excluded.expires_at,
                            queue_id = excluded.queue_id, released = 0",
                        params![tid, sid, qid, lease_id, fence, expires_at],
                    )?;
                    tx.execute(
                        "UPDATE threads SET target_session_id = ?2 WHERE id = ?1",
                        params![tid, sid],
                    )?;
                    self.agents_pinged();
                    (lease_id, fence)
                }
            };
            let mut rec = queue_record(AuditKind::QueueClaim, &now, qid)
                .with("thread_id", tid)
                .with("session", sid)
                .with("lease_id", lease_id.as_str())
                .with("fence", fence)
                .with("expires_at", expires_at.as_str());
            rec.ids.artifact = Some(aid);
            rec.ids.thread = Some(tid.to_string());
            rec.ids.session = Some(sid.to_string());
            self.record_audit(tx, ctx, rec)?;
            let report =
                report_in(tx, &q, tid, Visibility::All, &now)?.ok_or(CoreError::NotFound)?;
            Ok(Lease {
                lease_id,
                expires_at: expires_at.clone(),
                fence,
                report,
            })
        })
    }

    /// Extends session `sid`'s live lease on report `tid` of queue `qid` to
    /// [`LEASE_SECS`] from now, keeping its lease ID and fence. A
    /// heartbeat: it records nothing.
    ///
    /// # Errors
    /// [`Store::check_lease`]'s, and [`CoreError::LeaseLost`] when `sid`
    /// holds no live lease on it.
    pub fn renew(&self, qid: &str, tid: &str, sid: &str) -> Result<Lease> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let expires_at = later(now_utc, LEASE_SECS);
        // A lapsed lease's owner is cleared whatever this answers.
        self.check_lease(qid, tid, sid)?;
        self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            report_artifact_in(tx, qid, tid)?;
            check_lease_in(tx, tid, sid, &now)?;
            let cur = current_in(tx, tid, &now)?
                .filter(|c| c.live && c.session_id == sid)
                .ok_or(CoreError::LeaseLost)?;
            tx.execute(
                "UPDATE queue_claims SET expires_at = ?3 WHERE thread_id = ?1 AND session_id = ?2",
                params![tid, sid, expires_at],
            )?;
            let report =
                report_in(tx, &q, tid, Visibility::All, &now)?.ok_or(CoreError::NotFound)?;
            Ok(Lease {
                lease_id: cur.lease_id,
                expires_at: expires_at.clone(),
                fence: cur.fence,
                report,
            })
        })
    }

    /// Extends session `sid`'s live leases on the reports of `thread_ids`
    /// to [`LEASE_SECS`] from now (an agent's `working` on them renews
    /// them, spec §7.4); others are left as they are. Answers how many it
    /// extended; records nothing.
    ///
    /// # Errors
    /// Database errors.
    pub fn renew_session_leases(&self, sid: &str, thread_ids: &[String]) -> Result<usize> {
        if thread_ids.is_empty() {
            return Ok(0);
        }
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let expires_at = later(now_utc, LEASE_SECS);
        let ids = serde_json::to_string(thread_ids).expect("strings serialise");
        self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE queue_claims SET expires_at = ?3
                 WHERE session_id = ?1 AND expires_at > ?4 AND NOT released
                   AND thread_id IN (SELECT value FROM json_each(?2))
                   AND fence = (SELECT MAX(fence) FROM queue_claims m WHERE m.thread_id = queue_claims.thread_id)",
                params![sid, ids, expires_at, now],
            )?)
        })
    }

    /// Session `sid` releases its live lease on report `tid` of queue
    /// `qid`: the report is open again and the thread has no owner.
    /// Recorded as `queue.release {queue_id, thread_id, session, lease_id,
    /// reason: "explicit"}`.
    ///
    /// # Errors
    /// [`Store::check_lease`]'s, and [`CoreError::LeaseLost`] when `sid`
    /// holds no live lease on it.
    pub fn release(&self, ctx: &AuditCtx, qid: &str, tid: &str, sid: &str) -> Result<()> {
        let now = self.now();
        // A lapsed lease's owner is cleared whatever this answers.
        self.check_lease(qid, tid, sid)?;
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let aid = report_artifact_in(tx, qid, tid)?;
            check_lease_in(tx, tid, sid, &now)?;
            let cur = current_in(tx, tid, &now)?
                .filter(|c| c.live && c.session_id == sid)
                .ok_or(CoreError::LeaseLost)?;
            tx.execute(
                "UPDATE queue_claims SET released = 1 WHERE thread_id = ?1 AND session_id = ?2",
                params![tid, sid],
            )?;
            tx.execute(
                "UPDATE threads SET target_session_id = NULL WHERE id = ?1 AND target_session_id = ?2",
                params![tid, sid],
            )?;
            self.agents_pinged();
            let mut rec = queue_record(AuditKind::QueueRelease, &now, qid)
                .with("thread_id", tid)
                .with("session", sid)
                .with("lease_id", cur.lease_id.as_str())
                .with("reason", "explicit");
            rec.ids.artifact = Some(aid);
            rec.ids.thread = Some(tid.to_string());
            rec.ids.session = Some(sid.to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// `Ok` when session `sid` may write to report `tid` of queue `qid`
    /// (reply, resolve, metadata): it holds the report's live lease. An
    /// agent writes to a report only under its own claim; a person writes
    /// without one, and this check is not theirs. A lapsed lease's owner is
    /// cleared first.
    ///
    /// # Errors
    /// [`CoreError::LeaseLost`] when `sid` held a lease that expired or a
    /// later lease superseded; [`CoreError::ThreadTaken`] naming the holder
    /// when another session holds a live lease; [`CoreError::ClaimRequired`]
    /// when no session holds one and `sid` never claimed the report, or
    /// released its claim; `NotFound` for no such report of the queue.
    pub fn check_lease(&self, qid: &str, tid: &str, sid: &str) -> Result<()> {
        let now = self.now();
        // The settle commits whatever the check answers.
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            report_artifact_in(tx, qid, tid)?;
            settle_in(self, tx, tid, &now)?;
            Ok(check_lease_in(tx, tid, sid, &now))
        })?
    }

    /// Writes `value` as report `tid`'s metadata (queue `qid`) when its
    /// version is `if_version`, and answers the version it has after: one
    /// more, or the same when `value` equals what it holds (then nothing is
    /// written or recorded). Recorded as `thread.metadata {thread_id,
    /// metadata, metadata_sha256, previous_sha256}` under `ctx` (spec
    /// §6.6).
    ///
    /// # Errors
    /// `NotFound` for no such report of the queue; `metadata_too_large`
    /// over [`MAX_THREAD_METADATA_BYTES`]; [`CoreError::DocConflict`]
    /// (path `metadata`) when `if_version` is not its version.
    pub fn set_thread_metadata(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        value: serde_json::Value,
        if_version: u64,
    ) -> Result<u64> {
        self.set_thread_metadata_as(ctx, qid, tid, None, value, if_version)
    }

    /// [`Store::set_thread_metadata`] by `agent`'s session (`None`: a
    /// person), which must hold the report's live lease, checked in the
    /// write's transaction ([`Store::check_lease`]).
    ///
    /// # Errors
    /// [`Store::set_thread_metadata`]'s, and [`Store::check_lease`]'s for an
    /// agent.
    pub fn set_thread_metadata_as(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        agent: Option<&str>,
        value: serde_json::Value,
        if_version: u64,
    ) -> Result<u64> {
        let json = metadata_json(&value, MAX_THREAD_METADATA_BYTES)?;
        if let Some(sid) = agent {
            self.check_lease(qid, tid, sid)?;
        }
        let now = self.now();
        self.with_tx(|tx| {
            let aid = report_artifact_in(tx, qid, tid)?;
            if let Some(sid) = agent {
                check_lease_in(tx, tid, sid, &now)?;
            }
            let (before, version): (Option<String>, u64) = tx.query_row(
                "SELECT metadata_json, metadata_version FROM threads WHERE id = ?1",
                params![tid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if version != if_version {
                return Err(CoreError::DocConflict {
                    path: "metadata".into(),
                    current: Some(version),
                });
            }
            let before_json = before.as_deref().unwrap_or("null");
            if before_json == json {
                return Ok(version);
            }
            tx.execute(
                "UPDATE threads SET metadata_json = ?2, metadata_version = ?3 WHERE id = ?1",
                params![tid, json, version + 1],
            )?;
            let mut rec = AuditRecord::new(AuditKind::ThreadMetadata, self.now())
                .with("thread_id", tid)
                .with("metadata", value.clone())
                .with("metadata_sha256", metadata_sha256(&json))
                .with("previous_sha256", metadata_sha256(before_json));
            rec.ids.artifact = Some(aid);
            rec.ids.thread = Some(tid.to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(version + 1)
        })
    }

    /// Writes `value` as the metadata of report `tid`'s external artifact
    /// (queue `qid`) as [`Store::set_artifact_metadata`] does, by `agent`'s
    /// session (`None`: a person), which must hold the report's live lease,
    /// checked in the write's transaction.
    ///
    /// # Errors
    /// [`Store::set_artifact_metadata`]'s; `NotFound` for no such report of
    /// the queue; [`Store::check_lease`]'s for an agent.
    pub fn set_report_artifact_metadata(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        agent: Option<&str>,
        value: serde_json::Value,
        if_version: u64,
    ) -> Result<u64> {
        let json = metadata_json(&value, MAX_ARTIFACT_METADATA_BYTES)?;
        if let Some(sid) = agent {
            self.check_lease(qid, tid, sid)?;
        }
        let now = self.now();
        self.with_tx(|tx| {
            let aid = ArtifactId::parse(&report_artifact_in(tx, qid, tid)?)?;
            if let Some(sid) = agent {
                check_lease_in(tx, tid, sid, &now)?;
            }
            self.set_artifact_metadata_in(tx, ctx, &aid, &json, value, if_version)
        })
    }

    /// `by`'s reply `body` on report `tid` of queue `qid`, and the report
    /// after it. An agent's reply needs its session's live lease on the
    /// report, checked in the reply's transaction; it never reopens a
    /// resolved report. A person's needs no claim, and reopens one.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue, `NotFound` for no such
    /// report of it; [`Store::check_lease`]'s for an agent;
    /// `invalid_comment` for a bad body.
    pub fn report_reply(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: &ReportWriter<'_>,
        body: &str,
    ) -> Result<(Comment, Report)> {
        let comment = match by {
            ReportWriter::Agent(sess) => {
                // A lapsed lease's owner is cleared whatever this answers.
                self.check_lease(qid, tid, &sess.id)?;
                NewComment {
                    author_kind: AUTHOR_AGENT,
                    author_name: sess.harness.clone(),
                    author_public_id: None,
                    via_session_id: Some(sess.id.clone()),
                    body: body.to_string(),
                    via_page: false,
                }
            }
            ReportWriter::Person { name, public_id } => {
                self.report(qid, tid, Visibility::All)?
                    .ok_or(CoreError::NotFound)?;
                NewComment {
                    author_kind: AUTHOR_VIEWER,
                    author_name: name.clone(),
                    author_public_id: public_id.clone(),
                    via_session_id: None,
                    body: body.to_string(),
                    via_page: false,
                }
            }
        };
        let agent = by.session();
        let now = self.now();
        let c = self.add_comment_then(ctx, tid, comment, |tx, _, _| {
            report_artifact_in(tx, qid, tid)?;
            if let Some(sid) = agent {
                check_lease_in(tx, tid, sid, &now)?;
                check_not_taken_in(tx, tid, sid)?;
            }
            Ok(())
        })?;
        let report = self
            .report(qid, tid, Visibility::All)?
            .ok_or(CoreError::NotFound)?;
        Ok((c, report))
    }

    /// `by` resolves report `tid` of queue `qid` as
    /// [`Store::resolve_thread_touched`] does (`resolved_by`
    /// `agent:<harness>` with its session's person, or `viewer:<public ID>`),
    /// and answers the thread, what the resolve changed, and the report. An
    /// agent's resolve needs its session's live lease, checked in the
    /// resolve's transaction; a person's needs no claim.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue, `NotFound` for no such
    /// report of it; [`Store::check_lease`]'s for an agent.
    pub fn resolve_report(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: &ReportWriter<'_>,
    ) -> Result<(Thread, clax_core::feedback::Touched, Report)> {
        let (resolved_by, person) = match by {
            ReportWriter::Agent(sess) => {
                self.check_lease(qid, tid, &sess.id)?;
                (
                    format!("agent:{}", sess.harness),
                    Some(
                        sess.person
                            .clone()
                            .unwrap_or_else(|| clax_core::visibility::OWNER.to_string()),
                    ),
                )
            }
            ReportWriter::Person { public_id, .. } => (
                format!("viewer:{}", public_id.as_deref().unwrap_or("anonymous")),
                None,
            ),
        };
        let agent = by.session();
        let now = self.now();
        let touched = self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            report_artifact_in(tx, qid, tid)?;
            if let Some(sid) = agent {
                check_lease_in(tx, tid, sid, &now)?;
                check_not_taken_in(tx, tid, sid)?;
            }
            self.resolve_thread_in(tx, ctx, tid, &resolved_by, person.as_deref(), |_, _| {
                Ok(None)
            })
        })?;
        let thread = self.get_thread(tid)?.ok_or(CoreError::NotFound)?;
        let report = self
            .report(qid, tid, Visibility::All)?
            .ok_or(CoreError::NotFound)?;
        Ok((thread, touched, report))
    }

    /// Adds reporter `reporter_id`'s comment `body` to report `tid` of
    /// queue `qid`, as a viewer comment named by the reporter ID and
    /// carrying it (a resolved report reopens), and answers the report.
    ///
    /// # Errors
    /// `NotFound` for no such report of the queue, or one the reporter does
    /// not see; `invalid_comment` for a bad body.
    pub fn reporter_comment(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        reporter_id: &str,
        body: &str,
    ) -> Result<Report> {
        let vis = Visibility::Reporter(reporter_id);
        self.report(qid, tid, vis)?.ok_or(CoreError::NotFound)?;
        let rid = reporter_id.to_string();
        self.add_comment_then(
            ctx,
            tid,
            NewComment {
                author_kind: super::threads::AUTHOR_VIEWER,
                author_name: reporter_id.to_string(),
                author_public_id: None,
                via_session_id: None,
                body: body.to_string(),
                via_page: false,
            },
            move |tx, _, c| {
                tx.execute(
                    "UPDATE comments SET reporter_id = ?2 WHERE id = ?1",
                    params![c.id, rid],
                )?;
                Ok(())
            },
        )?;
        self.report(qid, tid, vis)?.ok_or(CoreError::NotFound)
    }
}

fn grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ServiceGrant> {
    Ok(ServiceGrant {
        id: r.get(0)?,
        queue_id: r.get(1)?,
        name: r.get(2)?,
        public_key: r.get(3)?,
        created_at: r.get(4)?,
    })
}

/// `Ok` when `sid` is a session that has not ended.
fn live_session_in(c: &Connection, sid: &str) -> Result<()> {
    let live: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND ended_at IS NULL)",
        params![sid],
        |r| r.get(0),
    )?;
    if live {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "unknown_session",
            "a claim comes from a live session",
        ))
    }
}

#[cfg(test)]
mod tests;
