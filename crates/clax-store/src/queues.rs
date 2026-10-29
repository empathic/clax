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
//!
//! A report also keeps what the feedback loop adds (spec
//! 2026-10-29-feedback-loop-design §5, §6): the context and diagnostics it
//! was filed with, the receiving server's stamp and the reporter's profile;
//! its fix attempts (`report_fixes`) and their verdicts; and its phase
//! (`threads.report_phase`), from which with the thread's status and lease
//! its [`State`] follows: a resolved thread is `verified` in phase
//! `verified` and `resolved` otherwise; an open one is `fix_proposed` in
//! phase `fix_proposed`, else `claimed` under a live lease, else `open`.
//! Sessions watch a queue (`queue_watches`) until they end.

use super::Store;
use super::external::{EXTERNAL_SELECT, NewExternal, row_to_external};
use super::threads::{
    AUTHOR_AGENT, AUTHOR_VIEWER, NewComment, NewThread, check_body, check_not_taken_in,
    clip_problem,
};
use clax_core::anchor::Anchor;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::feedback::{QueueEvent, Touched};
use clax_core::model::{Comment, Session, Thread};
use clax_core::queue::{
    AUTHOR_REPORTER, Claim, LEASE_SECS, Lease, MAX_ARTIFACT_METADATA_BYTES,
    MAX_THREAD_METADATA_BYTES, Queue, REPORT_PAGE, REPORTER_TOKEN_PREFIX, REPORTER_TOKEN_SECS,
    Report, ReportComment, ReportOrder, ReportersSee, Role, SERVICE_TOKEN_PREFIX, State,
    StateFilter, check_queue_name, check_reporter_id, metadata_json, metadata_sha256,
    normalize_origin,
};
use clax_core::report::{
    DiagLine, FixAttempt, FixOutcome, NewFix, ReportContext, ReporterProfile, ResolveReason,
    ServerStamp,
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
    /// Who the reporter is, as the minting backend asserted it; each
    /// report filed under the token keeps it.
    pub profile: Option<ReporterProfile>,
}

/// A report to file ([`Store::file_report_touched_async`]).
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
    /// What the reporter's browser said at capture, kept as filed.
    pub context: Option<ReportContext>,
    /// The receiving server's stamp.
    pub server: Option<ServerStamp>,
    /// The diagnostics filed beside the clip (an empty list is none).
    pub diagnostics: Option<Vec<DiagLine>>,
}

/// Who gives a verdict on a fix, or reopens a report
/// ([`Store::verify_report`], [`Store::reopen_report`]): its reporter, or
/// a triage person on the reporter's behalf. Agents give none, so no agent
/// marks its own fix verified.
#[derive(Clone, Debug)]
pub enum Verdict<'a> {
    /// The reporter that filed the report, by its reporter ID.
    Reporter(&'a str),
    /// A triage member themself; `public_id` is their viewer's, `None`
    /// writes as anonymous.
    Person {
        name: String,
        public_id: Option<String>,
    },
}

impl Verdict<'_> {
    /// Whose reads the verdict's answer is: the reporter's, or a member's.
    fn visibility(&self) -> Visibility<'_> {
        match self {
            Verdict::Reporter(r) => Visibility::Reporter(r),
            Verdict::Person { .. } => Visibility::All,
        }
    }

    /// The `resolved_by` of a verify.
    fn resolved_by(&self) -> String {
        match self {
            Verdict::Reporter(r) => format!("reporter:{r}"),
            Verdict::Person { public_id, .. } => {
                format!("viewer:{}", public_id.as_deref().unwrap_or("anonymous"))
            }
        }
    }
}

/// What waits for a reporter ([`Store::reporter_badge`]): its reports with
/// a fix waiting for its verdict, or with a reply it has not read, oldest
/// first.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Badge {
    /// How many reports wait (`report_ids.len()`).
    pub waiting: u32,
    pub report_ids: Vec<String>,
}

/// Whose external IDs a filed report's ID is among (spec §6.1): one
/// deployer's. On a local or self-hosted Clax the deployer is the
/// instance; on a hosted one each account is its own deployer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalScope {
    /// The whole instance: one ID is one artifact for every queue.
    Instance,
    /// The queue maker's account: queues of one account share an ID's
    /// artifact, and another account's same ID names another artifact.
    Maker,
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
    t.metadata_json, t.metadata_version, t.has_clip, t.created_at,
    t.report_context_json, t.report_server_json, t.reporter_profile_json, t.report_phase,
    t.resolve_reason, t.reopen_count,
    (SELECT n FROM report_diagnostics WHERE thread_id = t.id ORDER BY capture DESC LIMIT 1),
    (SELECT n FROM report_diagnostics WHERE thread_id = t.id AND capture = 0)
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

/// Whether live session `sid` acts for a member of live queue `qid` (its
/// person, or the owner, has a role there, the maker's being admin).
pub(crate) fn session_is_member_in(c: &Connection, qid: &str, sid: &str) -> Result<bool> {
    let Some(q) = queue_in(c, qid)? else {
        return Ok(false);
    };
    let person: Option<Option<String>> = c
        .query_row(
            "SELECT person FROM sessions WHERE id = ?1 AND ended_at IS NULL",
            params![sid],
            |r| r.get(0),
        )
        .optional()?;
    match person {
        Some(p) => Ok(role_in(c, &q, p.as_deref())?.is_some()),
        None => Ok(false),
    }
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

/// The comment a person's release of a claim posts when they give no
/// note ([`Store::release_as_person`]).
pub const RELEASED_BODY: &str = "Released the claim: the report is open again.";

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
    context_json: Option<String>,
    server_json: Option<String>,
    profile_json: Option<String>,
    phase: Option<String>,
    resolve_reason: Option<String>,
    reopen_count: u32,
    diagnostics_lines: Option<u32>,
    filing_diagnostics_lines: Option<u32>,
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
        context_json: r.get(9)?,
        server_json: r.get(10)?,
        profile_json: r.get(11)?,
        phase: r.get(12)?,
        resolve_reason: r.get(13)?,
        reopen_count: r.get(14)?,
        diagnostics_lines: r.get(15)?,
        filing_diagnostics_lines: r.get(16)?,
    })
}

/// A report's state (see the module documentation): from its thread's
/// `status`, its `phase` and whether a live lease holds it.
fn state_of(status: &str, phase: Option<&str>, claimed: bool) -> State {
    match (status, phase) {
        ("resolved", Some("verified")) => State::Verified,
        ("resolved", _) => State::Resolved,
        (_, Some("fix_proposed")) => State::FixProposed,
        _ if claimed => State::Claimed,
        _ => State::Open,
    }
}

/// The value of the JSON `json` the store wrote in `column` of artifact
/// `aid`'s report; `None` for none.
///
/// # Errors
/// [`CoreError::Corrupt`] when it is not one of `T`.
fn from_json<T: serde::de::DeserializeOwned>(
    json: Option<&str>,
    aid: &str,
    column: &'static str,
) -> Result<Option<T>> {
    json.map(|j| {
        serde_json::from_str(j).map_err(|_| CoreError::Corrupt {
            artifact_id: aid.to_string(),
            column,
            version: None,
        })
    })
    .transpose()
}

