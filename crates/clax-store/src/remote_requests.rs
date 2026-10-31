//! Connection requests: an agent's ask that the owner connect this home to
//! a Clax instance it is not connected to (spec
//! 2026-10-29-remote-instances §3). Initiation is split from approval: an
//! agent asks ([`Store::ask_remote`]), the owner answers in their browser
//! ([`Store::answer_remote_request`]), and a Connect runs the owner's own
//! device sign-in, approved on the remote.
//!
//! - **One open request per origin.** A later ask, by any session, joins
//!   it: each asking session is kept with the URL it wanted and its
//!   reason. Only an ask that opens a request puts it in front of the
//!   owner (one inbox item, [`Kind::RemoteRequest`](crate::inbox::Kind)).
//! - **Gates.** An origin connected already answers its remote; a muted
//!   origin ("Never for this instance", until the owner unmutes it)
//!   answers muted. A new request (not a join) is refused for
//!   [`NOT_NOW_S`] after a "Not now" or a sign-in denied on the remote,
//!   and once its session opened [`MAX_NEW_PER_SESSION`] in
//!   [`WINDOW_S`].
//! - **Life.** An open request expires [`REQUEST_LIFE_S`] after it opened.
//!   A request also settles as `connected` once its origin is connected,
//!   however that came about (its own Connect, or `clax remote add`).
//!
//! - **Audit.** Each change records its `remote_request.*` event in its own
//!   transaction: `remote_request.ask` for an ask that opened or joined a
//!   request (the asking session's agent), `remote_request.answer` for the
//!   owner's Connect, "Not now" or "Never", and `remote_request.close`
//!   when a request settles without them (connected, expired, or its
//!   Connect's sign-in ended), as `system:daemon` unless the call names
//!   its actor.
//!
//! Every time is the store's clock's.

use super::Store;
use crate::inbox;
use clax_core::audit::{AuditCtx, AuditKind, AuditRecord};
use clax_core::time::stamp;
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::json;

/// How long an open request waits for the owner, in seconds.
pub const REQUEST_LIFE_S: i64 = 24 * 3600;
/// How long a "Not now" (or a sign-in denied on the remote) holds new
/// requests for its origin off, in seconds.
pub const NOT_NOW_S: i64 = 24 * 3600;
/// The window [`MAX_NEW_PER_SESSION`] counts in, in seconds.
pub const WINDOW_S: i64 = 24 * 3600;
/// The most new requests one session opens in [`WINDOW_S`].
pub const MAX_NEW_PER_SESSION: i64 = 3;

/// Where a request is in its life: `Open` moves once to any other status,
/// which is final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Connected,
    Dismissed,
    Muted,
    Expired,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Connected => "connected",
            Status::Dismissed => "dismissed",
            Status::Muted => "muted",
            Status::Expired => "expired",
        }
    }

    fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "open" => Status::Open,
            "connected" => Status::Connected,
            "dismissed" => Status::Dismissed,
            "muted" => Status::Muted,
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

/// A connection request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Request {
    pub id: String,
    /// The instance's origin (`https://clax.example`).
    pub origin: String,
    /// Its name, as its discovery gave it.
    pub name: String,
    pub status: Status,
    /// The session that opened it.
    pub opened_by: String,
    pub created_at: String,
    /// When it stopped being open.
    pub answered_at: Option<String>,
    /// While open, the remote a Connect started signing in; once
    /// `connected`, the remote.
    pub remote_id: Option<String>,
    /// When it expires while open.
    pub expires_at: String,
    /// Every asking session, the opener first.
    pub askers: Vec<Asker>,
}

/// One session's ask, folded into a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Asker {
    pub session_id: String,
    /// The URL it wanted to read there, when it gave one.
    pub url: Option<String>,
    pub reason: Option<String>,
    pub asked_at: String,
}

