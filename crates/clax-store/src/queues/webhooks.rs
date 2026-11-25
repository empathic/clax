//! Report queues' outbound webhooks (spec 2026-10-27-clax-sdk-design §9,
//! [`clax_core::webhook`]): the endpoints a queue's admins name and the
//! outbox of their deliveries.
//!
//! A change to a report that is an [`Event`] (filing, claim, release,
//! reply, fix, verdict, reopen, resolve) queues one delivery per endpoint
//! of the queue that chose the event, in the change's own transaction
//! ([`Store::webhook_event_in`]): the event is never told without the
//! change, nor the change made without its event. Each event has one ID,
//! the same in every endpoint's delivery and on every attempt. The server
//! takes due deliveries ([`Store::take_due_webhooks`]), sends them and
//! records each outcome ([`Store::record_webhook_outcome`]): 2xx delivers
//! it; anything else waits [`backoff`] and is tried again, until
//! [`MAX_ATTEMPTS`], when it is `failed` and its endpoint marked failing
//! until a later delivery to it succeeds. An admin redelivers any report
//! delivery ([`Store::redeliver_webhook`]): it is due at once with a fresh
//! count of attempts, under the same event ID. Each endpoint keeps its
//! latest [`KEPT_DELIVERIES`] finished deliveries as its log.
//!
//! An endpoint is named unverified, with one delivery of
//! [`VERIFY_EVENT`] queued in the same transaction; its answer, checked
//! by the sender ([`clax_core::webhook::check_answer`]), verifies it
//! ([`Store::record_webhook_outcome`]). A verification is tried once:
//! another is the admin's to ask for ([`Store::verify_webhook`]), at most
//! [`VERIFY_SENDS_PER_HOUR`] an hour and within [`VERIFY_WINDOW_SECS`]
//! of being named or re-enabled ([`Webhook::verify_by`]). Only a verified endpoint that
//! is not disabled is queued report events, at most [`MAX_PER_HOUR`] an
//! hour; none is back-filled once it verifies. [`DISABLE_AFTER`]
//! deliveries in a row given up disable it and give up its pending ones;
//! [`Store::verify_webhook`] re-enables it, unverified until it answers
//! again.
//!
//! The endpoint's secret is kept in the clear: the server signs each
//! delivery with it. It is shown once, when the endpoint is made.

use super::{Visibility, queue_in, queue_record, report_in};
use crate::Store;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_core::secret::Secret;
use clax_core::webhook::{
    DISABLE_AFTER, EVENT_PREFIX, Event, IN_FLIGHT_SECS, MAX_ATTEMPTS, MAX_ENDPOINTS, MAX_PER_HOUR,
    PER_ENDPOINT, Payload, SECRET_PREFIX, VERIFY_EVENT, VERIFY_SENDS_PER_HOST_HOUR,
    VERIFY_SENDS_PER_HOUR, VERIFY_SENDS_PER_QUEUE_HOUR, VERIFY_WINDOW_SECS, VerifyPayload, backoff,
    check_url, logged, parse_events,
};
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

/// Finished deliveries (delivered or failed) an endpoint keeps as its log;
/// older ones are removed as new ones are queued.
pub const KEPT_DELIVERIES: i64 = 200;

/// An endpoint as its queue's admins read it: never its secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Webhook {
    pub id: String,
    #[serde(skip)]
    pub queue_id: String,
    pub url: String,
    /// The events it is sent, by name ([`Event::as_str`]).
    pub events: Vec<String>,
    pub created_at: String,
    /// `unverified` (sent no report event until it answers a
    /// verification), `verified`, `disabled` (after [`DISABLE_AFTER`]
    /// deliveries in a row were given up) or `expired` (unverified past
    /// `verify_by`: inert until removed).
    pub state: &'static str,
    /// When it answered its verification.
    pub verified_at: Option<String>,
    /// Until when an unverified endpoint may be verified; `None` once it
    /// is.
    pub verify_by: Option<String>,
    pub disabled_at: Option<String>,
    /// Deliveries given up in a row since the last one delivered.
    pub given_up: u32,
    /// Since when its deliveries fail: set when one is given up, cleared
    /// when one is delivered.
    pub failing_since: Option<String>,
    /// Why its latest given-up delivery, or its latest verification,
    /// failed.
    pub last_error: Option<String>,
    /// Events dropped for it past [`MAX_PER_HOUR`], ever.
    pub dropped: u64,
    pub last_dropped_at: Option<String>,
}

/// One delivery of an event to an endpoint, as the endpoint's log shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Delivery {
    pub id: String,
    pub webhook_id: String,
    pub event_id: String,
    pub event: String,
    /// `None` for a verification ([`VERIFY_EVENT`]).
    pub report_id: Option<String>,
    /// `pending`, `delivered` or `failed`.
    pub state: String,
    pub attempts: u32,
    /// When a pending one is next tried.
    pub next_at: Option<String>,
    pub last_at: Option<String>,
    /// The endpoint's last answer's status; `None` when none came.
    pub last_status: Option<u16>,
    /// Why the last attempt failed: the answer's body or the error.
    pub last_error: Option<String>,
    pub delivered_at: Option<String>,
    pub created_at: String,
}

/// A delivery taken for sending ([`Store::take_due_webhooks`]).
#[derive(Clone, Debug)]
pub struct DueDelivery {
    pub id: String,
    pub webhook_id: String,
    pub event_id: String,
    pub event: String,
    pub url: String,
    pub secret: Secret,
    /// The payload as queued, its `report.link` unset.
    pub payload: String,
    /// Attempts made before this one: what
    /// [`Store::record_webhook_outcome`] is given back.
    pub attempts: u32,
}

/// How one attempt ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The endpoint answered 2xx.
    Delivered { status: u16 },
    /// It answered `status` (non-2xx), or none came (`None`).
    Failed { status: Option<u16>, error: String },
}

const WEBHOOK_SELECT: &str = "SELECT id, queue_id, url, events_json, created_at, failing_since,
    last_error, verify_since, verified_at, disabled_at, given_up, dropped, last_dropped_at
    FROM queue_webhooks";

/// `at` (a stored stamp) `secs` seconds on.
fn later(at: &str, secs: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc) + chrono::Duration::seconds(secs))
}

/// Reads a [`WEBHOOK_SELECT`] row, its state as of `now`.
fn webhook_row(
    r: &rusqlite::Row<'_>,
    now: chrono::DateTime<chrono::Utc>,
) -> rusqlite::Result<Webhook> {
    let events: String = r.get(3)?;
    let verify_since: String = r.get(7)?;
    let verified_at: Option<String> = r.get(8)?;
    let disabled_at: Option<String> = r.get(9)?;
    let by = later(&verify_since, VERIFY_WINDOW_SECS);
    let state = if disabled_at.is_some() {
        "disabled"
    } else if verified_at.is_some() {
        "verified"
    } else if by.is_none_or(|by| now > by) {
        "expired"
    } else {
        "unverified"
    };
    Ok(Webhook {
        id: r.get(0)?,
        queue_id: r.get(1)?,
        url: r.get(2)?,
        events: serde_json::from_str(&events).unwrap_or_default(),
        created_at: r.get(4)?,
        state,
        verify_by: verified_at
            .is_none()
            .then(|| by.map(clax_core::time::stamp))
            .flatten(),
        verified_at,
        disabled_at,
        given_up: r.get(10)?,
        failing_since: r.get(5)?,
        last_error: r.get(6)?,
        dropped: r.get(11)?,
        last_dropped_at: r.get(12)?,
    })
}

const DELIVERY_SELECT: &str = "SELECT id, webhook_id, event_id, event, thread_id, state, attempts,
    next_at, last_at, last_status, last_error, delivered_at, created_at FROM webhook_deliveries";

fn delivery_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        id: r.get(0)?,
        webhook_id: r.get(1)?,
        event_id: r.get(2)?,
        event: r.get(3)?,
        report_id: r.get(4)?,
        state: r.get(5)?,
        attempts: r.get(6)?,
        next_at: r.get(7)?,
        last_at: r.get(8)?,
        last_status: r.get(9)?,
        last_error: r.get(10)?,
        delivered_at: r.get(11)?,
        created_at: r.get(12)?,
    })
}

