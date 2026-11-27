//! Queue requests: an agent's ask for a report queue, which its person
//! approves or denies (`docs/contract.md`, "Queue requests"). An agent may
//! never make a queue itself (`POST /api/queues` refuses every agent): it
//! asks ([`Store::request_queue`]), its person answers in their browser
//! ([`Store::approve_queue_request`], [`Store::deny_queue_request`]), and
//! the agent reads the outcome ([`Store::queue_request`]) and takes it
//! ([`Store::take_queue_request`]).
//!
//! - **Who answers.** A request is its agent's person's to answer: the
//!   owner's for the owner's agents (and a grant of the owner's row), a
//!   connected person's for theirs. One inbox item tells them
//!   ([`Kind::QueueRequest`](crate::inbox::Kind)).
//! - **Approval.** The queue is made with the approver as its admin, as
//!   their own `POST /api/queues` would make it. Nothing else is granted.
//! - **The one take.** The agent's first take of an approved request makes
//!   one service grant of the queue and hands it its token and Ed25519
//!   private key, in that answer alone: neither is kept, and a later take
//!   (or read) says it was taken. Reading a request changes nothing.
//! - **Queue-request tokens.** A device sign-in that asked for
//!   [`QUEUE_REQUEST_SCOPE`] alone makes a token whose one right is a grant
//!   on it from the approving person, `clax-queue-requests` `request`, with
//!   an end of its own ([`crate::grant_raises::issue_request_grant_in`]): it
//!   makes one request, and ends once its agent has taken that request's
//!   answer, at that grant's end ([`SCOPED_GRANT_LIFE_S`] after it was
//!   approved) or when revoked, whichever comes first.
//! - **Life.** A pending request expires [`REQUEST_LIFE_S`] after it was
//!   made, or with its scoped grant if that ends sooner; the end of the
//!   asking grant, however it ends, cancels its pending requests.
//! - **Audit.** `queue_request.ask` as the asking agent,
//!   `queue_request.answer` as the approver (with `queue.create` for the
//!   queue), `queue_request.close` when a request stops waiting without an
//!   answer (expired, cancelled), as `system:daemon` unless the call names
//!   its actor; the service grant is recorded as `queue.update` (`op`
//!   `grant`) under the agent's take.
//!
//! Every time is the store's clock's.

use super::Store;
use crate::inbox;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::queue::{Queue, ReportersSee, Role, check_queue_name};
use clax_core::secret::Secret;
use clax_core::sig::KeyPair;
use clax_core::time::stamp;
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

/// The one scope a grant may have: it may ask its person for a report
/// queue, and do nothing else.
pub const QUEUE_REQUEST_SCOPE: &str = "queue.request";
/// How long a pending request waits for its person, in seconds.
pub const REQUEST_LIFE_S: i64 = 3600;
/// How long a grant of [`QUEUE_REQUEST_SCOPE`] lasts at most, in seconds.
pub const SCOPED_GRANT_LIFE_S: i64 = 3600;
/// The most requests one agent (one grant, or the owner's agents) has
/// pending at once.
pub const MAX_PENDING: i64 = 5;
/// The longest note kept, in characters.
pub const MAX_NOTE_CHARS: usize = 500;

/// Where a request is: `Pending` moves once to any other status, which is
/// final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Approved,
    Denied,
    /// The asking grant ended before an answer.
    Cancelled,
    Expired,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Approved => "approved",
            Status::Denied => "denied",
            Status::Cancelled => "cancelled",
            Status::Expired => "expired",
        }
    }

    /// The status named `s`.
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "pending" => Status::Pending,
            "approved" => Status::Approved,
            "denied" => Status::Denied,
            "cancelled" => Status::Cancelled,
            "expired" => Status::Expired,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A queue request, as its person and its agent see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueueRequest {
    pub id: String,
    pub status: Status,
    /// The queue's name, as asked.
    pub name: String,
    /// The origins it would take reports from, normalized.
    pub origins: Vec<String>,
    pub reporters_see: ReportersSee,
    /// Why, in the agent's words (untrusted).
    pub note: Option<String>,
    /// Who asked: the session's or the grant's client name (untrusted).
    pub agent: String,
    /// Who answers: a connected person's public ID; `None` the owner.
    pub person: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub decided_at: Option<String>,
    /// The queue made, once approved.
    pub queue_id: Option<String>,
    /// The asking grant; `None` for the owner's agents.
    #[serde(skip)]
    pub grant_id: Option<String>,
    /// When the agent took the approved request's grant.
    #[serde(skip)]
    pub taken_at: Option<String>,
}