/// An agent's ask ([`Store::ask_remote`]).
#[derive(Clone, Debug, Default)]
pub struct Ask {
    /// The instance's origin, normalized.
    pub origin: String,
    /// Its name, from its discovery.
    pub name: String,
    /// The asking session.
    pub session: String,
    pub url: Option<String>,
    pub reason: Option<String>,
    /// Where the asking agent was working (the artifact and thread its
    /// person was last told to look at), for the request's inbox item.
    pub artifact_id: Option<String>,
    pub thread_id: Option<String>,
}

/// What an ask came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// A new request: the owner is told.
    Opened(Request),
    /// The open request for the origin, which the ask joined.
    Joined(Request),
    /// The origin is connected already: the remote's ID.
    Connected(String),
    /// The owner muted the origin.
    Muted,
    /// A "Not now" holds new requests for the origin off until `until`.
    Backoff { until: String },
    /// The session opened [`MAX_NEW_PER_SESSION`] in the last day.
    Capped,
}

/// How a request was answered ([`Store::answer_remote_request`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The owner chose Connect: remote `0`'s sign-in started. The request
    /// stays open until it ends. Recorded only when no sign-in of another
    /// Connect still waits for it: the request read after says whose is.
    Connecting(String),
    /// The sign-in finished: remote `0` is connected.
    Connected(String),
    /// "Not now": dismissed, and new requests for the origin held off.
    NotNow,
    /// "Never for this instance": the origin is muted.
    Never,
    /// The sign-in ended without a connection: denied on the remote
    /// (`hold_off`, as a "Not now"), or expired.
    Dismissed { hold_off: bool },
}

const COLS: &str = "id, origin, name, status, opened_by, created_at, answered_at, remote_id";

/// `t` moved by `secs` seconds, as the store writes times.
fn shifted(now: &chrono::DateTime<chrono::Utc>, secs: i64) -> String {
    stamp(*now + chrono::Duration::seconds(secs))
}