fn webhook_in(
    c: &Connection,
    qid: &str,
    wid: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Webhook> {
    c.prepare_cached(&format!("{WEBHOOK_SELECT} WHERE id = ?1 AND queue_id = ?2"))?
        .query_row(params![wid, qid], |r| webhook_row(r, now))
        .optional()?
        .ok_or(CoreError::NotFound)
}

/// Queues a verification of endpoint `wid` at `url` of queue `qid`
/// (`queue_name`), due `now`, in `tx`, and counts it in
/// `webhook_verify_sends`, which outlives the endpoint.
///
/// # Errors
/// `verifications_limited` when the queue's endpoints were sent
/// [`VERIFY_SENDS_PER_QUEUE_HOUR`] verifications in the last hour, or
/// `url`'s host [`VERIFY_SENDS_PER_HOST_HOUR`] from any queue: nothing is
/// queued, and the caller's transaction rolls back.
fn verify_delivery_in(
    tx: &Transaction<'_>,
    qid: &str,
    queue_name: &str,
    wid: &str,
    url: &str,
    now: &str,
) -> Result<()> {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    let hour_ago = later(now, -3600).map_or_else(String::new, clax_core::time::stamp);
    tx.execute(
        "DELETE FROM webhook_verify_sends WHERE sent_at <= ?1",
        params![hour_ago],
    )?;
    let (by_queue, by_host): (i64, i64) = tx.query_row(
        "SELECT (SELECT COUNT(*) FROM webhook_verify_sends WHERE queue_id = ?1 AND sent_at > ?3),
                (SELECT COUNT(*) FROM webhook_verify_sends WHERE host = ?2 AND sent_at > ?3)",
        params![qid, host, hour_ago],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if by_queue as usize >= VERIFY_SENDS_PER_QUEUE_HOUR {
        return Err(CoreError::invalid(
            "verifications_limited",
            format!(
                "this queue's webhooks were sent {VERIFY_SENDS_PER_QUEUE_HOUR} verifications in the last hour, removed ones included; try again later"
            ),
        ));
    }
    if by_host as usize >= VERIFY_SENDS_PER_HOST_HOUR {
        return Err(CoreError::invalid(
            "verifications_limited",
            format!(
                "{host} was sent {VERIFY_SENDS_PER_HOST_HOUR} webhook verifications in the last hour; try again later"
            ),
        ));
    }
    tx.execute(
        "INSERT INTO webhook_verify_sends (queue_id, host, sent_at) VALUES (?1, ?2, ?3)",
        params![qid, host, now],
    )?;
    let event_id = format!("{EVENT_PREFIX}{}", new_ulid());
    let payload = VerifyPayload::new(&event_id, now, qid, queue_name, wid, url);
    let json = serde_json::to_string(&payload).expect("payloads serialise");
    tx.execute(
        "INSERT INTO webhook_deliveries (id, webhook_id, event_id, event, thread_id,
            payload_json, created_at, next_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?6)",
        params![new_ulid(), wid, event_id, VERIFY_EVENT, json, now],
    )?;
    Ok(())
}

/// Which deliveries may be sent: a verification, or a report event to a
/// verified endpoint that is not disabled (`d` the delivery, `w` its
/// endpoint).
const SENDABLE: &str = "(d.event = 'webhook.verify'
    OR (w.verified_at IS NOT NULL AND w.disabled_at IS NULL))";