/// Who asks ([`Store::request_queue`]) or takes the answer ([`Store::take_queue_request`]).
#[derive(Clone, Debug, Default)]
pub struct Requester {
    /// The agent's person: a connected person's public ID; `None` the owner.
    pub person: Option<String>,
    /// The agent's grant; `None` for the owner's agents (the daemon token).
    pub grant_id: Option<String>,
    /// The agent's session, when it named one.
    pub session_id: Option<String>,
}

/// What an agent asks for.
#[derive(Clone, Debug)]
pub struct Ask {
    pub name: String,
    pub origins: Vec<String>,
    pub reporters_see: ReportersSee,
    pub note: Option<String>,
}

/// What the agent's take found ([`Store::take_queue_request`]).
#[derive(Debug)]
pub struct Taken {
    pub request: QueueRequest,
    /// The service grant made by this take, with its token and key: only
    /// on the first take of an approved request.
    pub grant: Option<(crate::queues::ServiceGrant, Secret, KeyPair)>,
}

const COLS: &str = "id, status, name, origins_json, reporters_see, note, agent, person, created_at,
    expires_at, decided_at, queue_id, grant_id, fetched_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<QueueRequest> {
    let bad = |i: usize, what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            format!("queue_requests.{what} is not a value this store writes").into(),
        )
    };
    let status: String = r.get(1)?;
    let origins: String = r.get(3)?;
    let see: String = r.get(4)?;
    Ok(QueueRequest {
        id: r.get(0)?,
        status: Status::parse(&status).ok_or_else(|| bad(1, "status"))?,
        name: r.get(2)?,
        origins: serde_json::from_str(&origins).map_err(|_| bad(3, "origins_json"))?,
        reporters_see: ReportersSee::parse(&see).ok_or_else(|| bad(4, "reporters_see"))?,
        note: r.get(5)?,
        agent: r.get(6)?,
        person: r.get(7)?,
        created_at: r.get(8)?,
        expires_at: r.get(9)?,
        decided_at: r.get(10)?,
        queue_id: r.get(11)?,
        grant_id: r.get(12)?,
        taken_at: r.get(13)?,
    })
}

fn read(c: &Connection, id: &str) -> Result<Option<QueueRequest>> {
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM queue_requests WHERE id = ?1"),
        params![id],
        row,
    )
    .optional()?)
}

fn record(kind: AuditKind, now: &str, id: &str) -> AuditRecord {
    AuditRecord::new(kind, now).with("request_id", id)
}

/// `invalid_args` for a note over [`MAX_NOTE_CHARS`] or with control
/// characters other than newlines; a blank one is none.
fn checked_note(note: Option<&str>) -> Result<Option<String>> {
    let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if n.chars().count() > MAX_NOTE_CHARS
        || n.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(CoreError::invalid(
            "invalid_args",
            format!("a note is at most {MAX_NOTE_CHARS} characters, without control characters"),
        ));
    }
    Ok(Some(n.to_string()))
}

/// Whether grant `gid` is approved, and the end of its grant to ask for a
/// queue when it is a queue-request token
/// ([`crate::grant_raises::request_grant_in`]): `None` for a grant that is
/// not approved, `Some(None)` for one its person gives its rights.
fn scope_of(c: &Connection, gid: &str) -> Result<Option<Option<String>>> {
    let approved: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM grants WHERE id = ?1 AND status = 'approved')",
        params![gid],
        |r| r.get(0),
    )?;
    if !approved {
        return Ok(None);
    }
    Ok(Some(crate::grant_raises::request_grant_in(c, gid)?))
}