fn askers(c: &Connection, id: &str) -> Result<Vec<Asker>> {
    let mut stmt = c.prepare_cached(
        "SELECT a.session_id, a.url, a.reason, a.asked_at FROM remote_request_askers a
         JOIN remote_requests r ON r.id = a.request_id
         WHERE a.request_id = ?1
         ORDER BY a.session_id = r.opened_by DESC, a.asked_at, a.session_id",
    )?;
    Ok(stmt
        .query_map(params![id], |r| {
            Ok(Asker {
                session_id: r.get(0)?,
                url: r.get(1)?,
                reason: r.get(2)?,
                asked_at: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// Request `id` as stored, with its askers; `None` when there is none.
fn read(c: &Connection, id: &str) -> Result<Option<Request>> {
    let row = c
        .query_row(
            &format!("SELECT {COLS} FROM remote_requests WHERE id = ?1"),
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((id, origin, name, status, opened_by, created_at, answered_at, remote_id)) = row
    else {
        return Ok(None);
    };
    let status = Status::parse(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            "remote_requests.status is not a value this store writes".into(),
        )
    })?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(&created_at)
        .map(|t| shifted(&t.with_timezone(&chrono::Utc), REQUEST_LIFE_S))
        .unwrap_or_default();
    let askers = askers(c, &id)?;
    Ok(Some(Request {
        id,
        origin,
        name,
        status,
        opened_by,
        created_at,
        answered_at,
        remote_id,
        expires_at,
        askers,
    }))
}

/// A `remote_request.*` event of request `id` (of `origin`) at `now`.
fn record(kind: AuditKind, id: &str, origin: &str, now: &str) -> AuditRecord {
    AuditRecord::new(kind, now)
        .with("request_id", id)
        .with("origin", origin)
}

/// Closes open request `id` with `status` at `now` (and `remote` when
/// given), reads its inbox item, and records `rec` (its
/// `remote_request.*` event) as `ctx`; nothing for a request no longer
/// open.
#[allow(clippy::too_many_arguments)]
fn close(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    id: &str,
    status: Status,
    remote: Option<&str>,
    now: &str,
    rec: AuditRecord,
) -> Result<()> {
    let closed = tx.execute(
        "UPDATE remote_requests SET status = ?2, answered_at = ?3, remote_id = COALESCE(?4, remote_id)
         WHERE id = ?1 AND status = 'open'",
        params![id, status.as_str(), now, remote],
    )?;
    if closed == 0 {
        return Ok(());
    }
    tx.execute(
        "UPDATE inbox_items SET read_at = COALESCE(read_at, ?2) WHERE key = ?1",
        params![inbox::remote_request_key(id), now],
    )?;
    st.record_audit(tx, ctx, rec)?;
    Ok(())
}

/// Request `id` (of `origin`) closed with `status` and `remote`, as
/// [`close`] does, recording `remote_request.close`.
#[allow(clippy::too_many_arguments)]
fn settle_one(
    st: &Store,
    tx: &Transaction<'_>,
    ctx: &AuditCtx,
    id: &str,
    origin: &str,
    status: Status,
    remote: Option<&str>,
    now: &str,
) -> Result<()> {
    let mut rec =
        record(AuditKind::RemoteRequestClose, id, origin, now).with("status", status.as_str());
    if let Some(r) = remote {
        rec = rec.with("remote_id", r);
    }
    close(st, tx, ctx, id, status, remote, now, rec)
}

/// Settles every open request that is no longer waiting, as
/// `system:daemon`: one whose origin is connected becomes `connected`,
/// one older than [`REQUEST_LIFE_S`] `expired`.
fn settle(st: &Store, tx: &Transaction<'_>, now: &chrono::DateTime<chrono::Utc>) -> Result<()> {
    let c: &Connection = tx;
    let at = stamp(*now);
    let cutoff = shifted(now, -REQUEST_LIFE_S);
    let due: Vec<(String, String, Option<String>, bool)> = {
        let mut stmt = c.prepare_cached(
            "SELECT q.id, q.origin,
                (SELECT r.id FROM remotes r WHERE r.origin = q.origin AND r.status = 'connected'),
                q.created_at <= ?1
             FROM remote_requests q WHERE q.status = 'open'",
        )?;
        stmt.query_map(params![cutoff], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let ctx = &AuditCtx::DAEMON;
    for (id, origin, connected, old) in due {
        match (connected, old) {
            (Some(rid), _) => settle_one(
                st,
                tx,
                ctx,
                &id,
                &origin,
                Status::Connected,
                Some(&rid),
                &at,
            )?,
            (None, true) => settle_one(st, tx, ctx, &id, &origin, Status::Expired, None, &at)?,
            (None, false) => {}
        }
    }
    Ok(())
}

impl Store {
    /// Folds `a` into the open request for its origin, or opens one, as the
    /// module comment says, recording `remote_request.ask` as `ctx` (the
    /// asking session's agent). An opened request makes its inbox item, in
    /// `a`'s place.
    ///
    /// # Errors
    /// The database's.
    pub fn ask_remote(&self, ctx: &AuditCtx, a: &Ask) -> Result<Asked> {
        let now_utc = self.now_utc();
        self.with_tx(|tx| {
            let now = stamp(now_utc);
            settle(self, tx, &now_utc)?;
            if let Some(early) = answered_before_asking(tx, &a.origin, &a.session, &now_utc)? {
                return Ok(early);
            }
            let open: Option<String> = tx
                .query_row(
                    "SELECT id FROM remote_requests WHERE origin = ?1 AND status = 'open'",
                    params![a.origin],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = open {
                tx.execute(
                    "INSERT INTO remote_request_askers (request_id, session_id, url, reason, asked_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (request_id, session_id) DO UPDATE SET
                        url = COALESCE(excluded.url, url), reason = COALESCE(excluded.reason, reason)",
                    params![id, a.session, a.url, a.reason, now],
                )?;
                let rec = record(AuditKind::RemoteRequestAsk, &id, &a.origin, &now).with("opened", false);
                self.record_audit(tx, ctx, rec)?;
                return Ok(Asked::Joined(read(tx, &id)?.ok_or(CoreError::NotFound)?));
            }
            let id = new_ulid();
            tx.execute(
                "INSERT INTO remote_requests (id, origin, name, status, opened_by, created_at)
                 VALUES (?1, ?2, ?3, 'open', ?4, ?5)",
                params![id, a.origin, a.name, a.session, now],
            )?;
            tx.execute(
                "INSERT INTO remote_request_askers (request_id, session_id, url, reason, asked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, a.session, a.url, a.reason, now],
            )?;
            let detail = json!({
                "request_id": id,
                "origin": a.origin,
                "name": a.name,
                "url": a.url,
                "reason": a.reason,
            });
            let text = [Some(a.name.as_str()), Some(a.origin.as_str()), a.url.as_deref(), a.reason.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            inbox::note_remote_request(
                tx,
                &id,
                &a.session,
                (a.artifact_id.as_deref(), a.thread_id.as_deref()),
                detail,
                &text,
                &now,
            )?;
            let rec = record(AuditKind::RemoteRequestAsk, &id, &a.origin, &now).with("opened", true);
            self.record_audit(tx, ctx, rec)?;
            Ok(Asked::Opened(read(tx, &id)?.ok_or(CoreError::NotFound)?))
        })
    }

    /// What session `session`'s ask about `origin` answers without a
    /// request to open or join, as [`Store::ask_remote`] would answer it
    /// now: `Connected`, `Muted`, and, while no request for it is open,
    /// `Backoff` or `Capped`; `None` when the ask would open or join one.
    /// The daemon asks this before it fetches the origin's discovery, so a
    /// muted, held-off or capped ask fetches nothing.
    ///
    /// # Errors
    /// The database's.
    pub fn remote_ask_answer(&self, origin: &str, session: &str) -> Result<Option<Asked>> {
        let now_utc = self.now_utc();
        self.with_tx(|tx| {
            settle(self, tx, &now_utc)?;
            answered_before_asking(tx, origin, session, &now_utc)
        })
    }

    /// Request `id`, settled first (expired, or connected elsewhere).
    ///
    /// # Errors
    /// `NotFound` for no such request.
    pub fn remote_request(&self, id: &str) -> Result<Request> {
        let now_utc = self.now_utc();
        self.with_tx(|tx| {
            settle(self, tx, &now_utc)?;
            read(tx, id)?.ok_or(CoreError::NotFound)
        })
    }

    /// Request `id` as stored, changing nothing: for views, which read. An
    /// open request past its life reads as `expired`.
    ///
    /// # Errors
    /// The database's.
    pub fn peek_remote_request(&self, id: &str) -> Result<Option<Request>> {
        let now = self.now();
        self.with_read(|c| {
            Ok(read(c, id)?.map(|mut r| {
                if r.status == Status::Open && r.expires_at <= now {
                    r.status = Status::Expired;
                }
                r
            }))
        })
    }

    /// Every open request, oldest first, settled first.
    ///
    /// # Errors
    /// The database's.
    pub fn open_remote_requests(&self) -> Result<Vec<Request>> {
        let now_utc = self.now_utc();
        self.with_tx(|tx| {
            settle(self, tx, &now_utc)?;
            let ids: Vec<String> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT id FROM remote_requests WHERE status = 'open' ORDER BY created_at, id",
                )?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            let mut out = Vec::new();
            for id in ids {
                out.extend(read(tx, &id)?);
            }
            Ok(out)
        })
    }

    /// Records answer `ans` to request `id` as `ctx`: the owner's
    /// (`Connecting`, `NotNow`, `Never`) as `remote_request.answer`, the
    /// end of its Connect's sign-in (`Connected`, `Dismissed`) as
    /// `remote_request.close`. The request after it.
    ///
    /// # Errors
    /// `NotFound` for no such request; `request_closed` for one no longer
    /// open (answered, or settled as connected or expired).
    pub fn answer_remote_request(&self, ctx: &AuditCtx, id: &str, ans: Answer) -> Result<Request> {
        let now_utc = self.now_utc();
        self.with_tx(|tx| {
            let now = stamp(now_utc);
            settle(self, tx, &now_utc)?;
            let r = read(tx, id)?.ok_or(CoreError::NotFound)?;
            if r.status != Status::Open {
                return Err(CoreError::invalid(
                    "request_closed",
                    format!("this request is {} already", r.status),
                ));
            }
            let answered = |answer: &str| {
                record(AuditKind::RemoteRequestAnswer, id, &r.origin, &now).with("answer", answer)
            };
            match &ans {
                Answer::Connecting(rid) => {
                    tx.execute(
                        "UPDATE remote_requests SET remote_id = ?2 WHERE id = ?1
                            AND (remote_id IS NULL OR remote_id NOT IN
                                (SELECT id FROM remotes WHERE status = 'pending'))",
                        params![id, rid],
                    )?;
                    self.record_audit(
                        tx,
                        ctx,
                        answered("connect").with("remote_id", rid.as_str()),
                    )?;
                }
                Answer::Connected(rid) => {
                    settle_one(
                        self,
                        tx,
                        ctx,
                        id,
                        &r.origin,
                        Status::Connected,
                        Some(rid),
                        &now,
                    )?;
                }
                Answer::NotNow | Answer::Dismissed { hold_off: true } => {
                    match ans {
                        Answer::NotNow => {
                            close(
                                self,
                                tx,
                                ctx,
                                id,
                                Status::Dismissed,
                                None,
                                &now,
                                answered("not_now"),
                            )?;
                        }
                        _ => {
                            settle_one(self, tx, ctx, id, &r.origin, Status::Dismissed, None, &now)?
                        }
                    }
                    tx.execute(
                        "INSERT INTO remote_dismissals (origin, until) VALUES (?1, ?2)
                         ON CONFLICT (origin) DO UPDATE SET until = excluded.until",
                        params![r.origin, shifted(&now_utc, NOT_NOW_S)],
                    )?;
                }
                Answer::Dismissed { hold_off: false } => {
                    settle_one(self, tx, ctx, id, &r.origin, Status::Dismissed, None, &now)?;
                }
                Answer::Never => {
                    close(
                        self,
                        tx,
                        ctx,
                        id,
                        Status::Muted,
                        None,
                        &now,
                        answered("never"),
                    )?;
                    tx.execute(
                        "INSERT OR IGNORE INTO remote_mutes (origin, muted_at) VALUES (?1, ?2)",
                        params![r.origin, now],
                    )?;
                }
            }
            read(tx, id)?.ok_or(CoreError::NotFound)
        })
    }

    /// The open request whose Connect started remote `rid`'s sign-in.
    ///
    /// # Errors
    /// The database's.
    pub fn remote_request_of(&self, rid: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT id FROM remote_requests WHERE remote_id = ?1 AND status = 'open'",
                params![rid],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Mutes `origin`: its requests answer `muted` and nobody is asked.
    ///
    /// # Errors
    /// The database's.
    pub fn mute_origin(&self, origin: &str) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO remote_mutes (origin, muted_at) VALUES (?1, ?2)",
                params![origin, now],
            )?;
            Ok(())
        })
    }

    /// Unmutes `origin`; whether it was muted.
    ///
    /// # Errors
    /// The database's.
    pub fn unmute_origin(&self, origin: &str) -> Result<bool> {
        self.with_tx(|tx| {
            Ok(tx.execute(
                "DELETE FROM remote_mutes WHERE origin = ?1",
                params![origin],
            )? > 0)
        })
    }

    /// The muted origins and when each was muted, newest first.
    ///
    /// # Errors
    /// The database's.
    pub fn remote_mutes(&self) -> Result<Vec<(String, String)>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT origin, muted_at FROM remote_mutes ORDER BY muted_at DESC, origin",
            )?;
            Ok(stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        })
    }
}

/// What an ask about `origin` by `session` answers in `tx` without opening
/// or joining a request: the remote connected, the origin muted, and,
/// while no request for it is open, its hold-off or the session's cap.
fn answered_before_asking(
    tx: &rusqlite::Connection,
    origin: &str,
    session: &str,
    now_utc: &chrono::DateTime<chrono::Utc>,
) -> Result<Option<Asked>> {
    let now = stamp(*now_utc);
    let connected: Option<String> = tx
        .query_row(
            "SELECT id FROM remotes WHERE origin = ?1 AND status = 'connected'",
            params![origin],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(rid) = connected {
        return Ok(Some(Asked::Connected(rid)));
    }
    let muted: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM remote_mutes WHERE origin = ?1)",
        params![origin],
        |r| r.get(0),
    )?;
    if muted {
        return Ok(Some(Asked::Muted));
    }
    let open: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM remote_requests WHERE origin = ?1 AND status = 'open')",
        params![origin],
        |r| r.get(0),
    )?;
    if open {
        return Ok(None);
    }
    let until: Option<String> = tx
        .query_row(
            "SELECT until FROM remote_dismissals WHERE origin = ?1 AND until > ?2",
            params![origin, now],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(until) = until {
        return Ok(Some(Asked::Backoff { until }));
    }
    let opened: i64 = tx.query_row(
        "SELECT COUNT(*) FROM remote_requests WHERE opened_by = ?1 AND created_at > ?2",
        params![session, shifted(now_utc, -WINDOW_S)],
        |r| r.get(0),
    )?;
    if opened >= MAX_NEW_PER_SESSION {
        return Ok(Some(Asked::Capped));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{session, store_with_clock};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    const ORIGIN: &str = "https://clax.example";

    fn ask(session: &str, url: &str) -> Ask {
        Ask {
            origin: ORIGIN.into(),
            name: "Example".into(),
            session: session.into(),
            url: Some(url.into()),
            reason: Some("to read the plan".into()),
            ..Ask::default()
        }
    }

    fn opened(a: Asked) -> Request {
        match a {
            Asked::Opened(r) => r,
            other => panic!("not opened: {other:?}"),
        }
    }

    fn clocked() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::at("2026-10-29T10:00:00Z"));
        let (dir, st) = store_with_clock(clock.clone());
        (dir, st, clock)
    }

    /// The remote-request items in the owner's inbox.
    fn items(st: &Store) -> i64 {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM inbox_items WHERE kind = 'remote_request'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }

    #[test]
    fn asks_fold_into_one_open_request_per_origin() {
        let (_d, st, _c) = clocked();
        let (s1, s2) = (session(&st, "claude", "h1"), session(&st, "codex", "h2"));
        let r = opened(
            st.ask_remote(
                &AuditCtx::DAEMON,
                &ask(&s1, "https://clax.example/a/aaaaaaaaaaaa"),
            )
            .unwrap(),
        );
        assert_eq!(r.status, Status::Open);
        assert_eq!(r.opened_by, s1);
        assert_eq!(items(&st), 1, "the owner hears once, when it opens");
        let Asked::Joined(j) = st
            .ask_remote(
                &AuditCtx::DAEMON,
                &ask(&s2, "https://clax.example/a/bbbbbbbbbbbb"),
            )
            .unwrap()
        else {
            panic!("a second ask joins the open request");
        };
        assert_eq!(j.id, r.id);
        assert_eq!(
            j.askers
                .iter()
                .map(|a| (a.session_id.as_str(), a.url.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (s1.as_str(), Some("https://clax.example/a/aaaaaaaaaaaa")),
                (s2.as_str(), Some("https://clax.example/a/bbbbbbbbbbbb"))
            ]
        );
        // The same session asking again joins too, and changes nothing else.
        assert!(matches!(
            st.ask_remote(
                &AuditCtx::DAEMON,
                &ask(&s1, "https://clax.example/a/aaaaaaaaaaaa")
            )
            .unwrap(),
            Asked::Joined(_)
        ));
        assert_eq!(st.open_remote_requests().unwrap().len(), 1);
        assert_eq!(items(&st), 1);
        // Another origin is a request of its own.
        let mut other = ask(&s2, "https://other.example/a/cccccccccccc");
        other.origin = "https://other.example".into();
        opened(st.ask_remote(&AuditCtx::DAEMON, &other).unwrap());
        assert_eq!(st.open_remote_requests().unwrap().len(), 2);
        assert_eq!(items(&st), 2);
    }

    #[test]
    fn not_now_holds_an_origin_off_for_a_day() {
        let (_d, st, clock) = clocked();
        let s1 = session(&st, "claude", "h1");
        let r = opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        let a = st
            .answer_remote_request(&AuditCtx::DAEMON, &r.id, Answer::NotNow)
            .unwrap();
        assert_eq!(a.status, Status::Dismissed);
        let read: Option<String> = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT read_at FROM inbox_items WHERE kind = 'remote_request'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(read.is_some(), "an answered request's item is read");
        clock.advance(NOT_NOW_S - 1);
        assert_eq!(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
            Asked::Backoff {
                until: "2026-10-30T10:00:00.000Z".into()
            }
        );
        clock.advance(1);
        opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        // A closed request takes no second answer.
        let e = st
            .answer_remote_request(&AuditCtx::DAEMON, &r.id, Answer::Never)
            .unwrap_err();
        assert!(
            matches!(
                e,
                CoreError::Invalid {
                    code: "request_closed",
                    ..
                }
            ),
            "{e:?}"
        );
    }

    #[test]
    fn never_mutes_until_unmuted() {
        let (_d, st, clock) = clocked();
        let s1 = session(&st, "claude", "h1");
        let r = opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        assert_eq!(
            st.answer_remote_request(&AuditCtx::DAEMON, &r.id, Answer::Never)
                .unwrap()
                .status,
            Status::Muted
        );
        clock.advance(30 * 24 * 3600);
        assert_eq!(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
            Asked::Muted
        );
        assert_eq!(
            st.remote_mutes().unwrap(),
            vec![(ORIGIN.to_string(), "2026-10-29T10:00:00.000Z".to_string())]
        );
        assert!(st.unmute_origin(ORIGIN).unwrap());
        assert!(!st.unmute_origin(ORIGIN).unwrap());
        opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        st.mute_origin(ORIGIN).unwrap();
        // A mute set while a request is open stops new asks, joins included.
        assert_eq!(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
            Asked::Muted
        );
    }

    #[test]
    fn a_session_opens_at_most_three_requests_a_day() {
        let (_d, st, clock) = clocked();
        let (s1, s2) = (session(&st, "claude", "h1"), session(&st, "codex", "h2"));
        let on = |n: usize, s: &str| {
            let mut a = ask(s, "https://x/a/y");
            a.origin = format!("https://c{n}.example");
            a
        };
        for n in 0..3 {
            opened(st.ask_remote(&AuditCtx::DAEMON, &on(n, &s1)).unwrap());
        }
        assert_eq!(
            st.ask_remote(&AuditCtx::DAEMON, &on(3, &s1)).unwrap(),
            Asked::Capped
        );
        // Joining another's request is not a new one.
        opened(st.ask_remote(&AuditCtx::DAEMON, &on(4, &s2)).unwrap());
        assert!(matches!(
            st.ask_remote(&AuditCtx::DAEMON, &on(4, &s1)).unwrap(),
            Asked::Joined(_)
        ));
        // Another session has its own three.
        opened(st.ask_remote(&AuditCtx::DAEMON, &on(5, &s2)).unwrap());
        // A day after the first three, the session may open more.
        clock.advance(WINDOW_S);
        opened(st.ask_remote(&AuditCtx::DAEMON, &on(3, &s1)).unwrap());
    }

    #[test]
    fn an_open_request_expires_after_a_day() {
        let (_d, st, clock) = clocked();
        let s1 = session(&st, "claude", "h1");
        let r = opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        assert_eq!(r.expires_at, "2026-10-30T10:00:00.000Z");
        clock.advance(REQUEST_LIFE_S - 1);
        assert_eq!(st.remote_request(&r.id).unwrap().status, Status::Open);
        clock.advance(1);
        assert_eq!(
            st.peek_remote_request(&r.id).unwrap().unwrap().status,
            Status::Expired,
            "a view reads it expired before anything settles it"
        );
        let e = st.remote_request(&r.id).unwrap();
        assert_eq!(
            (e.status.as_str(), e.answered_at.as_deref()),
            ("expired", Some("2026-10-30T10:00:00.000Z"))
        );
        // Settled as the daemon, its close recorded once.
        let closes: Vec<_> = st
            .events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "remote_request.close")
            .collect();
        assert_eq!(closes.len(), 1, "{closes:?}");
        let body: serde_json::Value = serde_json::from_str(&closes[0].body).unwrap();
        assert_eq!(
            (body["request_id"].as_str(), body["status"].as_str()),
            (Some(r.id.as_str()), Some("expired"))
        );
        assert!(
            closes[0].actor.contains("\"system\""),
            "{}",
            closes[0].actor
        );
        assert!(st.open_remote_requests().unwrap().is_empty());
        // An expired request holds nothing off: the next ask opens anew.
        let n = opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        assert_ne!(n.id, r.id);
    }

    #[test]
    fn a_connect_settles_when_its_remote_connects() {
        let (_d, st, _c) = clocked();
        let s1 = session(&st, "claude", "h1");
        let r = opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
        let c = st
            .answer_remote_request(&AuditCtx::DAEMON, &r.id, Answer::Connecting("r_1".into()))
            .unwrap();
        assert_eq!(
            (c.status.as_str(), c.remote_id.as_deref()),
            ("open", Some("r_1"))
        );
        assert_eq!(st.remote_request_of("r_1").unwrap(), Some(r.id.clone()));
        // r_1 is no pending remote: a later Connect's sign-in replaces it.
        let p = st
            .add_pending_remote(
                ORIGIN,
                "example",
                &crate::remotes::PendingSignIn {
                    device_code: clax_core::secret::Secret::new("dc".into()),
                    user_code: "ABCD-1234".into(),
                    verification_url: format!("{ORIGIN}/device?code=ABCD-1234"),
                    expires_at: "2026-10-29T10:10:00.000Z".into(),
                    interval_s: 5,
                },
            )
            .unwrap();
        let c = st
            .answer_remote_request(&AuditCtx::DAEMON, &r.id, Answer::Connecting(p.id.clone()))
            .unwrap();
        assert_eq!(c.remote_id.as_deref(), Some(p.id.as_str()));
        // While that sign-in waits, a second Connect's records nothing.
        let c = st
            .answer_remote_request(&AuditCtx::DAEMON, &r.id, Answer::Connecting("r_2".into()))
            .unwrap();
        assert_eq!(c.remote_id.as_deref(), Some(p.id.as_str()));
        let d = st
            .answer_remote_request(
                &AuditCtx::DAEMON,
                &r.id,
                Answer::Dismissed { hold_off: false },
            )
            .unwrap();
        assert_eq!(d.status, Status::Dismissed);
        // An expired sign-in holds nothing off.
        opened(
            st.ask_remote(&AuditCtx::DAEMON, &ask(&s1, "https://clax.example/a/x"))
                .unwrap(),
        );
    }
}