/// Report `tid`'s fix attempts on `c`, oldest first; `aid` is its
/// artifact.
fn fixes_in(c: &Connection, aid: &str, tid: &str) -> Result<Vec<FixAttempt>> {
    let rows: Vec<FixRow> = c
        .prepare_cached(
            "SELECT n, fix_json, proposer_name, created_at, outcome, verdict_note,
                verdict_context_json, verdict_at
             FROM report_fixes WHERE thread_id = ?1 ORDER BY n",
        )?
        .query_map(params![tid], |r| {
            Ok(FixRow {
                n: r.get(0)?,
                fix_json: r.get(1)?,
                proposer_name: r.get(2)?,
                created_at: r.get(3)?,
                outcome: r.get(4)?,
                verdict_note: r.get(5)?,
                verdict_context_json: r.get(6)?,
                verdict_at: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.into_iter()
        .map(|f| {
            let fix: NewFix =
                serde_json::from_str(&f.fix_json).map_err(|_| CoreError::Corrupt {
                    artifact_id: aid.to_string(),
                    column: "report_fixes.fix_json",
                    version: None,
                })?;
            Ok(FixAttempt {
                n: f.n,
                fix,
                proposer_name: f.proposer_name,
                created_at: f.created_at,
                outcome: FixOutcome::parse(&f.outcome).unwrap_or(FixOutcome::Pending),
                verdict_note: f.verdict_note,
                verdict_context: from_json(
                    f.verdict_context_json.as_deref(),
                    aid,
                    "report_fixes.verdict_context_json",
                )?,
                verdict_at: f.verdict_at,
            })
        })
        .collect()
}

struct FixRow {
    n: u32,
    fix_json: String,
    proposer_name: String,
    created_at: String,
    outcome: String,
    verdict_note: Option<String>,
    verdict_context_json: Option<String>,
    verdict_at: Option<String>,
}

/// The report views of `rows` as `vis` reads them, on `c` at `now`: the
/// reporter's profile only for a member ([`Visibility::All`]), never a
/// reporter; the filing's context and each verdict's context only for a
/// member and for the reporter that filed the report, never another
/// reporter (`reporters_see: all`), as its diagnostics are.
fn reports_of(
    c: &Connection,
    rows: Vec<ReportRow>,
    now: &str,
    vis: Visibility<'_>,
) -> Result<Vec<Report>> {
    let with_profile = vis == Visibility::All;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let own = match vis {
            Visibility::All => true,
            Visibility::Reporter(r) => row.reporter_id.as_deref() == Some(r),
        };
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
                "SELECT c.id, c.author_kind, c.author_name, c.body, c.created_at, c.reporter_id,
                    c.report_capture, c.clip_key IS NOT NULL,
                    (SELECT d.n FROM report_diagnostics d
                     WHERE d.thread_id = c.thread_id AND d.capture = c.report_capture),
                    c.triage_only
                 FROM comments c WHERE c.thread_id = ?1 AND (?2 OR NOT c.triage_only)
                 ORDER BY c.created_at, c.id",
            )?
            .query_map(params![row.id, with_profile], |r| {
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
                    capture: r.get(6)?,
                    has_clip: r.get(7)?,
                    diagnostics_lines: r.get::<_, Option<u32>>(8)?.unwrap_or(0),
                    triage_only: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let claim = current_in(c, &row.id, now)?
            .filter(|cur| cur.live)
            .map(|cur| Claim {
                holder_name: cur.holder_name,
                expires_at: cur.expires_at,
            });
        let state = state_of(&row.status, row.phase.as_deref(), claim.is_some());
        let mut fixes = fixes_in(c, &artifact.artifact_id, &row.id)?;
        if !own {
            for f in &mut fixes {
                f.verdict_context = None;
            }
        }
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
            context: if own {
                from_json(
                    row.context_json.as_deref(),
                    &artifact.artifact_id,
                    "threads.report_context_json",
                )?
            } else {
                None
            },
            server: from_json(
                row.server_json.as_deref(),
                &artifact.artifact_id,
                "threads.report_server_json",
            )?,
            reporter_profile: if with_profile {
                from_json(
                    row.profile_json.as_deref(),
                    &artifact.artifact_id,
                    "threads.reporter_profile_json",
                )?
            } else {
                None
            },
            resolve_reason: row.resolve_reason.as_deref().and_then(ResolveReason::parse),
            fixes,
            reopen_count: row.reopen_count,
            diagnostics_lines: row.diagnostics_lines.unwrap_or(0),
            filing_diagnostics_lines: row.filing_diagnostics_lines.unwrap_or(0),
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
    Ok(reports_of(c, row.into_iter().collect(), now, vis)?.pop())
}

/// Whether report `tid` of queue `q` is one `vis` sees, on `c`.
fn sees_in(c: &Connection, q: &QueueRow, tid: &str, vis: Visibility<'_>) -> Result<bool> {
    let (filter, reporter) = reporter_filter(q, vis);
    // `reporter_filter`'s SQL names `?4`, so `?2` (the time, which
    // REPORT_SELECT's callers bind) is bound to NULL and tested, keeping
    // the parameters numbered as there.
    Ok(c.prepare_cached(&format!(
        "SELECT EXISTS(SELECT 1 FROM threads t JOIN artifacts a ON a.id = t.artifact_id
            JOIN external_artifacts x ON x.artifact_id = t.artifact_id
            WHERE a.deleted_at IS NULL AND t.queue_id = ?1 AND t.id = ?3 AND ?2 IS NULL {filter})"
    ))?
    .query_row(params![q.id, Option::<&str>::None, tid, reporter], |r| {
        r.get(0)
    })?)
}

/// A new content-store key for a clip of a comment on thread `tid`: a
/// name of its own, so no capture's clip replaces another's.
fn comment_clip_key(tid: &str) -> String {
    format!("{tid}-{}", new_ulid())
}

/// Report `tid`'s thread status, phase, reporter and artifact on `c`.
struct Standing {
    status: String,
    phase: Option<String>,
    reporter_id: Option<String>,
    artifact_id: String,
}

fn standing_in(c: &Connection, tid: &str) -> Result<Standing> {
    c.query_row(
        "SELECT status, report_phase, reporter_id, artifact_id FROM threads WHERE id = ?1",
        params![tid],
        |r| {
            Ok(Standing {
                status: r.get(0)?,
                phase: r.get(1)?,
                reporter_id: r.get(2)?,
                artifact_id: r.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or(CoreError::NotFound)
}

/// Report `tid`'s standing, when `by` may give a verdict on it in queue
/// `q`: a member always may; a reporter only on a report it filed and
/// sees.
///
/// # Errors
/// `NotFound` otherwise, and for no such report of the queue.
fn check_verdict_in(c: &Connection, q: &QueueRow, tid: &str, by: &Verdict<'_>) -> Result<Standing> {
    if !sees_in(c, q, tid, by.visibility())? {
        return Err(CoreError::NotFound);
    }
    let s = standing_in(c, tid)?;
    if let Verdict::Reporter(r) = by
        && s.reporter_id.as_deref() != Some(*r)
    {
        return Err(CoreError::NotFound);
    }
    Ok(s)
}

/// Report `tid`'s diagnostics as stored, with their count, when `lines`
/// holds a line; checked as the diagnostics part of a filing is
/// ([`clax_core::report::parse_diagnostics`]), each text scrubbed again
/// and a line the shape does not admit dropped, with a note stamped `now`.
fn diagnostics_json(lines: &[DiagLine], now: &str) -> Result<Option<(String, u32)>> {
    if lines.is_empty() {
        return Ok(None);
    }
    let mut part = String::new();
    for l in lines {
        part.push_str(&serde_json::to_string(l).expect("diagnostics serialise"));
        part.push('\n');
    }
    let checked = clax_core::report::parse_diagnostics(part.as_bytes(), now)?;
    let json = serde_json::to_string(&checked).expect("diagnostics serialise");
    Ok(Some((json, checked.len() as u32)))
}

/// `ctx` as stored: checked and scrubbed as a filed context is
/// ([`clax_core::report::parse_context`]).
fn context_json(ctx: &ReportContext) -> Result<String> {
    let checked =
        clax_core::report::parse_context(&serde_json::to_string(ctx).expect("contexts serialise"))?;
    Ok(serde_json::to_string(&checked).expect("contexts serialise"))
}

/// Ends report `tid`'s current lease when it is live at `now`, as
/// [`Store::release`] does, recorded as `queue.release {queue_id,
/// thread_id, session, lease_id, reason}`.
#[allow(clippy::too_many_arguments)]
fn end_lease_in(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    qid: &str,
    tid: &str,
    aid: &str,
    now: &str,
    reason: &str,
) -> Result<()> {
    let Some(cur) = current_in(tx, tid, now)?.filter(|c| c.live) else {
        return Ok(());
    };
    tx.execute(
        "UPDATE queue_claims SET released = 1 WHERE thread_id = ?1 AND session_id = ?2",
        params![tid, cur.session_id],
    )?;
    tx.execute(
        "UPDATE threads SET target_session_id = NULL WHERE id = ?1 AND target_session_id = ?2",
        params![tid, cur.session_id],
    )?;
    st.agents_pinged();
    let mut rec = queue_record(AuditKind::QueueRelease, now, qid)
        .with("thread_id", tid)
        .with("session", cur.session_id.as_str())
        .with("lease_id", cur.lease_id.as_str())
        .with("reason", reason);
    rec.ids.artifact = Some(aid.to_string());
    rec.ids.thread = Some(tid.to_string());
    rec.ids.session = Some(cur.session_id);
    st.record_audit(tx, ctx, rec)?;
    Ok(())
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
    /// queue's maker is its admin whatever this says. A removal releases
    /// every live lease the person's sessions hold on the queue's reports,
    /// each recorded as `queue.release {queue_id, thread_id, session,
    /// lease_id, reason: "member_removed"}`, clears those threads'
    /// owner, and ends the person's sessions' watches of the queue.
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
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
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
            if role.is_none() && q.person.as_deref() != Some(person) {
                self.release_person_in(tx, ctx, qid, person, &now)?;
                // Its sessions no longer watch the queue.
                tx.execute(
                    "DELETE FROM queue_watches WHERE queue_id = ?1
                       AND session_id IN (SELECT id FROM sessions WHERE person = ?2)",
                    params![qid, person],
                )?;
            }
            Ok(())
        })
    }

    /// Releases every unreleased lease the sessions of `person` hold on
    /// queue `qid`'s reports, clearing the owner of each thread they own.
    fn release_person_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        qid: &str,
        person: &str,
        now: &str,
    ) -> Result<()> {
        let held: Vec<(String, String, String, String)> = tx
            .prepare(
                "SELECT c.thread_id, c.session_id, c.lease_id, t.artifact_id
                 FROM queue_claims c JOIN sessions s ON s.id = c.session_id
                 JOIN threads t ON t.id = c.thread_id
                 WHERE c.queue_id = ?1 AND s.person = ?2 AND NOT c.released
                 ORDER BY c.thread_id, c.session_id",
            )?
            .query_map(params![qid, person], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        for (tid, sid, lease_id, aid) in &held {
            tx.execute(
                "UPDATE queue_claims SET released = 1 WHERE thread_id = ?1 AND session_id = ?2",
                params![tid, sid],
            )?;
            tx.execute(
                "UPDATE threads SET target_session_id = NULL WHERE id = ?1 AND target_session_id = ?2",
                params![tid, sid],
            )?;
            let mut rec = queue_record(AuditKind::QueueRelease, now, qid)
                .with("thread_id", tid.as_str())
                .with("session", sid.as_str())
                .with("lease_id", lease_id.as_str())
                .with("reason", "member_removed");
            rec.ids.artifact = Some(aid.clone());
            rec.ids.thread = Some(tid.clone());
            rec.ids.session = Some(sid.clone());
            self.record_audit(tx, ctx, rec)?;
        }
        if !held.is_empty() {
            self.agents_pinged();
        }
        Ok(())
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
    /// [`REPORTER_TOKEN_SECS`], carrying the reporter's `profile` (an
    /// empty one is none). Answers the token, kept only as its SHA-256,
    /// and its expiry. A token minting changes no history and records
    /// nothing. Tokens that expired a day before are dropped.
    ///
    /// # Errors
    /// [`CoreError::WrongOrigin`] for an origin the queue does not list;
    /// `invalid_origin`, `invalid_reporter_id`, `invalid_key` or
    /// [`CoreError::InvalidProfile`] for bad input; `NotFound` when the
    /// grant or its queue is gone.
    pub fn mint_reporter_token(
        &self,
        grant: &ServiceGrant,
        reporter_id: &str,
        origin: &str,
        public_key: &str,
        profile: Option<&ReporterProfile>,
    ) -> Result<(Secret, String)> {
        check_reporter_id(reporter_id)?;
        let origin = normalize_origin(origin)?;
        clax_core::sig::decode_public(public_key)
            .map_err(|_| CoreError::invalid("invalid_key", "public_key is not an Ed25519 key"))?;
        let profile = profile.filter(|p| !p.is_empty());
        if let Some(p) = profile {
            p.check()?;
        }
        let profile_json = profile.map(|p| serde_json::to_string(p).expect("profiles serialise"));
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
                    reporter_id, origin, public_key, expires_at, profile_json)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    token.sha256_hex(),
                    grant.queue_id,
                    grant.id,
                    reporter_id,
                    origin,
                    public_key,
                    expires_at,
                    profile_json
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
                "SELECT r.queue_id, r.service_grant_id, r.reporter_id, r.origin, r.public_key, r.expires_at,
                    r.profile_json
                 FROM queue_reporter_tokens r
                 JOIN queue_service_grants g ON g.id = r.service_grant_id
                 JOIN queues q ON q.id = r.queue_id
                 WHERE r.token_sha256 = ?1 AND g.revoked_at IS NULL AND q.deleted_at IS NULL",
            )?
            .query_row(params![sha256], |r| {
                Ok((
                    ReporterToken {
                        queue_id: r.get(0)?,
                        service_grant_id: r.get(1)?,
                        reporter_id: r.get(2)?,
                        origin: r.get(3)?,
                        public_key: r.get(4)?,
                        expires_at: r.get(5)?,
                        profile: None,
                    },
                    r.get::<_, Option<String>>(6)?,
                ))
            })
            .optional()?)
        })?;
        let found = match found {
            Some((mut t, profile)) => {
                t.profile = profile
                    .map(|j| {
                        serde_json::from_str(&j).map_err(|_| CoreError::Corrupt {
                            artifact_id: t.queue_id.clone(),
                            column: "queue_reporter_tokens.profile_json",
                            version: None,
                        })
                    })
                    .transpose()?;
                Some(t)
            }
            None => None,
        };
        match found {
            Some(t) if t.expires_at <= now => Err(CoreError::TokenExpired),
            other => Ok(other),
        }
    }

    /// [`Store::file_report_touched_async`], blocking on its content store
    /// and dropping the sessions it woke: for a caller with no waits to
    /// wake (tests and tools without a daemon), whose wake rows are still
    /// written and taken at the sessions' next poll.
    ///
    /// # Errors
    /// [`Store::file_report_touched_async`]'s.
    pub fn file_report(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        reporter: Option<&ReporterToken>,
        scope: ExternalScope,
        r: NewReport,
    ) -> Result<Report> {
        self.block_on(self.file_report_touched_async(ctx, qid, reporter, scope, r))
            .map(|(report, _)| report)
    }

    /// Files report `r` to queue `qid` by `reporter` (the reporter token
    /// it came with; `None` otherwise). Finds or makes the external
    /// artifact `r.external_id` names, or the page URL without its route
    /// when it names none, among the external IDs of `scope`
    /// ([`Store::ensure_external_async`], as the queue's maker's), then makes the thread on its current version with the
    /// queue, the reporter and its token's profile, the metadata, the
    /// context, the server stamp and the diagnostics in the thread's
    /// transaction (`thread.open`, `comment.add`, and `thread.metadata`
    /// for metadata that is not null). The context and diagnostics are
    /// checked and scrubbed again as a filing's are
    /// ([`clax_core::report::parse_context`],
    /// [`clax_core::report::parse_diagnostics`]). The anchor's file is
    /// the artifact's one page, and a route it names is dropped: an
    /// external artifact is no live page. In the same transaction every
    /// live session watching the queue is woken for the new report
    /// ([`Store::notify_queue_event_in`], `filed`). Answers the report as
    /// its filer reads it (a reporter's view carries no profile), and the
    /// sessions woken, to wake their waits with.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue, or a token of another
    /// queue; `invalid_clip`, `metadata_too_large`, `invalid_reporter_id`,
    /// [`CoreError::InvalidContext`], [`CoreError::InvalidDiagnostics`]
    /// and the errors of [`Store::ensure_external_async`] and
    /// [`Store::create_thread`] for bad input.
    pub async fn file_report_touched_async(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        reporter: Option<&ReporterToken>,
        scope: ExternalScope,
        r: NewReport,
    ) -> Result<(Report, Touched)> {
        let q = self.queue(qid)?.ok_or(CoreError::NotAMember)?;
        if let Some(tok) = reporter {
            check_reporter_id(&tok.reporter_id)?;
            if tok.queue_id != qid {
                return Err(CoreError::NotAMember);
            }
        }
        if let Some(problem) = r.clip.as_deref().and_then(clip_problem) {
            return Err(CoreError::invalid("invalid_clip", problem));
        }
        let context = r.context.as_ref().map(context_json).transpose()?;
        let server = r
            .server
            .as_ref()
            .map(|s| serde_json::to_string(s).expect("stamps serialise"));
        let diagnostics = match r.diagnostics.as_deref() {
            Some(lines) => diagnostics_json(lines, &self.now())?,
            None => None,
        };
        let profile = reporter
            .and_then(|t| t.profile.as_ref())
            .filter(|p| !p.is_empty())
            .map(|p| serde_json::to_string(p).expect("profiles serialise"));
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
                    namespace: match scope {
                        ExternalScope::Instance => String::new(),
                        ExternalScope::Maker => q.person.clone().unwrap_or_default(),
                    },
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
        let reporter_owned = reporter.map(|t| t.reporter_id.clone());
        let metadata = r.metadata;
        let now = self.now();
        let artifact_id = x.artifact_id.clone();
        let mut woken = Touched::default();
        let woken_in = &mut woken;
        let thread = self
            .create_thread_then_async(ctx, &aid, t, move |tx, tid| {
                tx.execute(
                    "UPDATE threads SET queue_id = ?2, reporter_id = ?3, metadata_json = ?4,
                        metadata_version = 1, report_context_json = ?5, report_server_json = ?6,
                        reporter_profile_json = ?7 WHERE id = ?1",
                    params![
                        tid,
                        qid_owned,
                        reporter_owned,
                        meta,
                        context,
                        server,
                        profile
                    ],
                )?;
                if let Some((lines, n)) = &diagnostics {
                    tx.execute(
                        "INSERT INTO report_diagnostics (thread_id, capture, lines_json, n)
                         VALUES (?1, 0, ?2, ?3)",
                        params![tid, lines, n],
                    )?;
                }
                if reporter_owned.is_some() {
                    tx.execute(
                        "UPDATE comments SET reporter_id = ?2 WHERE thread_id = ?1",
                        params![tid, reporter_owned],
                    )?;
                }
                // The metadata it is filed with is journaled as its first
                // write (spec §6.6), so a later write's `previous_sha256`
                // names it.
                let records = match &meta {
                    None => Vec::new(),
                    Some(json) => {
                        let mut rec = AuditRecord::new(AuditKind::ThreadMetadata, now)
                            .with("thread_id", tid)
                            .with("metadata", metadata)
                            .with("metadata_sha256", metadata_sha256(json))
                            .with("previous_sha256", metadata_sha256("null"));
                        rec.ids.artifact = Some(artifact_id);
                        rec.ids.thread = Some(tid.to_string());
                        vec![rec]
                    }
                };
                let note: String = tx.query_row(
                    "SELECT id FROM comments WHERE thread_id = ?1 ORDER BY created_at, id LIMIT 1",
                    params![tid],
                    |r| r.get(0),
                )?;
                *woken_in = self.notify_queue_event_in(
                    tx,
                    &qid_owned,
                    tid,
                    &note,
                    QueueEvent::Filed,
                    None,
                )?;
                Ok((None, records))
            })
            .await?;
        let vis = match reporter {
            Some(t) => Visibility::Reporter(&t.reporter_id),
            None => Visibility::All,
        };
        let report = self
            .report(qid, &thread.id, vis)?
            .ok_or(CoreError::NotFound)?;
        Ok((report, woken))
    }

    /// A page of queue `qid`'s reports that `vis` sees, in order of arrival,
    /// after report `cursor` (`None`: from the first), that `state` passes
    /// (`None`: any), at most [`REPORT_PAGE`]; and the cursor of the next
    /// page when there is one.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `invalid_cursor` when
    /// `cursor` is no report of the queue.
    pub fn reports(
        &self,
        qid: &str,
        vis: Visibility<'_>,
        state: Option<StateFilter>,
        cursor: Option<&str>,
    ) -> Result<(Vec<Report>, Option<String>)> {
        self.reports_in_order(qid, vis, state, cursor, ReportOrder::Oldest)
    }

    /// [`Store::reports`] in `order`: with [`ReportOrder::Newest`] the
    /// latest filed first, each page after `cursor` holding the reports
    /// filed before it.
    ///
    /// # Errors
    /// As [`Store::reports`].
    pub fn reports_in_order(
        &self,
        qid: &str,
        vis: Visibility<'_>,
        state: Option<StateFilter>,
        cursor: Option<&str>,
        order: ReportOrder,
    ) -> Result<(Vec<Report>, Option<String>)> {
        let now = self.now();
        self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            if let Some(cur) = cursor {
                let known: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM threads WHERE id = ?1 AND queue_id = ?2)",
                    params![cur, qid],
                    |r| r.get(0),
                )?;
                if !known {
                    return Err(CoreError::invalid(
                        "invalid_cursor",
                        format!("{cur} is not a report of queue {qid}"),
                    ));
                }
            }
            let (filter, reporter) = reporter_filter(&q, vis);
            // As `state_of` reads a report's state.
            const UNFIXED: &str = "t.status = 'open' AND t.report_phase IS NOT 'fix_proposed'";
            let state_sql = match state {
                None => String::new(),
                Some(StateFilter::Active) => format!("AND {UNFIXED}"),
                Some(StateFilter::One(s)) => match s {
                    State::Open => format!("AND {UNFIXED} AND NOT {LIVE_CLAIM}"),
                    State::Claimed => format!("AND {UNFIXED} AND {LIVE_CLAIM}"),
                    State::FixProposed => {
                        "AND t.status = 'open' AND t.report_phase = 'fix_proposed'".into()
                    }
                    State::Verified => {
                        "AND t.status = 'resolved' AND t.report_phase = 'verified'".into()
                    }
                    State::Resolved => {
                        "AND t.status = 'resolved' AND t.report_phase IS NOT 'verified'".into()
                    }
                },
            };
            let (after, dir) = match order {
                ReportOrder::Oldest => (">", "ASC"),
                ReportOrder::Newest => ("<", "DESC"),
            };
            let sql = format!(
                "{REPORT_SELECT} {filter} {state_sql}
                 AND (?3 IS NULL OR (t.created_at, t.id) {after} (SELECT created_at, id FROM threads WHERE id = ?3))
                 ORDER BY t.created_at {dir}, t.id {dir} LIMIT ?5"
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
            Ok((reports_of(c, rows, &now, vis)?, next))
        })
    }

    /// The PNG clip of report `tid` in queue `qid`, as `vis` may see it:
    /// `None` when the report has no clip; `NotFound` when `vis` does not
    /// see the report.
    pub async fn report_clip_async(
        &self,
        qid: &str,
        tid: &str,
        vis: Visibility<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let r = self.report(qid, tid, vis)?.ok_or(CoreError::NotFound)?;
        if !r.has_clip {
            return Ok(None);
        }
        let aid = ArtifactId::parse(&r.artifact.id)?;
        self.read_clip_async(&aid, tid).await
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
    /// [`CoreError::NotClaimable`] while a fix waits for its verdict
    /// (`fix_proposed`, until it is reopened); `unknown_session` for a
    /// session that is not live.
    pub fn claim(&self, ctx: &AuditCtx, qid: &str, tid: &str, sid: &str) -> Result<Lease> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let expires_at = later(now_utc, LEASE_SECS);
        self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let aid = report_artifact_in(tx, qid, tid)?;
            let s = standing_in(tx, tid)?;
            if state_of(&s.status, s.phase.as_deref(), false) == State::FixProposed {
                return Err(CoreError::NotClaimable);
            }
            live_session_in(tx, sid, "a claim comes from a live session")?;
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
    /// them, spec §7.4), where the session's person (`None`: the owner)
    /// still has a role on the lease's queue; others are left as they are.
    /// Answers how many it extended; records nothing.
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
                   AND fence = (SELECT MAX(fence) FROM queue_claims m WHERE m.thread_id = queue_claims.thread_id)
                   AND EXISTS (SELECT 1 FROM sessions s JOIN queues q ON q.id = queue_claims.queue_id
                     WHERE s.id = ?1 AND q.deleted_at IS NULL
                       AND (q.person IS s.person
                            OR EXISTS (SELECT 1 FROM queue_members qm
                                       WHERE qm.queue_id = q.id AND qm.person = s.person)))",
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

    /// A triage person (`name`, `public_id`) ends the live lease on report
    /// `tid` of queue `qid`, whoever holds it: the report is open again,
    /// the thread has no owner, and the holder's next renew, write or fix
    /// is [`CoreError::LeaseLost`] (its lease ends at once, as one that
    /// lapsed; a new claim takes a larger fence). The person's `note` (or,
    /// blank or absent, [`RELEASED_BODY`]) is posted as their comment, a
    /// note for the queue's members alone (`triage_only`: a reporter's view
    /// leaves it out and its badge does not count it), and the holder's session
    /// is woken with it (`released`, [`Store::wake_session_in`]) while it
    /// is live and acts for a member. Recorded as `queue.release
    /// {queue_id, thread_id, session, lease_id, reason: "person"}` under
    /// `ctx`, the person's. Answers the report and the sessions woken, or
    /// for a report no live lease holds the report as it is and `None`:
    /// nothing is written.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue, `NotFound` for no such
    /// report of it; `invalid_comment` for a note too long.
    pub fn release_as_person(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        name: &str,
        public_id: Option<&str>,
        note: Option<String>,
    ) -> Result<(Report, Option<Touched>)> {
        let body = note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(RELEASED_BODY)
            .to_string();
        let now = self.now();
        let woken = self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let aid = report_artifact_in(tx, qid, tid)?;
            settle_in(self, tx, tid, &now)?;
            let Some(cur) = current_in(tx, tid, &now)?.filter(|c| c.live) else {
                return Ok(None);
            };
            // The lease lapses now, unreleased: its holder's next check is
            // lease_lost, as for a lapsed or superseded lease.
            tx.execute(
                "UPDATE queue_claims SET expires_at = ?3 WHERE thread_id = ?1 AND session_id = ?2",
                params![tid, cur.session_id, now],
            )?;
            tx.execute(
                "UPDATE threads SET target_session_id = NULL WHERE id = ?1 AND target_session_id = ?2",
                params![tid, cur.session_id],
            )?;
            self.agents_pinged();
            let (c, _) = self.add_comment_in(
                tx,
                ctx,
                tid,
                NewComment {
                    author_kind: AUTHOR_VIEWER,
                    author_name: name.to_string(),
                    author_public_id: public_id.map(str::to_string),
                    via_session_id: None,
                    body,
                    via_page: false,
                },
            )?;
            tx.execute(
                "UPDATE comments SET triage_only = 1 WHERE id = ?1",
                params![c.id],
            )?;
            let mut rec = queue_record(AuditKind::QueueRelease, &now, qid)
                .with("thread_id", tid)
                .with("session", cur.session_id.as_str())
                .with("lease_id", cur.lease_id.as_str())
                .with("reason", "person");
            rec.ids.artifact = Some(aid);
            rec.ids.thread = Some(tid.to_string());
            rec.ids.session = Some(cur.session_id.clone());
            self.record_audit(tx, ctx, rec)?;
            self.wake_session_in(tx, qid, tid, &c.id, QueueEvent::Released, &cur.session_id)
                .map(Some)
        })?;
        let report = self
            .report(qid, tid, Visibility::All)?
            .ok_or(CoreError::NotFound)?;
        Ok((report, woken))
    }

    /// What a change to report `tid` of queue `qid` tells the queue's
    /// topic: the report's state now, and the recipient keys of the
    /// queue's members (empty for the owner, a connected person's public
    /// ID), its maker first; `None` for no such live queue or report.
    ///
    /// # Errors
    /// Database errors.
    pub fn report_change(&self, qid: &str, tid: &str) -> Result<Option<(State, Vec<String>)>> {
        let now = self.now();
        self.with_read(|c| {
            let Some(q) = queue_in(c, qid)? else {
                return Ok(None);
            };
            match report_artifact_in(c, qid, tid) {
                Ok(_) => {}
                Err(CoreError::NotFound) => return Ok(None),
                Err(e) => return Err(e),
            }
            let s = standing_in(c, tid)?;
            let claimed = current_in(c, tid, &now)?.is_some_and(|cur| cur.live);
            let state = state_of(&s.status, s.phase.as_deref(), claimed);
            let mut members = vec![q.person.clone().unwrap_or_default()];
            let rows: Vec<String> = c
                .prepare_cached(
                    "SELECT person FROM queue_members WHERE queue_id = ?1 ORDER BY created_at, person",
                )?
                .query_map(params![qid], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for p in rows {
                if !members.contains(&p) {
                    members.push(p);
                }
            }
            Ok(Some((state, members)))
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
    /// report, checked in the reply's transaction; a person's needs no
    /// claim. A reply never changes the report's state: only
    /// [`Store::reopen_report`] reopens a closed one.
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

    /// `by` resolves report `tid` of queue `qid` for `reason` as
    /// [`Store::resolve_thread_touched`] does (`resolved_by`
    /// `agent:<harness>` with its session's person, or `viewer:<public ID>`),
    /// and answers the thread, what the resolve changed, and the report:
    /// `resolved` with its reason, kept with the thread and named by its
    /// `thread.resolve`. A fix waiting for a verdict waits no more (its
    /// attempt is `closed`). Resolving a closed report changes nothing.
    /// An agent's resolve needs its session's live lease, checked in the
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
        reason: ResolveReason,
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
            self.resolve_thread_reason_in(
                tx,
                ctx,
                tid,
                &resolved_by,
                person.as_deref(),
                Some(reason.as_str()),
                |_, _| Ok(None),
            )
        })?;
        let thread = self.get_thread(tid)?.ok_or(CoreError::NotFound)?;
        let report = self
            .report(qid, tid, Visibility::All)?
            .ok_or(CoreError::NotFound)?;
        Ok((thread, touched, report))
    }

    /// Adds reporter `reporter_id`'s comment `body` to report `tid` of
    /// queue `qid`, as a viewer comment named by the reporter ID and
    /// carrying it, and answers the report. The comment leaves the
    /// report's state as it is: a closed report is reopened by
    /// [`Store::reopen_report`] alone.
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
        self.reporter_comment_touched(ctx, qid, tid, reporter_id, body)
            .map(|(r, _)| r)
    }

    /// [`Store::reporter_comment`], sending the comment to the session
    /// that holds the report's live lease (the agent working on it) or,
    /// while a fix waits (`fix_proposed`), to the session of its latest
    /// lease (the fix's proposer) while that session lives, when that
    /// session's person is still a member of the queue: a feedback row for
    /// it, made in the comment's transaction, which reaches the session as
    /// any comment sent to it does, under the queue's member rule (whole
    /// while its rights read reports, `no_access` otherwise). Otherwise no
    /// session is sent it, and triage reads it on the report. Answers the
    /// report and the session woken, for the caller to wake its waits and
    /// pushes.
    ///
    /// # Errors
    /// As [`Store::reporter_comment`].
    pub fn reporter_comment_touched(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        reporter_id: &str,
        body: &str,
    ) -> Result<(Report, Touched)> {
        let vis = Visibility::Reporter(reporter_id);
        self.report(qid, tid, vis)?.ok_or(CoreError::NotFound)?;
        let rid = reporter_id.to_string();
        let now = self.now();
        let mut touched = Touched::default();
        let woken = &mut touched;
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
            move |tx, t, c| {
                tx.execute(
                    "UPDATE comments SET reporter_id = ?2 WHERE id = ?1",
                    params![c.id, rid],
                )?;
                let Some(cur) = current_in(tx, tid, &now)? else {
                    return Ok(());
                };
                if !cur.live {
                    // A fix waits: its proposer's lease ended with the
                    // proposal, and the reply goes to that session still.
                    let s = standing_in(tx, tid)?;
                    if state_of(&s.status, s.phase.as_deref(), false) != State::FixProposed {
                        return Ok(());
                    }
                }
                if !session_is_member_in(tx, qid, &cur.session_id)? {
                    return Ok(());
                }
                tx.execute(
                    "INSERT INTO feedback (id, thread_id, comment_id, target_session_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![new_ulid(), tid, c.id, cur.session_id, now],
                )?;
                woken.targets.insert(cur.session_id);
                woken
                    .threads
                    .insert((t.artifact_id.clone(), tid.to_string()));
                Ok(())
            },
        )?;
        let report = self.report(qid, tid, vis)?.ok_or(CoreError::NotFound)?;
        Ok((report, touched))
    }

    /// `by` proposes `fix` on report `tid` of queue `qid` (spec
    /// 2026-10-29-feedback-loop-design §6.2), and answers the report after:
    /// the attempt is numbered one past the report's last, its summary is
    /// posted as `by`'s comment (which the reporter reads), the report
    /// moves to `fix_proposed`, and its live lease ends as
    /// [`Store::release`] ends one (`queue.release`, reason
    /// `fix_proposed`). Recorded as `queue.fix {queue_id, thread_id, n,
    /// url?, commit?}`. An agent proposes only under its own live lease,
    /// checked in the fix's transaction; a person needs none, and a
    /// person's fix on a report that is `fix_proposed` already is its next
    /// attempt, the one waiting becoming `superseded`.
    ///
    /// # Errors
    /// `invalid_fix` for a bad fix ([`NewFix::check`]);
    /// [`CoreError::NotAMember`] for no such queue, `NotFound` for no such
    /// report of it; [`Store::check_lease`]'s for an agent;
    /// [`CoreError::NotClaimable`] for a `verified` or `resolved` report.
    pub fn propose_fix(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: &ReportWriter<'_>,
        fix: NewFix,
    ) -> Result<Report> {
        self.propose_fix_fenced(ctx, qid, tid, by, fix, None)
    }

    /// [`Store::propose_fix`], and for an agent given `fence` (the one its
    /// claim answered), checked in the fix's transaction against its
    /// session's lease: a lease of another fence, its session's earlier
    /// one, is [`CoreError::LeaseLost`]. A person's fix holds no lease,
    /// and its `fence` is not read.
    ///
    /// # Errors
    /// [`Store::propose_fix`]'s, and [`CoreError::LeaseLost`] as above.
    pub fn propose_fix_fenced(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: &ReportWriter<'_>,
        fix: NewFix,
        fence: Option<u64>,
    ) -> Result<Report> {
        fix.check()?;
        let comment = match by {
            ReportWriter::Agent(sess) => {
                // A lapsed lease's owner is cleared whatever this answers.
                self.check_lease(qid, tid, &sess.id)?;
                NewComment {
                    author_kind: AUTHOR_AGENT,
                    author_name: sess.harness.clone(),
                    author_public_id: None,
                    via_session_id: Some(sess.id.clone()),
                    body: fix.summary.clone(),
                    via_page: false,
                }
            }
            ReportWriter::Person { name, public_id } => NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: name.clone(),
                author_public_id: public_id.clone(),
                via_session_id: None,
                body: fix.summary.clone(),
                via_page: false,
            },
        };
        let proposer = comment.author_name.clone();
        let agent = by.session();
        let now = self.now();
        let fix_json = serde_json::to_string(&fix).expect("fixes serialise");
        self.add_comment_then(ctx, tid, comment, |tx, t, _| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let aid = report_artifact_in(tx, qid, tid)?;
            if let Some(sid) = agent {
                check_lease_in(tx, tid, sid, &now)?;
                check_not_taken_in(tx, tid, sid)?;
                if let Some(f) = fence
                    && current_in(tx, tid, &now)?.is_none_or(|cur| cur.fence != f)
                {
                    return Err(CoreError::LeaseLost);
                }
            }
            // `t` is the thread as it was before the summary: a person's
            // comment would have reopened a closed one.
            if t.status == "resolved" {
                return Err(CoreError::NotClaimable);
            }
            // A person's next fix supersedes the one still waiting.
            tx.execute(
                "UPDATE report_fixes SET outcome = 'superseded' WHERE thread_id = ?1
                   AND outcome = 'pending'",
                params![tid],
            )?;
            let n: u32 = tx.query_row(
                "SELECT COALESCE(MAX(n), 0) + 1 FROM report_fixes WHERE thread_id = ?1",
                params![tid],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO report_fixes (thread_id, n, fix_json, proposer_name, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![tid, n, fix_json, proposer, now],
            )?;
            tx.execute(
                "UPDATE threads SET report_phase = 'fix_proposed' WHERE id = ?1",
                params![tid],
            )?;
            end_lease_in(self, tx, ctx, qid, tid, &aid, &now, "fix_proposed")?;
            let mut rec = queue_record(AuditKind::QueueFix, &now, qid)
                .with("thread_id", tid)
                .with("n", n);
            if let Some(u) = &fix.url {
                rec = rec.with("url", u.as_str());
            }
            if let Some(c) = &fix.commit {
                rec = rec.with("commit", c.as_str());
            }
            rec.ids.artifact = Some(aid);
            rec.ids.thread = Some(tid.to_string());
            rec.ids.session = agent.map(str::to_string);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })?;
        self.report(qid, tid, Visibility::All)?
            .ok_or(CoreError::NotFound)
    }

    /// [`Store::verify_report_touched`]'s report.
    ///
    /// # Errors
    /// [`Store::verify_report_touched`]'s.
    pub fn verify_report(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: Verdict<'_>,
        note: Option<String>,
        context: Option<ReportContext>,
    ) -> Result<Report> {
        self.verify_report_touched(ctx, qid, tid, by, note, context)
            .map(|(r, _)| r)
    }

    /// `by` says the fix waiting on report `tid` of queue `qid` works (spec
    /// 2026-10-29-feedback-loop-design §6.3): the latest attempt's outcome
    /// is `verified`, with `note` (blank is none), `by`'s `context` at
    /// verification and the time; the report is `verified` and its thread
    /// resolved as [`Store::resolve_thread_touched`] resolves one, by
    /// `reporter:<reporter ID>` or `viewer:<public ID>`, for reason
    /// `fixed`. Recorded as `queue.verify {queue_id, thread_id, n}`.
    /// Answers the report as `by` reads it, and what the resolve changed.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// report of it, or one a reporter did not file or does not see;
    /// [`CoreError::NotFixProposed`] when no fix waits for a verdict;
    /// `invalid_comment` for a note too long; [`CoreError::InvalidContext`].
    pub fn verify_report_touched(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: Verdict<'_>,
        note: Option<String>,
        context: Option<ReportContext>,
    ) -> Result<(Report, clax_core::feedback::Touched)> {
        let note = note.filter(|n| !n.trim().is_empty());
        if let Some(n) = &note {
            check_body(n)?;
        }
        let context = context.as_ref().map(context_json).transpose()?;
        let now = self.now();
        let resolved_by = by.resolved_by();
        let touched = self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let s = check_verdict_in(tx, &q, tid, &by)?;
            if state_of(&s.status, s.phase.as_deref(), false) != State::FixProposed {
                return Err(CoreError::NotFixProposed);
            }
            let n: u32 = tx
                .query_row(
                    "SELECT MAX(n) FROM report_fixes WHERE thread_id = ?1",
                    params![tid],
                    |r| r.get::<_, Option<u32>>(0),
                )?
                .ok_or(CoreError::NotFixProposed)?;
            tx.execute(
                "UPDATE report_fixes SET outcome = 'verified', verdict_note = ?3,
                    verdict_context_json = ?4, verdict_at = ?5 WHERE thread_id = ?1 AND n = ?2",
                params![tid, n, note, context, now],
            )?;
            let touched = self.resolve_thread_reason_in(
                tx,
                ctx,
                tid,
                &resolved_by,
                None,
                Some(ResolveReason::Fixed.as_str()),
                |_, _| Ok(None),
            )?;
            tx.execute(
                "UPDATE threads SET report_phase = 'verified' WHERE id = ?1",
                params![tid],
            )?;
            let mut rec = queue_record(AuditKind::QueueVerify, &now, qid)
                .with("thread_id", tid)
                .with("n", n);
            rec.ids.artifact = Some(s.artifact_id);
            rec.ids.thread = Some(tid.to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(touched)
        })?;
        let report = self
            .report(qid, tid, by.visibility())?
            .ok_or(CoreError::NotFound)?;
        Ok((report, touched))
    }

    /// [`Store::reopen_report_async`], blocking on its content store.
    ///
    /// # Errors
    /// [`Store::reopen_report_async`]'s.
    #[allow(clippy::too_many_arguments)]
    pub fn reopen_report(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: Verdict<'_>,
        body: String,
        clip: Option<Vec<u8>>,
        diagnostics: Option<Vec<DiagLine>>,
        context: Option<ReportContext>,
    ) -> Result<(Report, Touched)> {
        self.block_on(self.reopen_report_async(ctx, qid, tid, by, body, clip, diagnostics, context))
    }

    /// `by` says report `tid` of queue `qid` is still broken (spec
    /// 2026-10-29-feedback-loop-design §6.3), from `fix_proposed`,
    /// `verified` or `resolved`. The reopen is a new capture, numbered as
    /// its `reopen_count` (the filing is capture 0): `body` is posted as
    /// `by`'s comment (a reporter's carries its reporter ID) carrying the
    /// capture's number and its `clip`, kept apart from every other
    /// capture's ([`Store::report_comment_clip_async`]), and its
    /// `diagnostics` are kept as that capture's
    /// ([`Store::report_diagnostics`]); earlier captures stay as they were.
    /// A latest attempt still `pending` is `rejected`, with `body` as its
    /// note, `by`'s `context` and the time. The report is open again (its
    /// thread unresolved, `thread.reopen` when it was resolved), its phase
    /// and resolve reason cleared. Recorded as `queue.reopen {queue_id,
    /// thread_id, n?, reopen_count}` (`n` the latest attempt's, when there
    /// is one), after the comment's `comment.add`. In the same transaction
    /// the reopen's comment wakes every live session watching the queue
    /// and the session of the report's latest lease when it has not ended
    /// (whoever claimed it last, the fix's proposer), watching or not
    /// ([`Store::notify_queue_event_in`], `reopened`). Answers the report
    /// as `by` reads it, and the sessions woken, to wake their waits with.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// report of it, or one a reporter did not file or does not see;
    /// [`CoreError::NotFixProposed`] for an `open` or `claimed` report;
    /// `invalid_comment`, `invalid_clip`, [`CoreError::InvalidDiagnostics`]
    /// and [`CoreError::InvalidContext`] for bad input.
    #[allow(clippy::too_many_arguments)]
    pub async fn reopen_report_async(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        tid: &str,
        by: Verdict<'_>,
        body: String,
        clip: Option<Vec<u8>>,
        diagnostics: Option<Vec<DiagLine>>,
        context: Option<ReportContext>,
    ) -> Result<(Report, Touched)> {
        check_body(&body)?;
        if let Some(problem) = clip.as_deref().and_then(clip_problem) {
            return Err(CoreError::invalid("invalid_clip", problem));
        }
        let diagnostics = match diagnostics.as_deref() {
            Some(lines) => diagnostics_json(lines, &self.now())?,
            None => None,
        };
        let context = context.as_ref().map(context_json).transpose()?;
        let reopenable = |s: &Standing| match state_of(&s.status, s.phase.as_deref(), false) {
            State::FixProposed | State::Verified | State::Resolved => Ok(()),
            State::Open | State::Claimed => Err(CoreError::NotFixProposed),
        };
        // Checked before the clip is written, and again in the reopen's
        // transaction.
        let aid = self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            let s = check_verdict_in(c, &q, tid, &by)?;
            reopenable(&s)?;
            Ok(s.artifact_id)
        })?;
        let aid = ArtifactId::parse(&aid)?;
        // The clip goes first, under a key of its own and the lock a delete
        // holds across its transaction and the content's removal; it is
        // removed when the reopen is refused.
        let (_lock, clip_key) = match &clip {
            Some(bytes) => {
                let lock = self.lock_content(&aid).await?;
                if !self.with_read(|c| super::threads::artifact_live(c, aid.as_str()))? {
                    return Err(CoreError::NotFound);
                }
                let key = comment_clip_key(tid);
                self.content.put_clip(&aid, &key, bytes).await?;
                (Some(lock), Some(key))
            }
            None => (None, None),
        };
        let comment = match &by {
            Verdict::Reporter(r) => NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: r.to_string(),
                author_public_id: None,
                via_session_id: None,
                body: body.clone(),
                via_page: false,
            },
            Verdict::Person { name, public_id } => NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: name.clone(),
                author_public_id: public_id.clone(),
                via_session_id: None,
                body: body.clone(),
                via_page: false,
            },
        };
        let now = self.now();
        let done = self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let s = check_verdict_in(tx, &q, tid, &by)?;
            reopenable(&s)?;
            let capture: u32 = tx.query_row(
                "SELECT reopen_count + 1 FROM threads WHERE id = ?1",
                params![tid],
                |r| r.get(0),
            )?;
            let (c, _) = self.add_comment_in(tx, ctx, tid, comment)?;
            let reporter = match &by {
                Verdict::Reporter(r) => Some(*r),
                Verdict::Person { .. } => None,
            };
            tx.execute(
                "UPDATE comments SET reporter_id = ?2, report_capture = ?3, clip_key = ?4
                 WHERE id = ?1",
                params![c.id, reporter, capture, clip_key],
            )?;
            let last: Option<(u32, String)> = tx
                .query_row(
                    "SELECT n, outcome FROM report_fixes WHERE thread_id = ?1 ORDER BY n DESC LIMIT 1",
                    params![tid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((n, outcome)) = &last
                && outcome == FixOutcome::Pending.as_str()
            {
                tx.execute(
                    "UPDATE report_fixes SET outcome = 'rejected', verdict_note = ?3,
                        verdict_context_json = ?4, verdict_at = ?5 WHERE thread_id = ?1 AND n = ?2",
                    params![tid, n, body, context, now],
                )?;
            }
            tx.execute(
                "UPDATE threads SET status = 'open', resolved_at = NULL, resolved_by = NULL,
                    resolved_by_person = NULL, report_phase = NULL, resolve_reason = NULL,
                    reopen_count = ?2 WHERE id = ?1",
                params![tid, capture],
            )?;
            if s.status == "resolved" {
                let rec = super::threads::thread_record(
                    AuditKind::ThreadReopen,
                    &now,
                    &s.artifact_id,
                    tid,
                );
                self.record_audit(tx, ctx, rec)?;
            }
            if let Some((lines, n)) = &diagnostics {
                tx.execute(
                    "INSERT INTO report_diagnostics (thread_id, capture, lines_json, n)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![tid, capture, lines, n],
                )?;
            }
            let previous = match current_in(tx, tid, &now)? {
                Some(cur) => {
                    let live: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND ended_at IS NULL)",
                        params![cur.session_id],
                        |r| r.get(0),
                    )?;
                    live.then_some(cur.session_id)
                }
                None => None,
            };
            let mut rec = queue_record(AuditKind::QueueReopen, &now, qid).with("thread_id", tid);
            if let Some((n, _)) = last {
                rec = rec.with("n", n);
            }
            rec = rec.with("reopen_count", capture);
            rec.ids.artifact = Some(s.artifact_id);
            rec.ids.thread = Some(tid.to_string());
            self.record_audit(tx, ctx, rec)?;
            self.notify_queue_event_in(
                tx,
                qid,
                tid,
                &c.id,
                QueueEvent::Reopened,
                previous.as_deref(),
            )
        });
        let woken = match done {
            Ok(w) => w,
            Err(e) => {
                if let Some(key) = &clip_key
                    && let Err(gone) = self.content.delete_clip(&aid, key).await
                {
                    tracing::warn!(
                        artifact = aid.as_str(),
                        thread = tid,
                        error = %gone,
                        "could not remove the clip of a report not reopened"
                    );
                }
                return Err(e);
            }
        };
        let report = self
            .report(qid, tid, by.visibility())?
            .ok_or(CoreError::NotFound)?;
        Ok((report, woken))
    }

    /// The PNG clip of comment `cid` of report `tid` in queue `qid` (a
    /// reopen's capture), as `vis` may see it: `None` when the comment has
    /// none. The report's own clip, filed with it, is
    /// [`Store::report_clip_async`]'s.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// report of it, one `vis` does not see, or no such comment of it.
    pub async fn report_comment_clip_async(
        &self,
        qid: &str,
        tid: &str,
        cid: &str,
        vis: Visibility<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let (aid, key) = self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            if !sees_in(c, &q, tid, vis)? {
                return Err(CoreError::NotFound);
            }
            let key: Option<String> = c
                .query_row(
                    "SELECT clip_key FROM comments WHERE id = ?1 AND thread_id = ?2",
                    params![cid, tid],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            Ok((standing_in(c, tid)?.artifact_id, key))
        })?;
        let Some(key) = key else {
            return Ok(None);
        };
        self.content
            .read_clip(&ArtifactId::parse(&aid)?, &key)
            .await
    }

    /// The diagnostics of report `tid` of queue `qid` as `vis` may see
    /// them: those of capture `capture` (0 the filing's, `n` the `n`th
    /// reopen's), or with `None` the latest capture's that has any; `None`
    /// when that capture has none. A reporter reads those of the reports it
    /// filed alone, even where `reporters_see: all` shows it others.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// report of it, one `vis` does not see, or one a reporter did not
    /// file.
    pub fn report_diagnostics(
        &self,
        qid: &str,
        tid: &str,
        vis: Visibility<'_>,
        capture: Option<u32>,
    ) -> Result<Option<Vec<DiagLine>>> {
        self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            if !sees_in(c, &q, tid, vis)? {
                return Err(CoreError::NotFound);
            }
            let s = standing_in(c, tid)?;
            // A reporter's console and requests are its own, whoever else
            // `reporters_see: all` shows the report to.
            if let Visibility::Reporter(r) = vis
                && s.reporter_id.as_deref() != Some(r)
            {
                return Err(CoreError::NotFound);
            }
            let aid = s.artifact_id;
            let json: Option<String> = c
                .query_row(
                    "SELECT lines_json FROM report_diagnostics
                     WHERE thread_id = ?1 AND (?2 IS NULL OR capture = ?2)
                     ORDER BY capture DESC LIMIT 1",
                    params![tid, capture],
                    |r| r.get(0),
                )
                .optional()?;
            from_json(json.as_deref(), &aid, "report_diagnostics.lines_json")
        })
    }

    /// What waits for reporter `reporter_id` in queue `qid` (spec
    /// 2026-10-29-feedback-loop-design §6.1): the reports it filed whose
    /// fix waits for its verdict (`fix_proposed`), or that hold a comment
    /// not its own made no earlier than it last read the report
    /// ([`Store::mark_reporter_seen`]; any, before it ever did), oldest
    /// first.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn reporter_badge(&self, qid: &str, reporter_id: &str) -> Result<Badge> {
        self.with_read(|c| {
            queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            let report_ids: Vec<String> = c
                .prepare_cached(
                    "SELECT t.id FROM threads t JOIN artifacts a ON a.id = t.artifact_id
                     WHERE a.deleted_at IS NULL AND t.queue_id = ?1 AND t.reporter_id = ?2
                       AND ((t.status = 'open' AND t.report_phase = 'fix_proposed')
                         OR EXISTS (SELECT 1 FROM comments c WHERE c.thread_id = t.id
                              AND c.reporter_id IS NOT ?2 AND NOT c.triage_only
                              AND (t.reporter_seen_at IS NULL OR c.created_at >= t.reporter_seen_at)))
                     ORDER BY t.created_at, t.id",
                )?
                .query_map(params![qid, reporter_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Badge {
                waiting: report_ids.len() as u32,
                report_ids,
            })
        })
    }

    /// Reporter `reporter_id` has read report `tid` of queue `qid` now: a
    /// comment before now no longer counts in its badge
    /// ([`Store::reporter_badge`]). A report it sees but did not file is
    /// left as it is. Records nothing.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// report of it, or one the reporter does not see.
    pub fn mark_reporter_seen(&self, qid: &str, tid: &str, reporter_id: &str) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            if !sees_in(tx, &q, tid, Visibility::Reporter(reporter_id))? {
                return Err(CoreError::NotFound);
            }
            tx.execute(
                "UPDATE threads SET reporter_seen_at = ?3 WHERE id = ?1 AND reporter_id = ?2",
                params![tid, reporter_id, now],
            )?;
            Ok(())
        })
    }

    /// Session `sid` watches queue `qid` (spec 2026-10-29-feedback-loop-design
    /// §7.3) until it unwatches it or ends; watching it again changes
    /// nothing. Records nothing.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `unknown_session` for a
    /// session that is not live.
    pub fn watch_queue(&self, qid: &str, sid: &str) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            live_session_in(tx, sid, "a queue is watched by a live session")?;
            tx.execute(
                "INSERT INTO queue_watches (queue_id, session_id, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(queue_id, session_id) DO NOTHING",
                params![qid, sid, now],
            )?;
            Ok(())
        })
    }

    /// Session `sid` no longer watches queue `qid`; one that did not
    /// watch it is fine.
    ///
    /// # Errors
    /// Database errors.
    pub fn unwatch_queue(&self, qid: &str, sid: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM queue_watches WHERE queue_id = ?1 AND session_id = ?2",
                params![qid, sid],
            )?;
            Ok(())
        })
    }

    /// The live sessions watching queue `qid`, in the order they began
    /// watching it.
    ///
    /// # Errors
    /// Database errors.
    pub fn queue_watchers(&self, qid: &str) -> Result<Vec<String>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT w.session_id FROM queue_watches w JOIN sessions s ON s.id = w.session_id
                 WHERE w.queue_id = ?1 AND s.ended_at IS NULL ORDER BY w.created_at, w.session_id",
            )?
            .query_map(params![qid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
        })
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
///
/// # Errors
/// `unknown_session` with `message` otherwise.
fn live_session_in(c: &Connection, sid: &str, message: &'static str) -> Result<()> {
    let live: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND ended_at IS NULL)",
        params![sid],
        |r| r.get(0),
    )?;
    if live {
        Ok(())
    } else {
        Err(CoreError::invalid("unknown_session", message))
    }
}

#[cfg(test)]
mod tests;