/// What the asking agent calls itself: its session's client name or
/// harness, else its grant's client name, else "An agent".
fn agent_name(c: &Connection, who: &Requester) -> Result<String> {
    if let Some(sid) = &who.session_id {
        let n: Option<String> = c
            .query_row(
                "SELECT COALESCE(NULLIF(client_name, ''), harness) FROM sessions WHERE id = ?1",
                params![sid],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(n) = n {
            return Ok(n);
        }
    }
    if let Some(gid) = &who.grant_id {
        let n: Option<String> = c
            .query_row(
                "SELECT client_name FROM grants WHERE id = ?1",
                params![gid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = n.filter(|n| !n.is_empty()) {
            return Ok(n);
        }
    }
    Ok("An agent".to_string())
}

/// Closes pending request `id` of approver `person` as `status`, reading
/// its inbox item and recording `queue_request.close` under `ctx`;
/// nothing for one no longer pending.
fn close(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    id: &str,
    person: Option<&str>,
    status: Status,
    now: &str,
) -> Result<()> {
    let n = tx.execute(
        "UPDATE queue_requests SET status = ?2, decided_at = ?3 WHERE id = ?1 AND status = 'pending'",
        params![id, status.as_str(), now],
    )?;
    if n == 0 {
        return Ok(());
    }
    inbox::queue_request_closed(tx, id, person, now)?;
    st.record_audit(
        tx,
        ctx,
        record(AuditKind::QueueRequestClose, now, id).with("status", status.as_str()),
    )
    .map(drop)
}

/// Expires every pending request past its time, as `system:daemon`.
fn settle(st: &Store, tx: &Transaction<'_>, now: &str) -> Result<()> {
    let due: Vec<(String, Option<String>)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id, person FROM queue_requests WHERE status = 'pending' AND expires_at <= ?1",
        )?;
        stmt.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, person) in due {
        close(
            st,
            tx,
            &AuditCtx::DAEMON,
            &id,
            person.as_deref(),
            Status::Expired,
            now,
        )?;
    }
    Ok(())
}

/// Inside the end of grant `gid`, cancels its pending requests, recorded
/// under `ctx` as the grant's end is.
pub(crate) fn cancel_for_grant(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    gid: &str,
    now: &str,
) -> Result<()> {
    let pending: Vec<(String, Option<String>)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT id, person FROM queue_requests WHERE grant_id = ?1 AND status = 'pending'",
        )?;
        stmt.query_map(params![gid], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, person) in pending {
        close(st, tx, ctx, &id, person.as_deref(), Status::Cancelled, now)?;
    }
    Ok(())
}

/// `request_closed` for a request no longer pending.
fn request_closed(r: &QueueRequest) -> CoreError {
    CoreError::invalid(
        "request_closed",
        format!("this request is {} already", r.status),
    )
}