impl Store {
    /// Queue `qid`'s endpoints, oldest first.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn webhooks(&self, qid: &str) -> Result<Vec<Webhook>> {
        let now = self.now_utc();
        self.with_read(|c| {
            queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            Ok(c.prepare_cached(&format!(
                "{WEBHOOK_SELECT} WHERE queue_id = ?1 ORDER BY created_at, id"
            ))?
            .query_map(params![qid], |r| webhook_row(r, now))?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Makes an endpoint of queue `qid` at `url`, sent `events` (every
    /// event when empty) once it is verified, by `person` (`None`: the
    /// owner), and its secret ([`SECRET_PREFIX`] and 32 random bytes),
    /// answered here alone; queues its verification ([`VERIFY_EVENT`]),
    /// due now.
    /// `hosted` holds the URL to a hosted Clax's rule
    /// ([`clax_core::webhook::check_url`]). Recorded as `queue.update
    /// {queue_id, op: "webhook_add", webhook_id, url, events}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `invalid_url`,
    /// `invalid_events`; `too_many_webhooks` past [`MAX_ENDPOINTS`];
    /// `verifications_limited` past the queue's or the host's hourly cap
    /// on verifications ([`VERIFY_SENDS_PER_QUEUE_HOUR`],
    /// [`VERIFY_SENDS_PER_HOST_HOUR`]): no endpoint is made.
    pub fn create_webhook(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        person: Option<&str>,
        url: &str,
        events: &[String],
        hosted: bool,
    ) -> Result<(Webhook, Secret)> {
        let url = check_url(url.trim(), hosted)?.to_string();
        let events: Vec<String> = parse_events(events)?
            .into_iter()
            .map(|e| e.as_str().to_string())
            .collect();
        let events_json = serde_json::to_string(&events).expect("strings serialise");
        let secret = clax_core::secret::token(SECRET_PREFIX);
        let id = new_ulid();
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let hook = self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM queue_webhooks WHERE queue_id = ?1",
                params![qid],
                |r| r.get(0),
            )?;
            if n as usize >= MAX_ENDPOINTS {
                return Err(CoreError::invalid(
                    "too_many_webhooks",
                    format!("a queue names at most {MAX_ENDPOINTS} webhook endpoints"),
                ));
            }
            tx.execute(
                "INSERT INTO queue_webhooks (id, queue_id, url, events_json, secret, created_at,
                    created_by, verify_since)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6)",
                params![id, qid, url, events_json, secret.expose(), now, person],
            )?;
            verify_delivery_in(tx, qid, &q.name, &id, &url, &now)?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "webhook_add")
                .with("webhook_id", id.as_str())
                .with("url", url.as_str())
                .with("events", serde_json::json!(events));
            self.record_audit(tx, ctx, rec)?;
            self.webhooks_queued();
            webhook_in(tx, qid, &id, now_utc)
        })?;
        Ok((hook, secret))
    }

    /// Sends endpoint `wid` of queue `qid` another verification, due now,
    /// with a new challenge; a `disabled` one is re-enabled first,
    /// unverified (its window starting now) until it answers. Recorded as
    /// `queue.update {queue_id, op: "webhook_verify", webhook_id,
    /// reenabled}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// endpoint of it; `already_verified`; `verification_expired` past
    /// its window (remove it and name it again); `limit_reached` past
    /// [`VERIFY_SENDS_PER_HOUR`] verifications in the last hour;
    /// `verifications_limited` past the queue's or the host's hourly cap,
    /// nothing changed.
    pub fn verify_webhook(&self, ctx: &AuditCtx, qid: &str, wid: &str) -> Result<Webhook> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let hour_ago = clax_core::time::stamp(now_utc - chrono::Duration::hours(1));
        self.with_tx(|tx| {
            let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let h = webhook_in(tx, qid, wid, now_utc)?;
            match h.state {
                "verified" => {
                    return Err(CoreError::invalid(
                        "already_verified",
                        "this webhook is verified already",
                    ));
                }
                "expired" => {
                    return Err(CoreError::invalid(
                        "verification_expired",
                        format!(
                            "this webhook was not verified within {} hours of being added; remove it and add it again",
                            VERIFY_WINDOW_SECS / 3600
                        ),
                    ));
                }
                _ => {}
            }
            let sent: i64 = tx.query_row(
                "SELECT COUNT(*) FROM webhook_deliveries
                 WHERE webhook_id = ?1 AND event = ?2 AND created_at > ?3",
                params![wid, VERIFY_EVENT, hour_ago],
                |r| r.get(0),
            )?;
            if sent as usize >= VERIFY_SENDS_PER_HOUR {
                return Err(CoreError::invalid(
                    "limit_reached",
                    format!(
                        "a webhook is sent at most {VERIFY_SENDS_PER_HOUR} verifications an hour; try again later"
                    ),
                ));
            }
            let reenabled = h.state == "disabled";
            if reenabled {
                tx.execute(
                    "UPDATE queue_webhooks SET disabled_at = NULL, verified_at = NULL,
                        verify_since = ?2, given_up = 0, failing_since = NULL, last_error = NULL
                     WHERE id = ?1",
                    params![wid, now],
                )?;
            }
            verify_delivery_in(tx, qid, &q.name, wid, &h.url, &now)?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "webhook_verify")
                .with("webhook_id", wid)
                .with("reenabled", reenabled);
            self.record_audit(tx, ctx, rec)?;
            self.webhooks_queued();
            webhook_in(tx, qid, wid, now_utc)
        })
    }

    /// Removes endpoint `wid` of queue `qid` and its deliveries, sent or
    /// not. Recorded as `queue.update {queue_id, op: "webhook_remove",
    /// webhook_id}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// endpoint of it.
    pub fn delete_webhook(&self, ctx: &AuditCtx, qid: &str, wid: &str) -> Result<()> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            webhook_in(tx, qid, wid, now_utc)?;
            tx.execute(
                "DELETE FROM webhook_deliveries WHERE webhook_id = ?1",
                params![wid],
            )?;
            tx.execute("DELETE FROM queue_webhooks WHERE id = ?1", params![wid])?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "webhook_remove")
                .with("webhook_id", wid);
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })
    }

    /// Endpoint `wid`'s deliveries, latest first, at most `limit`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// endpoint of it.
    pub fn webhook_deliveries(&self, qid: &str, wid: &str, limit: usize) -> Result<Vec<Delivery>> {
        let now = self.now_utc();
        self.with_read(|c| {
            queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            webhook_in(c, qid, wid, now)?;
            Ok(c.prepare_cached(&format!(
                "{DELIVERY_SELECT} WHERE webhook_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2"
            ))?
            .query_map(params![wid, limit as i64], delivery_row)?
            .collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Makes report delivery `did` of verified endpoint `wid` of queue
    /// `qid` due now, with no attempts made, whatever its state: the same
    /// event, ID and payload, sent again. Recorded as `queue.update
    /// {queue_id, op: "webhook_redeliver", webhook_id, delivery_id,
    /// event_id}`.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for no such
    /// endpoint of it or delivery of the endpoint; `not_verified` for an
    /// endpoint that is not `verified`, or a verification (sent again by
    /// [`Store::verify_webhook`] alone).
    pub fn redeliver_webhook(
        &self,
        ctx: &AuditCtx,
        qid: &str,
        wid: &str,
        did: &str,
    ) -> Result<Delivery> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        self.with_tx(|tx| {
            queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
            let h = webhook_in(tx, qid, wid, now_utc)?;
            let (event_id, event): (String, String) = tx
                .query_row(
                    "SELECT event_id, event FROM webhook_deliveries WHERE id = ?1 AND webhook_id = ?2",
                    params![did, wid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or(CoreError::NotFound)?;
            if event == VERIFY_EVENT {
                return Err(CoreError::invalid(
                    "not_verified",
                    "a verification is not redelivered: ask for a new one (Verify again)",
                ));
            }
            if h.state != "verified" {
                return Err(CoreError::invalid(
                    "not_verified",
                    format!("this webhook is {}: it is sent no report event until it is verified", h.state),
                ));
            }
            tx.execute(
                "UPDATE webhook_deliveries SET state = 'pending', attempts = 0, next_at = ?2,
                    delivered_at = NULL WHERE id = ?1",
                params![did, now],
            )?;
            let rec = queue_record(AuditKind::QueueUpdate, &now, qid)
                .with("op", "webhook_redeliver")
                .with("webhook_id", wid)
                .with("delivery_id", did)
                .with("event_id", event_id.as_str());
            self.record_audit(tx, ctx, rec)?;
            self.webhooks_queued();
            Ok(tx.query_row(
                &format!("{DELIVERY_SELECT} WHERE id = ?1"),
                params![did],
                delivery_row,
            )?)
        })
    }

    /// Queues `event` of report `tid` of queue `qid` for each verified,
    /// enabled endpoint of the queue that chose it, in the caller's
    /// transaction, after the change it reports: one event ID, the payload
    /// as the report reads now (to its triage members) with `comment` (a
    /// reply's, a reopen's) when given, due now. An endpoint queued
    /// [`MAX_PER_HOUR`] events in the hour since its window started is
    /// not queued this one: it is counted as `dropped` instead. Nothing is
    /// read or written for a queue with no such endpoint. Older finished
    /// deliveries of each endpoint past [`KEPT_DELIVERIES`] are removed.
    ///
    /// # Errors
    /// Database errors; `NotFound` for no such report.
    pub(crate) fn webhook_event_in(
        &self,
        tx: &Transaction<'_>,
        qid: &str,
        tid: &str,
        event: Event,
        comment: Option<&str>,
    ) -> Result<()> {
        let wanted: Vec<(String, Option<String>, u32)> = tx
            .prepare_cached(
                "SELECT id, events_json, window_start, window_count FROM queue_webhooks
                 WHERE queue_id = ?1 AND verified_at IS NOT NULL AND disabled_at IS NULL",
            )?
            .query_map(params![qid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, u32>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|(_, events, _, _)| {
                serde_json::from_str::<Vec<String>>(events)
                    .is_ok_and(|e| e.iter().any(|n| n == event.as_str()))
            })
            .map(|(id, _, start, count)| (id, start, count))
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let mut queued = Vec::new();
        for (wid, start, count) in wanted {
            // The hour's window: one that ended starts again now.
            let fresh = start
                .as_deref()
                .and_then(|s| later(s, 3600))
                .is_none_or(|end| now_utc >= end);
            let count = if fresh { 0 } else { count };
            if count >= MAX_PER_HOUR {
                tx.execute(
                    "UPDATE queue_webhooks SET dropped = dropped + 1, last_dropped_at = ?2
                     WHERE id = ?1",
                    params![wid, now],
                )?;
                continue;
            }
            tx.execute(
                "UPDATE queue_webhooks SET window_start = CASE WHEN ?3 THEN ?2 ELSE window_start END,
                    window_count = ?4 WHERE id = ?1",
                params![wid, now, fresh, count + 1],
            )?;
            queued.push(wid);
        }
        if queued.is_empty() {
            return Ok(());
        }
        let q = queue_in(tx, qid)?.ok_or(CoreError::NotAMember)?;
        let report = report_in(tx, &q, tid, Visibility::All, &now)?.ok_or(CoreError::NotFound)?;
        let event_id = format!("{EVENT_PREFIX}{}", new_ulid());
        let payload = Payload::of(&event_id, event, &now, &q.name, &report, comment);
        let json = serde_json::to_string(&payload).expect("payloads serialise");
        for wid in queued {
            tx.execute(
                "INSERT INTO webhook_deliveries (id, webhook_id, event_id, event, thread_id,
                    payload_json, created_at, next_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![new_ulid(), wid, event_id, event.as_str(), tid, json, now],
            )?;
            tx.execute(
                "DELETE FROM webhook_deliveries WHERE webhook_id = ?1 AND state != 'pending'
                   AND id NOT IN (SELECT id FROM webhook_deliveries WHERE webhook_id = ?1
                     AND state != 'pending' ORDER BY created_at DESC, id DESC LIMIT ?2)",
                params![wid, KEPT_DELIVERIES],
            )?;
        }
        self.webhooks_queued();
        Ok(())
    }

    /// Takes up to `limit` pending deliveries that are due, earliest
    /// first and at most [`PER_ENDPOINT`] of any one endpoint, for
    /// sending: each is due again [`IN_FLIGHT_SECS`] on, so a send whose
    /// outcome is never recorded is tried again, and no other pass takes
    /// it meanwhile. A report event is taken only for a verified endpoint
    /// that is not disabled; a verification always.
    ///
    /// # Errors
    /// Database errors.
    pub fn take_due_webhooks(&self, limit: usize) -> Result<Vec<DueDelivery>> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let later = clax_core::time::stamp(now_utc + chrono::Duration::seconds(IN_FLIGHT_SECS));
        self.with_tx(|tx| {
            let due: Vec<DueDelivery> = tx
                .prepare_cached(&format!(
                    "SELECT d.id, d.webhook_id, d.event_id, d.event, w.url, w.secret,
                        d.payload_json, d.attempts
                     FROM (SELECT id, webhook_id, event_id, event, payload_json, attempts, next_at,
                             ROW_NUMBER() OVER (PARTITION BY webhook_id ORDER BY next_at, id) AS k
                           FROM webhook_deliveries
                           WHERE state = 'pending' AND next_at <= ?1) d
                     JOIN queue_webhooks w ON w.id = d.webhook_id
                     WHERE d.k <= ?3 AND {SENDABLE}
                     ORDER BY d.next_at, d.id LIMIT ?2"
                ))?
                .query_map(params![now, limit as i64, PER_ENDPOINT as i64], |r| {
                    Ok(DueDelivery {
                        id: r.get(0)?,
                        webhook_id: r.get(1)?,
                        event_id: r.get(2)?,
                        event: r.get(3)?,
                        url: r.get(4)?,
                        secret: Secret::new(r.get(5)?),
                        payload: r.get(6)?,
                        attempts: r.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            for d in &due {
                tx.execute(
                    "UPDATE webhook_deliveries SET next_at = ?2 WHERE id = ?1",
                    params![d.id, later],
                )?;
            }
            Ok(due)
        })
    }

    /// Records how the attempt of delivery `did` taken with `attempts`
    /// made before it ended. Delivered: `delivered`, and its endpoint no
    /// longer failing, none given up in a row. Failed: due again after
    /// [`backoff`] of the attempts made, and the endpoint's other pending
    /// deliveries due no sooner; or, at [`MAX_ATTEMPTS`], `failed`, its
    /// endpoint failing since now (when it was not) with the error and
    /// one more given up in a row: at [`DISABLE_AFTER`] the endpoint is
    /// disabled and its pending deliveries given up.
    ///
    /// A verification ([`VERIFY_EVENT`], whose answer the sender checked)
    /// is tried once. Delivered, it verifies its endpoint, unless the
    /// endpoint is no longer unverified or its window has passed; failed,
    /// it is `failed` at once, the endpoint's `last_error` saying why.
    ///
    /// A delivery redelivered or removed since it was taken is left as it
    /// is now.
    ///
    /// # Errors
    /// Database errors.
    pub fn record_webhook_outcome(
        &self,
        did: &str,
        attempts: u32,
        outcome: &Outcome,
    ) -> Result<()> {
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let made = attempts + 1;
        self.with_tx(|tx| {
            let Some((wid, event)) = tx
                .query_row(
                    "SELECT webhook_id, event FROM webhook_deliveries
                     WHERE id = ?1 AND state = 'pending' AND attempts = ?2",
                    params![did, attempts],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?
            else {
                return Ok(());
            };
            if event == VERIFY_EVENT {
                let (status, error) = match outcome {
                    Outcome::Delivered { status } => (Some(*status), None),
                    Outcome::Failed { status, error } => (*status, Some(logged(error))),
                };
                tx.execute(
                    "UPDATE webhook_deliveries SET state = ?2, attempts = ?3, next_at = NULL,
                        last_at = ?4, last_status = ?5, last_error = ?6,
                        delivered_at = CASE WHEN ?6 IS NULL THEN ?4 END
                     WHERE id = ?1",
                    params![
                        did,
                        if error.is_none() { "delivered" } else { "failed" },
                        made,
                        now,
                        status,
                        error
                    ],
                )?;
                let since: Option<String> = tx
                    .query_row(
                        "SELECT verify_since FROM queue_webhooks
                         WHERE id = ?1 AND verified_at IS NULL AND disabled_at IS NULL",
                        params![wid],
                        |r| r.get(0),
                    )
                    .optional()?;
                let open = since
                    .as_deref()
                    .and_then(|s| later(s, VERIFY_WINDOW_SECS))
                    .is_some_and(|by| now_utc <= by);
                if open {
                    match &error {
                        None => tx.execute(
                            "UPDATE queue_webhooks SET verified_at = ?2, last_error = NULL
                             WHERE id = ?1",
                            params![wid, now],
                        )?,
                        Some(e) => tx.execute(
                            "UPDATE queue_webhooks SET last_error = ?2 WHERE id = ?1",
                            params![wid, format!("verification failed: {e}")],
                        )?,
                    };
                }
                return Ok(());
            }
            match outcome {
                Outcome::Delivered { status } => {
                    tx.execute(
                        "UPDATE webhook_deliveries SET state = 'delivered', attempts = ?2,
                            next_at = NULL, last_at = ?3, last_status = ?4, last_error = NULL,
                            delivered_at = ?3 WHERE id = ?1",
                        params![did, made, now, status],
                    )?;
                    tx.execute(
                        "UPDATE queue_webhooks SET failing_since = NULL, given_up = 0 WHERE id = ?1",
                        params![wid],
                    )?;
                }
                Outcome::Failed { status, error } => {
                    let error = logged(error);
                    if made >= MAX_ATTEMPTS {
                        tx.execute(
                            "UPDATE webhook_deliveries SET state = 'failed', attempts = ?2,
                                next_at = NULL, last_at = ?3, last_status = ?4, last_error = ?5
                             WHERE id = ?1",
                            params![did, made, now, status, error],
                        )?;
                        tx.execute(
                            "UPDATE queue_webhooks SET failing_since = COALESCE(failing_since, ?2),
                                last_error = ?3, given_up = given_up + 1 WHERE id = ?1",
                            params![wid, now, error],
                        )?;
                        let disable = tx.execute(
                            "UPDATE queue_webhooks SET disabled_at = ?2
                             WHERE id = ?1 AND disabled_at IS NULL AND given_up >= ?3",
                            params![wid, now, DISABLE_AFTER],
                        )? > 0;
                        if disable {
                            tx.execute(
                                "UPDATE webhook_deliveries SET state = 'failed', next_at = NULL,
                                    last_error = ?2
                                 WHERE webhook_id = ?1 AND state = 'pending'",
                                params![
                                    wid,
                                    format!(
                                        "not sent: the endpoint was disabled after {DISABLE_AFTER} deliveries in a row were given up"
                                    )
                                ],
                            )?;
                        }
                    } else {
                        let next = clax_core::time::stamp(
                            now_utc
                                + chrono::Duration::from_std(backoff(made))
                                    .expect("backoff fits a chrono duration"),
                        );
                        tx.execute(
                            "UPDATE webhook_deliveries SET attempts = ?2, next_at = ?3,
                                last_at = ?4, last_status = ?5, last_error = ?6 WHERE id = ?1",
                            params![did, made, next, now, status, error],
                        )?;
                        // The endpoint's other pending deliveries wait as
                        // long: an endpoint that is down costs a pass one
                        // attempt per backoff, not one per delivery queued.
                        tx.execute(
                            "UPDATE webhook_deliveries SET next_at = ?3
                             WHERE webhook_id = ?1 AND id != ?2 AND state = 'pending'
                               AND next_at < ?3",
                            params![wid, did, next],
                        )?;
                    }
                }
            }
            Ok(())
        })
    }

    /// When the earliest pending delivery [`Store::take_due_webhooks`]
    /// would take is due (a taken one counts at [`IN_FLIGHT_SECS`] on), or
    /// `None` with none pending.
    ///
    /// # Errors
    /// Database errors.
    pub fn webhooks_next_due(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let at: Option<String> = self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT MIN(d.next_at) FROM webhook_deliveries d
                 JOIN queue_webhooks w ON w.id = d.webhook_id
                 WHERE d.state = 'pending' AND {SENDABLE}"
            ))?
            .query_row([], |r| r.get(0))?)
        })?;
        Ok(at.and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|t| t.with_timezone(&chrono::Utc))
        }))
    }

    /// Removes queue `qid`'s endpoints and their deliveries, in `tx` (the
    /// queue's deletion).
    pub(crate) fn delete_webhooks_in(tx: &Transaction<'_>, qid: &str) -> Result<()> {
        tx.execute(
            "DELETE FROM webhook_deliveries WHERE webhook_id IN
               (SELECT id FROM queue_webhooks WHERE queue_id = ?1)",
            params![qid],
        )?;
        tx.execute(
            "DELETE FROM queue_webhooks WHERE queue_id = ?1",
            params![qid],
        )?;
        Ok(())
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::queues::{ExternalScope, NewQueue, NewReport, ReportWriter, ReporterToken, Verdict};
    use crate::test_util::{DAEMON, anchor, session, store_with_clock};
    use clax_core::queue::ReportersSee;
    use clax_core::report::{NewFix, ReporterProfile};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    const ORIGIN: &str = "https://app.example.com";

    fn clocked() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::at("2026-11-24T12:00:00Z"));
        let (d, st) = store_with_clock(clock.clone());
        (d, st, clock)
    }

    fn queue(st: &Store) -> String {
        st.create_queue(
            DAEMON,
            None,
            NewQueue {
                name: "App reports".into(),
                origins: vec![ORIGIN.into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id
    }

    /// A new endpoint of `qid` for `events`, unverified.
    fn unverified(st: &Store, qid: &str, events: &[&str]) -> (Webhook, Secret) {
        let events: Vec<String> = events.iter().map(|e| e.to_string()).collect();
        st.create_webhook(
            DAEMON,
            qid,
            None,
            "https://hooks.example.com/clax",
            &events,
            false,
        )
        .unwrap()
    }

    /// The pending verification of endpoint `wid`, as taken: its ID and
    /// attempts made.
    fn verification(st: &Store, wid: &str) -> Option<(String, u32)> {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT id, attempts FROM webhook_deliveries
                 WHERE webhook_id = ?1 AND event = 'webhook.verify' AND state = 'pending'",
                [wid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
        .unwrap()
    }

    /// Records the pending verification of `wid` answered rightly.
    fn answer(st: &Store, wid: &str) {
        let (id, attempts) = verification(st, wid).expect("a pending verification");
        st.record_webhook_outcome(&id, attempts, &Outcome::Delivered { status: 200 })
            .unwrap();
    }

    /// A new endpoint of `qid` for `events`, verified.
    fn hook(st: &Store, qid: &str, events: &[&str]) -> (Webhook, Secret) {
        let (h, secret) = unverified(st, qid, events);
        answer(st, &h.id);
        let h = st
            .webhooks(qid)
            .unwrap()
            .into_iter()
            .find(|x| x.id == h.id)
            .unwrap();
        assert_eq!(h.state, "verified");
        (h, secret)
    }

    /// Endpoint `wid`'s log of report deliveries, latest first.
    fn reports_log(st: &Store, qid: &str, wid: &str) -> Vec<Delivery> {
        st.webhook_deliveries(qid, wid, 100)
            .unwrap()
            .into_iter()
            .filter(|d| d.event != VERIFY_EVENT)
            .collect()
    }

    fn state_of(st: &Store, qid: &str, wid: &str) -> Webhook {
        st.webhooks(qid)
            .unwrap()
            .into_iter()
            .find(|x| x.id == wid)
            .unwrap()
    }

    fn file(st: &Store, qid: &str) -> String {
        let tok = ReporterToken {
            queue_id: qid.into(),
            service_grant_id: "g1".into(),
            reporter_id: "emp-1".into(),
            origin: ORIGIN.into(),
            public_key: String::new(),
            expires_at: "2026-11-24T12:15:00.000Z".into(),
            profile: Some(ReporterProfile {
                name: Some("Ana".into()),
                email: Some("ana@example.com".into()),
                github: Some("ana".into()),
                avatar_url: None,
            }),
        };
        st.file_report(
            DAEMON,
            qid,
            Some(&tok),
            ExternalScope::Instance,
            NewReport {
                external_id: Some("invoice:4821".into()),
                url: format!("{ORIGIN}/invoices/4821"),
                title: "Invoice 4821".into(),
                anchor: anchor(),
                body: "The total is wrong".into(),
                author_name: "Ana".into(),
                metadata: serde_json::Value::Null,
                revision: None,
                clip: None,
                context: None,
                server: None,
                diagnostics: None,
            },
        )
        .unwrap()
        .id
    }

    fn person() -> ReportWriter<'static> {
        ReportWriter::Person {
            name: "Mia".into(),
            public_id: Some("u_mia".into()),
        }
    }

    fn fix() -> NewFix {
        NewFix {
            summary: "Totals now include tax".into(),
            url: Some("https://github.com/acme/app/pull/7".into()),
            commit: None,
            release: None,
            verify_url: Some("https://app-pr-7.example.com/invoices/4821".into()),
            verify_after: None,
        }
    }

    fn payloads(st: &Store, wid: &str) -> Vec<Payload> {
        st.with_read(|c| {
            let rows: Vec<String> = c
                .prepare(
                    "SELECT payload_json FROM webhook_deliveries WHERE webhook_id = ?1
                       AND event != 'webhook.verify' ORDER BY created_at, id",
                )?
                .query_map([wid], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows
                .iter()
                .map(|p| serde_json::from_str(p).unwrap())
                .collect())
        })
        .unwrap()
    }

    fn delivered(st: &Store) {
        for d in st.take_due_webhooks(100).unwrap() {
            st.record_webhook_outcome(&d.id, d.attempts, &Outcome::Delivered { status: 200 })
                .unwrap();
        }
    }

    /// Each report change is queued, as the endpoint chose, in the
    /// change's transaction, with what a relay needs: the report, its
    /// reporter's profile, the fix and the reply.
    #[test]
    fn each_change_queues_its_event_with_the_report_and_its_reporter() {
        let (_d, st, _clock) = clocked();
        let qid = queue(&st);
        let (all, secret) = hook(&st, &qid, &[]);
        assert!(secret.expose().starts_with(SECRET_PREFIX));
        let (fixes, _) = hook(&st, &qid, &["report.fix_proposed"]);
        let tid = file(&st, &qid);
        let sid = session(&st, "claude", "a");
        st.claim(DAEMON, &qid, &tid, &sid).unwrap();
        // A renewal by the holder is no new claim.
        st.claim(DAEMON, &qid, &tid, &sid).unwrap();
        st.release(DAEMON, &qid, &tid, &sid).unwrap();
        st.report_reply(DAEMON, &qid, &tid, &person(), "Looking at it")
            .unwrap();
        st.propose_fix(DAEMON, &qid, &tid, &person(), fix())
            .unwrap();
        st.reopen_report(
            DAEMON,
            &qid,
            &tid,
            Verdict::Reporter("emp-1"),
            "Still wrong".into(),
            None,
            None,
            None,
        )
        .unwrap();
        st.propose_fix(DAEMON, &qid, &tid, &person(), fix())
            .unwrap();
        st.verify_report(DAEMON, &qid, &tid, Verdict::Reporter("emp-1"), None, None)
            .unwrap();
        let got = payloads(&st, &all.id);
        let kinds: Vec<&str> = got.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "report.created",
                "report.claimed",
                "report.released",
                "report.replied",
                "report.fix_proposed",
                "report.reopened",
                "report.fix_proposed",
                "report.verified",
            ]
        );
        let ids: std::collections::BTreeSet<&str> = got.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), got.len(), "each event has its own ID");
        let created = &got[0];
        assert_eq!(created.queue.name, "App reports");
        assert_eq!(created.report.id, tid);
        assert_eq!(created.report.external_id, "invoice:4821");
        assert_eq!(created.report.reporter_id.as_deref(), Some("emp-1"));
        let who = created.report.reporter.as_ref().unwrap();
        assert_eq!(who.email.as_deref(), Some("ana@example.com"));
        assert_eq!(who.github.as_deref(), Some("ana"));
        assert_eq!(got[3].comment.as_ref().unwrap().body, "Looking at it");
        let f = got[4].fix.as_ref().unwrap();
        assert_eq!(f.summary, "Totals now include tax");
        assert_eq!(f.url.as_deref(), Some("https://github.com/acme/app/pull/7"));
        assert_eq!(
            f.verify_url.as_deref(),
            Some("https://app-pr-7.example.com/invoices/4821")
        );
        assert_eq!(got[5].comment.as_ref().unwrap().body, "Still wrong");
        assert_eq!(got[7].report.state, clax_core::queue::State::Verified);

        let only: Vec<String> = payloads(&st, &fixes.id)
            .into_iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(only, ["report.fix_proposed", "report.fix_proposed"]);
        // One event, one ID, in every endpoint's delivery.
        assert_eq!(payloads(&st, &fixes.id)[0].id, got[4].id);

        // Resolving a closed report changes nothing and tells nothing.
        st.resolve_report(
            DAEMON,
            &qid,
            &tid,
            &person(),
            clax_core::report::ResolveReason::Other,
        )
        .unwrap();
        assert_eq!(payloads(&st, &all.id).len(), 8);
    }

    /// A change refused leaves no event behind: the delivery is written in
    /// the change's transaction or not at all.
    #[test]
    fn a_refused_change_queues_nothing() {
        let (_d, st, _clock) = clocked();
        let qid = queue(&st);
        let (h, _) = hook(&st, &qid, &[]);
        let tid = file(&st, &qid);
        let sid = session(&st, "claude", "a");
        let sess = st.get_session(&sid).unwrap().unwrap();
        // An agent replies without a lease: refused.
        assert!(
            st.report_reply(DAEMON, &qid, &tid, &ReportWriter::Agent(&sess), "hi")
                .is_err()
        );
        st.resolve_report(
            DAEMON,
            &qid,
            &tid,
            &person(),
            clax_core::report::ResolveReason::Other,
        )
        .unwrap();
        // A fix on a resolved report: refused after its comment was written
        // in the same transaction, which rolls back with the event.
        assert!(matches!(
            st.propose_fix(DAEMON, &qid, &tid, &person(), fix()),
            Err(CoreError::NotClaimable)
        ));
        let kinds: Vec<String> = payloads(&st, &h.id).into_iter().map(|p| p.kind).collect();
        assert_eq!(kinds, ["report.created", "report.resolved"]);
    }

    /// A failed attempt waits its backoff on the store's clock; past
    /// MAX_ATTEMPTS it is given up and its endpoint marked failing; a
    /// redelivery sends it again under the same event ID and a delivery
    /// clears the mark.
    #[test]
    fn retries_back_off_give_up_and_redeliver() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let (h, _) = hook(&st, &qid, &["report.created"]);
        file(&st, &qid);
        let fail = Outcome::Failed {
            status: Some(500),
            error: "upstream down".into(),
        };
        let mut event_id = None;
        for n in 1..MAX_ATTEMPTS {
            let due = st.take_due_webhooks(10).unwrap();
            assert_eq!(due.len(), 1, "attempt {n} is due");
            let d = &due[0];
            assert_eq!(d.attempts, n - 1);
            assert_eq!(
                event_id.get_or_insert_with(|| d.event_id.clone()),
                &d.event_id
            );
            st.record_webhook_outcome(&d.id, d.attempts, &fail).unwrap();
            let wait = backoff(n).as_secs() as i64;
            clock.advance(wait - 1);
            assert!(
                st.take_due_webhooks(10).unwrap().is_empty(),
                "attempt {} is not due before its backoff",
                n + 1
            );
            assert_eq!(
                st.webhooks_next_due().unwrap().unwrap(),
                st.now_utc() + chrono::Duration::seconds(1)
            );
            clock.advance(1);
        }
        let event_id = event_id.unwrap();
        let last = st.take_due_webhooks(10).unwrap().remove(0);
        assert_eq!(last.attempts, MAX_ATTEMPTS - 1);
        st.record_webhook_outcome(&last.id, last.attempts, &fail)
            .unwrap();
        let log = reports_log(&st, &qid, &h.id);
        assert_eq!(log[0].state, "failed");
        assert_eq!(log[0].attempts, MAX_ATTEMPTS);
        assert_eq!(log[0].last_status, Some(500));
        assert_eq!(log[0].last_error.as_deref(), Some("upstream down"));
        assert_eq!(st.webhooks_next_due().unwrap(), None);
        let marked = &st.webhooks(&qid).unwrap()[0];
        assert_eq!(marked.failing_since.as_deref(), Some(st.now().as_str()));
        assert_eq!(marked.last_error.as_deref(), Some("upstream down"));

        // An outcome recorded late, for an attempt already counted, is
        // dropped.
        st.record_webhook_outcome(&last.id, last.attempts, &Outcome::Delivered { status: 200 })
            .unwrap();
        assert_eq!(reports_log(&st, &qid, &h.id)[0].state, "failed");

        let again = st.redeliver_webhook(DAEMON, &qid, &h.id, &last.id).unwrap();
        assert_eq!((again.state.as_str(), again.attempts), ("pending", 0));
        let d = st.take_due_webhooks(10).unwrap().remove(0);
        assert_eq!(d.event_id, event_id, "a redelivery keeps the event's ID");
        st.record_webhook_outcome(&d.id, d.attempts, &Outcome::Delivered { status: 204 })
            .unwrap();
        let log = reports_log(&st, &qid, &h.id);
        assert_eq!(
            (log[0].state.as_str(), log[0].last_status),
            ("delivered", Some(204))
        );
        assert!(st.webhooks(&qid).unwrap()[0].failing_since.is_none());
    }

    /// An endpoint that is down holds back no other: a pass takes at most
    /// PER_ENDPOINT of any one endpoint's deliveries, and a failed attempt
    /// puts the endpoint's other pending deliveries behind its backoff.
    #[test]
    fn a_dead_endpoint_holds_back_no_other() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let (dead, _) = hook(&st, &qid, &[]);
        let (live, _) = hook(&st, &qid, &[]);
        let backlog = 2 * PER_ENDPOINT + 1;
        for _ in 0..backlog {
            file(&st, &qid);
        }
        let of =
            |due: &[DueDelivery], wid: &str| due.iter().filter(|d| d.webhook_id == wid).count();
        let due = st.take_due_webhooks(100).unwrap();
        assert_eq!(
            (of(&due, &dead.id), of(&due, &live.id)),
            (PER_ENDPOINT, PER_ENDPOINT)
        );
        for d in &due {
            let o = if d.webhook_id == dead.id {
                Outcome::Failed {
                    status: None,
                    error: "no answer".into(),
                }
            } else {
                Outcome::Delivered { status: 200 }
            };
            st.record_webhook_outcome(&d.id, d.attempts, &o).unwrap();
        }
        // The dead endpoint's backlog waits out its backoff; the live one's
        // goes on.
        let due = st.take_due_webhooks(100).unwrap();
        assert_eq!((of(&due, &dead.id), of(&due, &live.id)), (0, PER_ENDPOINT));
        for d in &due {
            st.record_webhook_outcome(&d.id, d.attempts, &Outcome::Delivered { status: 200 })
                .unwrap();
        }
        let due = st.take_due_webhooks(100).unwrap();
        assert_eq!((of(&due, &dead.id), of(&due, &live.id)), (0, 1));
        clock.advance(backoff(1).as_secs() as i64);
        let due = st.take_due_webhooks(100).unwrap();
        assert_eq!(of(&due, &dead.id), PER_ENDPOINT);
    }

    /// A delivery taken and never answered (the process ended mid-send) is
    /// due again IN_FLIGHT_SECS on, and no pass takes it meanwhile.
    #[test]
    fn a_taken_delivery_is_due_again_once_its_send_is_presumed_lost() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        hook(&st, &qid, &[]);
        file(&st, &qid);
        let taken = st.take_due_webhooks(10).unwrap();
        assert_eq!(taken.len(), 1);
        assert!(st.take_due_webhooks(10).unwrap().is_empty());
        clock.advance(IN_FLIGHT_SECS);
        let again = st.take_due_webhooks(10).unwrap();
        assert_eq!(again[0].id, taken[0].id);
        assert_eq!(again[0].attempts, 0);
    }

    /// The post-commit listener hears of a write that queued a delivery,
    /// and of none that queued nothing.
    #[test]
    fn the_listener_hears_of_queued_deliveries() {
        let (_d, st, _clock) = clocked();
        let heard = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = heard.clone();
        st.set_webhook_listener(move || {
            h.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        let qid = queue(&st);
        file(&st, &qid);
        assert_eq!(heard.load(std::sync::atomic::Ordering::Relaxed), 0);
        // Naming an endpoint queues its verification.
        let (h, _) = unverified(&st, &qid, &[]);
        assert_eq!(heard.load(std::sync::atomic::Ordering::Relaxed), 1);
        // An unverified endpoint is queued nothing.
        file(&st, &qid);
        assert_eq!(heard.load(std::sync::atomic::Ordering::Relaxed), 1);
        answer(&st, &h.id);
        file(&st, &qid);
        assert_eq!(heard.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn endpoints_are_checked_counted_and_removed_with_their_deliveries() {
        let (_d, st, _clock) = clocked();
        let qid = queue(&st);
        for (url, hosted) in [
            ("http://hooks.example.com/x", false),
            ("http://localhost:3000/x", true),
            ("https://169.254.169.254/latest", true),
        ] {
            let e = st
                .create_webhook(DAEMON, &qid, None, url, &[], hosted)
                .unwrap_err();
            assert!(
                matches!(
                    e,
                    CoreError::Invalid {
                        code: "invalid_url",
                        ..
                    }
                ),
                "{url}"
            );
        }
        st.create_webhook(DAEMON, &qid, None, "http://localhost:3000/x", &[], false)
            .unwrap();
        assert!(matches!(
            st.create_webhook(
                DAEMON,
                &qid,
                None,
                "https://h.example.com/",
                &["report.gone".into()],
                false
            ),
            Err(CoreError::Invalid {
                code: "invalid_events",
                ..
            })
        ));
        for _ in 1..MAX_ENDPOINTS {
            hook(&st, &qid, &[]);
        }
        assert!(matches!(
            st.create_webhook(DAEMON, &qid, None, "https://h.example.com/", &[], false),
            Err(CoreError::Invalid {
                code: "too_many_webhooks",
                ..
            })
        ));
        file(&st, &qid);
        let first = st.webhooks(&qid).unwrap().remove(0);
        st.delete_webhook(DAEMON, &qid, &first.id).unwrap();
        assert!(matches!(
            st.webhook_deliveries(&qid, &first.id, 10),
            Err(CoreError::NotFound)
        ));
        assert_eq!(st.take_due_webhooks(100).unwrap().len(), MAX_ENDPOINTS - 1);
        delivered(&st);
        st.delete_queue(DAEMON, &qid).unwrap();
        let left: i64 = st
            .with_read(|c| {
                Ok(c.query_row(
                    "SELECT (SELECT COUNT(*) FROM queue_webhooks) + (SELECT COUNT(*) FROM webhook_deliveries)",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(left, 0, "a deleted queue keeps no endpoint or delivery");
    }

    fn invalid_code(e: CoreError) -> &'static str {
        match e {
            CoreError::Invalid { code, .. } => code,
            e => panic!("{e:?}"),
        }
    }

    /// An endpoint named is unverified and queued no report event until
    /// it answers its one verification; what happened before is not sent
    /// once it verifies.
    #[test]
    fn an_unverified_endpoint_is_sent_only_its_verification() {
        let (_d, st, _clock) = clocked();
        let qid = queue(&st);
        let (h, _) = unverified(&st, &qid, &[]);
        assert_eq!(h.state, "unverified");
        assert_eq!(
            h.verify_by.as_deref(),
            Some(
                clax_core::time::stamp(
                    st.now_utc() + chrono::Duration::seconds(VERIFY_WINDOW_SECS)
                )
                .as_str()
            )
        );
        file(&st, &qid);
        let due = st.take_due_webhooks(100).unwrap();
        assert_eq!(due.len(), 1, "the verification alone");
        let d = &due[0];
        assert_eq!(d.event, VERIFY_EVENT);
        let p: VerifyPayload = serde_json::from_str(&d.payload).unwrap();
        assert_eq!(
            (p.webhook.id.as_str(), p.queue.name.as_str()),
            (h.id.as_str(), "App reports")
        );
        assert_eq!(p.challenge.len(), 43);
        st.record_webhook_outcome(&d.id, d.attempts, &Outcome::Delivered { status: 200 })
            .unwrap();
        let v = state_of(&st, &qid, &h.id);
        assert_eq!((v.state, v.verify_by.as_deref()), ("verified", None));
        assert_eq!(v.verified_at.as_deref(), Some(st.now().as_str()));
        assert!(payloads(&st, &h.id).is_empty(), "no event is back-filled");
        assert!(st.take_due_webhooks(100).unwrap().is_empty());
        file(&st, &qid);
        assert_eq!(payloads(&st, &h.id).len(), 1);
        let log = st.webhook_deliveries(&qid, &h.id, 10).unwrap();
        let ver = log.iter().find(|d| d.event == VERIFY_EVENT).unwrap();
        assert_eq!(
            (ver.state.as_str(), ver.report_id.as_deref()),
            ("delivered", None)
        );
    }

    /// A failed verification is not retried: it fails at once, says why on
    /// its endpoint, and another is the admin's to ask for, at most
    /// VERIFY_SENDS_PER_HOUR an hour (the first included) by the store's
    /// clock. A verification is never redelivered, and an unverified
    /// endpoint's deliveries are not either.
    #[test]
    fn a_verification_is_sent_once_and_verify_again_is_capped() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let (h, _) = unverified(&st, &qid, &[]);
        let d = st.take_due_webhooks(10).unwrap().remove(0);
        let fail = Outcome::Failed {
            status: Some(200),
            error: "the answer has no \"answer\" string".into(),
        };
        st.record_webhook_outcome(&d.id, d.attempts, &fail).unwrap();
        // Past the first retry's backoff, within the hour.
        clock.advance(backoff(2).as_secs() as i64);
        assert!(st.take_due_webhooks(10).unwrap().is_empty(), "not retried");
        assert_eq!(st.webhooks_next_due().unwrap(), None);
        let v = state_of(&st, &qid, &h.id);
        assert_eq!(v.state, "unverified");
        assert!(v.last_error.unwrap().starts_with("verification failed: "));
        assert_eq!(
            invalid_code(
                st.redeliver_webhook(DAEMON, &qid, &h.id, &d.id)
                    .unwrap_err()
            ),
            "not_verified"
        );

        for _ in 1..VERIFY_SENDS_PER_HOUR {
            st.verify_webhook(DAEMON, &qid, &h.id).unwrap();
            clock.advance(60);
        }
        assert_eq!(
            invalid_code(st.verify_webhook(DAEMON, &qid, &h.id).unwrap_err()),
            "limit_reached"
        );
        let sent = st.webhook_deliveries(&qid, &h.id, 100).unwrap();
        assert_eq!(sent.len(), VERIFY_SENDS_PER_HOUR);
        let challenges: std::collections::BTreeSet<String> = st
            .take_due_webhooks(100)
            .unwrap()
            .iter()
            .map(|d| {
                serde_json::from_str::<VerifyPayload>(&d.payload)
                    .unwrap()
                    .challenge
            })
            .collect();
        assert_eq!(
            challenges.len(),
            VERIFY_SENDS_PER_HOUR - 1,
            "each a new challenge"
        );
        // An hour after the first, one more may go.
        clock.advance(3600 - backoff(2).as_secs() as i64 - 60 * (VERIFY_SENDS_PER_HOUR as i64 - 1));
        st.verify_webhook(DAEMON, &qid, &h.id).unwrap();
        answer(&st, &h.id);
        assert_eq!(
            invalid_code(st.verify_webhook(DAEMON, &qid, &h.id).unwrap_err()),
            "already_verified"
        );
    }

    /// An endpoint not verified within VERIFY_WINDOW_SECS of being named is
    /// inert: Verify again is refused and a late right answer verifies
    /// nothing.
    #[test]
    fn an_endpoint_never_verified_in_its_window_is_inert() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let (h, _) = unverified(&st, &qid, &[]);
        let (id, attempts) = verification(&st, &h.id).unwrap();
        clock.advance(VERIFY_WINDOW_SECS);
        assert_eq!(state_of(&st, &qid, &h.id).state, "unverified");
        clock.advance(1);
        assert_eq!(state_of(&st, &qid, &h.id).state, "expired");
        assert_eq!(
            invalid_code(st.verify_webhook(DAEMON, &qid, &h.id).unwrap_err()),
            "verification_expired"
        );
        st.record_webhook_outcome(&id, attempts, &Outcome::Delivered { status: 200 })
            .unwrap();
        assert_eq!(state_of(&st, &qid, &h.id).state, "expired");
        file(&st, &qid);
        assert!(payloads(&st, &h.id).is_empty());
    }

    /// DISABLE_AFTER deliveries given up in a row disable an endpoint: its
    /// pending deliveries are given up, it is queued nothing more, and
    /// re-enabling it verifies it again before any event is sent. A
    /// delivery that gets through in between starts the count again.
    #[test]
    fn deliveries_given_up_in_a_row_disable_and_reenabling_verifies_again() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let (h, _) = hook(&st, &qid, &["report.created"]);
        let fail = Outcome::Failed {
            status: Some(500),
            error: "down".into(),
        };
        // Gives up every delivery due, attempt after attempt.
        let give_up = |n: usize| {
            for _ in 0..MAX_ATTEMPTS {
                let due = st.take_due_webhooks(100).unwrap();
                assert_eq!(due.len(), n);
                for d in &due {
                    st.record_webhook_outcome(&d.id, d.attempts, &fail).unwrap();
                }
                clock.advance(6 * 3600);
            }
        };
        for _ in 1..DISABLE_AFTER {
            file(&st, &qid);
        }
        give_up(DISABLE_AFTER as usize - 1);
        assert_eq!(state_of(&st, &qid, &h.id).given_up, DISABLE_AFTER - 1);
        // One delivered starts the count again.
        file(&st, &qid);
        delivered(&st);
        let v = state_of(&st, &qid, &h.id);
        assert_eq!((v.state, v.given_up), ("verified", 0));

        for _ in 1..DISABLE_AFTER {
            file(&st, &qid);
        }
        give_up(DISABLE_AFTER as usize - 1);
        // The last one, with another filed an attempt behind it.
        file(&st, &qid);
        for round in 0..MAX_ATTEMPTS {
            for d in st.take_due_webhooks(100).unwrap() {
                st.record_webhook_outcome(&d.id, d.attempts, &fail).unwrap();
            }
            if state_of(&st, &qid, &h.id).state == "disabled" {
                assert_eq!(round, MAX_ATTEMPTS - 1);
                break;
            }
            if round == 0 {
                file(&st, &qid);
            }
            clock.advance(6 * 3600);
        }
        let v = state_of(&st, &qid, &h.id);
        assert_eq!(v.state, "disabled");
        assert_eq!(v.disabled_at.as_deref(), Some(st.now().as_str()));
        assert!(
            reports_log(&st, &qid, &h.id)
                .iter()
                .all(|d| d.state != "pending"),
            "its pending deliveries are given up"
        );
        assert!(
            reports_log(&st, &qid, &h.id)[0]
                .last_error
                .as_deref()
                .unwrap()
                .contains("disabled")
        );
        assert_eq!(st.webhooks_next_due().unwrap(), None);
        let before = payloads(&st, &h.id).len();
        file(&st, &qid);
        assert_eq!(
            payloads(&st, &h.id).len(),
            before,
            "a disabled endpoint is queued nothing"
        );
        let did = reports_log(&st, &qid, &h.id)[0].id.clone();
        assert_eq!(
            invalid_code(st.redeliver_webhook(DAEMON, &qid, &h.id, &did).unwrap_err()),
            "not_verified"
        );

        let again = st.verify_webhook(DAEMON, &qid, &h.id).unwrap();
        assert_eq!(
            (
                again.state,
                again.given_up,
                again.disabled_at.as_deref(),
                again.failing_since.as_deref()
            ),
            ("unverified", 0, None, None)
        );
        file(&st, &qid);
        assert_eq!(
            payloads(&st, &h.id).len(),
            before,
            "none until it verifies again"
        );
        answer(&st, &h.id);
        file(&st, &qid);
        assert_eq!(payloads(&st, &h.id).len(), before + 1);
    }

    /// Past MAX_PER_HOUR events in its hour an endpoint is queued none:
    /// each is dropped for it and counted, and the next hour by the
    /// store's clock it is queued again.
    #[test]
    fn events_past_the_hourly_cap_are_dropped_and_counted() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let (capped, _) = hook(&st, &qid, &[]);
        let (other, _) = hook(&st, &qid, &[]);
        file(&st, &qid);
        // As if MAX_PER_HOUR - 1 more had been queued this hour.
        st.with_tx(|tx| {
            tx.execute(
                "UPDATE queue_webhooks SET window_count = ?2 WHERE id = ?1",
                params![capped.id, MAX_PER_HOUR - 1],
            )?;
            Ok(())
        })
        .unwrap();
        clock.advance(60);
        file(&st, &qid);
        assert_eq!(payloads(&st, &capped.id).len(), 2, "the hour's last");
        clock.advance(60);
        file(&st, &qid);
        file(&st, &qid);
        assert_eq!(payloads(&st, &capped.id).len(), 2);
        assert_eq!(
            payloads(&st, &other.id).len(),
            4,
            "another endpoint is not held back"
        );
        let v = state_of(&st, &qid, &capped.id);
        assert_eq!(
            (v.dropped, v.last_dropped_at.as_deref()),
            (2, Some(st.now().as_str()))
        );
        // The window started with the first event, 120 s ago.
        clock.advance(3600 - 120);
        file(&st, &qid);
        assert_eq!(payloads(&st, &capped.id).len(), 3);
        assert_eq!(state_of(&st, &qid, &capped.id).dropped, 2);
    }

    fn queue_named(st: &Store, name: &str) -> String {
        st.create_queue(
            DAEMON,
            None,
            NewQueue {
                name: name.into(),
                origins: vec![ORIGIN.into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id
    }

    fn add(st: &Store, qid: &str, url: &str) -> Result<(Webhook, Secret)> {
        st.create_webhook(DAEMON, qid, None, url, &[], false)
    }

    /// Removing an endpoint and adding it again sends no more
    /// verifications: past VERIFY_SENDS_PER_QUEUE_HOUR in an hour, removed
    /// endpoints' included, adding one or Verify again is refused and
    /// leaves nothing behind; an hour on by the store's clock it is taken.
    #[test]
    fn removing_and_adding_again_hits_the_queue_s_hourly_verification_cap() {
        let (_d, st, clock) = clocked();
        let qid = queue(&st);
        let url = "https://hooks.example.com/clax";
        let (kept, _) = add(&st, &qid, url).unwrap();
        for _ in 1..VERIFY_SENDS_PER_QUEUE_HOUR {
            let (h, _) = add(&st, &qid, url).unwrap();
            st.delete_webhook(DAEMON, &qid, &h.id).unwrap();
        }
        let sent_before = st.take_due_webhooks(1000).unwrap().len();
        assert_eq!(
            sent_before, 1,
            "only the kept endpoint's verification is left"
        );
        assert_eq!(
            invalid_code(add(&st, &qid, url).unwrap_err()),
            "verifications_limited"
        );
        assert_eq!(
            invalid_code(st.verify_webhook(DAEMON, &qid, &kept.id).unwrap_err()),
            "verifications_limited"
        );
        let left = st.webhooks(&qid).unwrap();
        assert_eq!(left.len(), 1, "a refused add leaves no endpoint");
        assert_eq!(left[0].id, kept.id);
        assert_eq!(
            st.webhook_deliveries(&qid, &kept.id, 100).unwrap().len(),
            1,
            "a refused Verify again queues nothing"
        );
        clock.advance(3600 - 1);
        assert!(add(&st, &qid, url).is_err(), "not before the hour is up");
        clock.advance(1);
        add(&st, &qid, url).unwrap();
        st.verify_webhook(DAEMON, &qid, &kept.id).unwrap();
    }

    /// Verifications to one host are capped across every queue: a person
    /// with many queues sends a host no more than
    /// VERIFY_SENDS_PER_HOST_HOUR an hour; another host is not held back.
    #[test]
    fn a_host_s_verifications_are_capped_across_queues() {
        let (_d, st, clock) = clocked();
        let url = "https://hooks.example.com/a";
        let queues: Vec<String> =
            (0..VERIFY_SENDS_PER_HOST_HOUR.div_ceil(VERIFY_SENDS_PER_QUEUE_HOUR) + 1)
                .map(|n| queue_named(&st, &format!("Q{n}")))
                .collect();
        let mut sent = 0;
        'fill: for qid in &queues {
            for _ in 0..VERIFY_SENDS_PER_QUEUE_HOUR {
                if sent == VERIFY_SENDS_PER_HOST_HOUR {
                    break 'fill;
                }
                let (h, _) = add(&st, qid, url).unwrap();
                st.delete_webhook(DAEMON, qid, &h.id).unwrap();
                sent += 1;
            }
        }
        let fresh = queues.last().unwrap();
        assert_eq!(
            invalid_code(add(&st, fresh, "https://HOOKS.example.com/other").unwrap_err()),
            "verifications_limited"
        );
        assert!(st.webhooks(fresh).unwrap().is_empty());
        add(&st, fresh, "https://elsewhere.example.com/hook").unwrap();
        clock.advance(3600);
        add(&st, fresh, url).unwrap();
    }
}