impl Store {
    /// Files `a` as `who`'s request, for `who`'s person to answer,
    /// recording `queue_request.ask` under `ctx` (the asking agent), and
    /// tells the person in their inbox.
    ///
    /// # Errors
    /// `invalid_name`, `invalid_origin` or `invalid_args` for bad fields;
    /// `grant_revoked` for a grant that is not approved;
    /// `request_exists` for a scoped grant that asked once already;
    /// `limit_reached` with [`MAX_PENDING`] pending.
    pub fn request_queue(&self, ctx: &AuditCtx, who: &Requester, a: Ask) -> Result<QueueRequest> {
        let name = check_queue_name(&a.name)?;
        let origins = crate::queues::normalize_origins(&a.origins)?;
        let note = checked_note(a.note.as_deref())?;
        let now_utc = self.now_utc();
        self.with_tx(|tx| {
            let now = stamp(now_utc);
            settle(self, tx, &now)?;
            let mut expires_at = stamp(now_utc + chrono::Duration::seconds(REQUEST_LIFE_S));
            if let Some(gid) = &who.grant_id {
                let ends = scope_of(tx, gid)?.ok_or_else(|| {
                    CoreError::invalid("grant_revoked", "this agent's grant has ended")
                })?;
                if ends.is_some() {
                    let asked: bool = tx.query_row(
                        "SELECT EXISTS (SELECT 1 FROM queue_requests WHERE grant_id = ?1)",
                        params![gid],
                        |r| r.get(0),
                    )?;
                    if asked {
                        return Err(CoreError::invalid(
                            "request_exists",
                            "this token may ask for one queue, and has asked: take that request's answer",
                        ));
                    }
                }
                if let Some(e) = ends.filter(|e| *e < expires_at) {
                    expires_at = e;
                }
            }
            let pending: i64 = tx.query_row(
                "SELECT COUNT(*) FROM queue_requests WHERE status = 'pending'
                   AND grant_id IS ?1 AND (?1 IS NOT NULL OR person IS NULL)",
                params![who.grant_id],
                |r| r.get(0),
            )?;
            if pending >= MAX_PENDING {
                return Err(CoreError::invalid(
                    "limit_reached",
                    format!("{MAX_PENDING} requests wait for an answer already"),
                ));
            }
            let id = new_ulid();
            let agent = agent_name(tx, who)?;
            tx.execute(
                "INSERT INTO queue_requests (id, person, grant_id, session_id, agent, name,
                    origins_json, reporters_see, note, status, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, ?11)",
                params![
                    id,
                    who.person,
                    who.grant_id,
                    who.session_id,
                    agent,
                    name,
                    serde_json::to_string(&origins).expect("strings serialise"),
                    a.reporters_see.as_str(),
                    note,
                    now,
                    expires_at
                ],
            )?;
            let text = [Some(agent.as_str()), Some(name.as_str()), note.as_deref()]
                .into_iter()
                .flatten()
                .chain(origins.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            inbox::note_queue_request(
                tx,
                &id,
                who.person.as_deref(),
                who.session_id.as_deref(),
                &text,
                &now,
            )?;
            let mut rec = record(AuditKind::QueueRequestAsk, &now, &id)
                .with("name", name.as_str())
                .with("origins", origins.clone())
                .with("reporters_see", a.reporters_see.as_str())
                .with("approver", who.person.clone());
            if let Some(n) = &note {
                rec = rec.with("note", n.as_str());
            }
            if let Some(g) = &who.grant_id {
                rec = rec.with("grant_id", g.as_str());
            }
            self.record_audit(tx, ctx, rec)?;
            read(tx, &id)?.ok_or(CoreError::NotFound)
        })
    }

    /// Request `id`, its expiry settled first; `None` when there is none.
    ///
    /// # Errors
    /// The database's.
    pub fn queue_request(&self, id: &str) -> Result<Option<QueueRequest>> {
        self.with_tx(|tx| {
            settle(self, tx, &self.now())?;
            read(tx, id)
        })
    }

    /// The requests `person` (`None`: the owner) answers, newest first,
    /// only those of `status` when given; at most 200.
    ///
    /// # Errors
    /// The database's.
    pub fn queue_requests_for(
        &self,
        person: Option<&str>,
        status: Option<Status>,
    ) -> Result<Vec<QueueRequest>> {
        self.with_tx(|tx| {
            settle(self, tx, &self.now())?;
            let mut stmt = tx.prepare_cached(&format!(
                "SELECT {COLS} FROM queue_requests
                 WHERE COALESCE(person, '') = COALESCE(?1, '') AND (?2 IS NULL OR status = ?2)
                 ORDER BY created_at DESC, id DESC LIMIT 200"
            ))?;
            Ok(stmt
                .query_map(params![person, status.map(Status::as_str)], row)?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Approves pending request `id` as its person `approver` (`None`: the
    /// owner): makes its queue, with them as admin, recording
    /// `queue_request.answer` and `queue.create` under `ctx`.
    ///
    /// # Errors
    /// `NotFound` for no such request of `approver`'s; `request_closed`
    /// for one no longer pending.
    pub fn approve_queue_request(
        &self,
        ctx: &AuditCtx,
        id: &str,
        approver: Option<&str>,
    ) -> Result<(QueueRequest, Queue)> {
        self.with_tx(|tx| {
            let now = self.now();
            settle(self, tx, &now)?;
            let r = read(tx, id)?
                .filter(|r| r.person.as_deref() == approver)
                .ok_or(CoreError::NotFound)?;
            if r.status != Status::Pending {
                return Err(request_closed(&r));
            }
            let queue =
                self.create_queue_in(tx, ctx, approver, &r.name, &r.origins, r.reporters_see)?;
            tx.execute(
                "UPDATE queue_requests SET status = 'approved', decided_at = ?2, queue_id = ?3
                 WHERE id = ?1",
                params![id, now, queue.id],
            )?;
            inbox::queue_request_closed(tx, id, approver, &now)?;
            self.record_audit(
                tx,
                ctx,
                record(AuditKind::QueueRequestAnswer, &now, id)
                    .with("answer", "approve")
                    .with("queue_id", queue.id.as_str()),
            )?;
            let r = read(tx, id)?.ok_or(CoreError::NotFound)?;
            Ok((r, queue.view(Role::Admin)))
        })
    }

    /// Denies pending request `id` as its person `approver`, recording
    /// `queue_request.answer` under `ctx`.
    ///
    /// # Errors
    /// As [`Store::approve_queue_request`].
    pub fn deny_queue_request(
        &self,
        ctx: &AuditCtx,
        id: &str,
        approver: Option<&str>,
    ) -> Result<QueueRequest> {
        self.with_tx(|tx| {
            let now = self.now();
            settle(self, tx, &now)?;
            let r = read(tx, id)?
                .filter(|r| r.person.as_deref() == approver)
                .ok_or(CoreError::NotFound)?;
            if r.status != Status::Pending {
                return Err(request_closed(&r));
            }
            tx.execute(
                "UPDATE queue_requests SET status = 'denied', decided_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            inbox::queue_request_closed(tx, id, approver, &now)?;
            self.record_audit(
                tx,
                ctx,
                record(AuditKind::QueueRequestAnswer, &now, id).with("answer", "deny"),
            )?;
            read(tx, id)?.ok_or(CoreError::NotFound)
        })
    }

    /// The asking agent `who` takes the answer to request `id`. The first
    /// take of an approved request whose queue is still there makes its
    /// service grant (named for the agent), recorded under `ctx`, and
    /// hands it over in [`Taken::grant`]. A take of a decided request by a
    /// scoped grant ends that grant; a pending request is returned as it
    /// is, changing nothing. `who` must be the request's own grant, or, for a
    /// request of the owner's agents, an agent of the owner's.
    ///
    /// # Errors
    /// `NotFound` for no such request of `who`'s.
    pub fn take_queue_request(&self, ctx: &AuditCtx, id: &str, who: &Requester) -> Result<Taken> {
        self.with_tx(|tx| {
            let now = self.now();
            settle(self, tx, &now)?;
            let r = read(tx, id)?
                .filter(|r| r.grant_id == who.grant_id && r.person == who.person)
                .ok_or(CoreError::NotFound)?;
            let mut grant = None;
            if r.status == Status::Approved
                && r.taken_at.is_none()
                && let Some(qid) = &r.queue_id
                && crate::queues::queue_in(tx, qid)?.is_some()
            {
                let name: String = format!("Requested by {}", r.agent).chars().take(120).collect();
                let made = self.create_service_grant_in(tx, ctx, qid, name.trim(), None)?;
                tx.execute(
                    "UPDATE queue_requests SET fetched_at = ?2, service_grant_id = ?3 WHERE id = ?1",
                    params![id, now, made.0.id],
                )?;
                grant = Some(made);
            }
            if r.status != Status::Pending
                && let Some(gid) = &r.grant_id
                && matches!(scope_of(tx, gid)?, Some(Some(_)))
            {
                super::people::end_grant_in(self, tx, ctx, gid)?;
            }
            let request = read(tx, id)?.ok_or(CoreError::NotFound)?;
            Ok(Taken { request, grant })
        })
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests;
