//! Fixes checked on preview deployments: fix tickets and the reports a
//! preview's origin holds a fix for.
//!
//! A fix proposed with `preview` ([`Store::propose_fix_ticketed`]) is
//! issued a ticket: a [`FIX_TICKET_PREFIX`] token, kept as its SHA-256,
//! bound to that fix of that report and to the queue's origin patterns as
//! they were when it was issued. Its whole right is to set that fix's
//! `verify_url` to a preview deployment's origin, one of those patterns
//! (and the queue, still) allows ([`Store::attach_preview`]). It reads
//! nothing, mints nothing and gives no verdict. It ends after
//! [`FIX_TICKET_SECS`], or as soon as its fix is no longer waiting for a
//! verdict (verified, rejected, superseded or closed), whichever is first.
//!
//! A ticket's token reads `cxf_<queue ID>.<report ID>.<fix number>.<secret>`:
//! the IDs name the route it is presented on, so a deployment holds one
//! value, and the secret alone is what authenticates it.

use super::super::threads::{AUTHOR_AGENT, AUTHOR_VIEWER, NewComment};
use super::{
    QueueRow, ReportWriter, Visibility, later, queue_in, queue_record, report_artifact_in,
};
use crate::Store;
use clax_core::audit::{AuditCtx, AuditKind};
use clax_core::origin::{OriginPattern, normalize_origin};
use clax_core::queue::{FIX_TICKET_PREFIX, FIX_TICKET_SECS, Report, State, queue_allows_origin};
use clax_core::report::FixOutcome;
use clax_core::secret::Secret;
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, Transaction, params};

/// The patterns among a queue's `origins` (each holding a `*`): what a
/// ticket may attach an origin from.
pub(super) fn patterns_of(origins: &[String]) -> Vec<String> {
    origins
        .iter()
        .filter(|o| OriginPattern::parse(o).is_ok_and(|p| p.is_pattern()))
        .cloned()
        .collect()
}

/// 400 `no_preview_origins`: a ticket is for a preview, whose origin one
/// of the queue's patterns matches.
pub(super) fn no_preview_origins() -> CoreError {
    CoreError::invalid(
        "no_preview_origins",
        "the queue lists no origin pattern (such as https://app-pr-*.example.com): \
         a fix ticket attaches a preview deployment matched by one",
    )
}

/// Who a ticket is issued by: an agent's session, named as its agent is
/// (else its harness), for its person; or a person themself.
pub(super) struct Issuer {
    name: String,
    session_id: Option<String>,
    /// The person's public ID; `None` for the owner.
    person: Option<String>,
}

impl Issuer {
    pub(super) fn of(by: &ReportWriter<'_>) -> Issuer {
        match by {
            ReportWriter::Agent(s) => Issuer {
                name: s.name.clone().unwrap_or_else(|| s.harness.clone()),
                session_id: Some(s.id.clone()),
                person: s.person.clone(),
            },
            ReportWriter::Person { name, public_id } => Issuer {
                name: name.clone(),
                session_id: None,
                person: public_id.clone(),
            },
        }
    }
}

/// A ticket just issued ([`Store::propose_fix_ticketed`]): its token,
/// shown in this answer alone, and what it is bound to. Its `Debug` shows
/// no token.
#[derive(Clone)]
pub struct IssuedTicket {
    pub id: String,
    /// `cxf_<queue ID>.<report ID>.<n>.<secret>`.
    pub token: Secret,
    /// The fix's number.
    pub n: u32,
    /// The queue's origin patterns it may attach an origin from.
    pub origins: Vec<String>,
    pub expires_at: String,
}

impl std::fmt::Debug for IssuedTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedTicket")
            .field("id", &self.id)
            .field("n", &self.n)
            .field("origins", &self.origins)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// A live fix ticket as stored ([`Store::fix_ticket_by_token`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixTicket {
    pub id: String,
    pub queue_id: String,
    /// The report's thread.
    pub thread_id: String,
    /// The fix's number.
    pub n: u32,
    /// The queue's origin patterns when it was issued.
    pub origins: Vec<String>,
    /// Who it was issued by: the agent's name, or the person's.
    pub issuer_name: String,
    pub issuer_session_id: Option<String>,
    /// The person it was issued for; `None` for the owner.
    pub issuer_person: Option<String>,
    pub expires_at: String,
}

/// What [`Store::attach_preview`] did: the report after, and the notice
/// comment it posted (`None` when the fix already named that URL, and
/// nothing changed).
#[derive(Clone, Debug)]
pub struct Attached {
    pub report: Report,
    pub url: String,
    pub comment_id: Option<String>,
}

/// The body of the notice an attach posts on the report.
fn notice(n: u32, url: &str) -> String {
    format!("A preview with fix {n} is ready: check it at {url}")
}

/// The longest path an attach takes, in bytes.
const MAX_PREVIEW_PATH_BYTES: usize = 1024;

impl Store {
    /// Issues the ticket of fix `n` of report `tid` of queue `q`, in `tx`,
    /// by `issuer`; tickets that ended a day before are dropped.
    pub(super) fn issue_ticket_in(
        &self,
        tx: &Transaction<'_>,
        q: &QueueRow,
        tid: &str,
        n: u32,
        issuer: &Issuer,
    ) -> Result<IssuedTicket> {
        let origins = patterns_of(&q.origins);
        if origins.is_empty() {
            return Err(no_preview_origins());
        }
        let now = self.now_utc();
        let expires_at = later(now, FIX_TICKET_SECS);
        let id = new_ulid();
        let token = clax_core::secret::token(&format!("{FIX_TICKET_PREFIX}{}.{tid}.{n}.", q.id));
        tx.execute(
            "DELETE FROM queue_fix_tickets WHERE expires_at <= ?1",
            params![later(now, -86_400)],
        )?;
        tx.execute(
            "INSERT INTO queue_fix_tickets (id, token_sha256, queue_id, thread_id, n,
                origins_json, issuer_name, issuer_session_id, issuer_person, created_at,
                expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                token.sha256_hex(),
                q.id,
                tid,
                n,
                serde_json::to_string(&origins).expect("strings serialise"),
                issuer.name,
                issuer.session_id,
                issuer.person,
                clax_core::time::stamp(now),
                expires_at
            ],
        )?;
        Ok(IssuedTicket {
            id,
            token,
            n,
            origins,
            expires_at,
        })
    }

    /// The fix ticket whose token's SHA-256 (lowercase hex) is `sha256`,
    /// when its queue is live; `None` for no such ticket.
    ///
    /// # Errors
    /// [`CoreError::TokenExpired`] for a ticket past its expiry, or whose
    /// fix is no longer waiting for a verdict (or whose report is gone).
    pub fn fix_ticket_by_token(&self, sha256: &str) -> Result<Option<FixTicket>> {
        let now = self.now();
        let found = self.with_read(|c| {
            let row = c
                .prepare_cached(
                    "SELECT k.id, k.queue_id, k.thread_id, k.n, k.origins_json, k.issuer_name,
                        k.issuer_session_id, k.issuer_person, k.expires_at,
                        (SELECT f.outcome FROM report_fixes f
                          WHERE f.thread_id = k.thread_id AND f.n = k.n)
                     FROM queue_fix_tickets k JOIN queues q ON q.id = k.queue_id
                     WHERE k.token_sha256 = ?1 AND q.deleted_at IS NULL",
                )?
                .query_row(params![sha256], |r| {
                    let origins: String = r.get(4)?;
                    Ok((
                        FixTicket {
                            id: r.get(0)?,
                            queue_id: r.get(1)?,
                            thread_id: r.get(2)?,
                            n: r.get(3)?,
                            origins: serde_json::from_str(&origins).unwrap_or_default(),
                            issuer_name: r.get(5)?,
                            issuer_session_id: r.get(6)?,
                            issuer_person: r.get(7)?,
                            expires_at: r.get(8)?,
                        },
                        r.get::<_, Option<String>>(9)?,
                    ))
                })
                .optional()?;
            Ok(row)
        })?;
        match found {
            None => Ok(None),
            Some((t, outcome)) => {
                let pending = outcome.as_deref() == Some(FixOutcome::Pending.as_str());
                if t.expires_at <= now || !pending {
                    Err(CoreError::TokenExpired)
                } else {
                    Ok(Some(t))
                }
            }
        }
    }

    /// `ticket` attaches the preview deployment at exact `origin` to its
    /// fix: the fix's `verify_url` becomes `origin` and `path` (`/` when
    /// none), replacing what it named, and a notice is posted on the
    /// report as the ticket's issuer (the agent's comment through its
    /// session, or the person's), which the reporter reads as it reads a
    /// fix's summary. Recorded as `queue.preview {queue_id, thread_id, n,
    /// origin, url, ticket_id, issued_by, for_person}` (`for_person` the
    /// person's public ID, or `owner`), after the notice's `comment.add`.
    /// Attaching the URL the fix names already changes nothing and
    /// records nothing.
    ///
    /// # Errors
    /// `invalid_origin` for an `origin` that is not an exact origin;
    /// [`CoreError::WrongOrigin`] for one none of the ticket's patterns
    /// allows, or the queue no longer allows; `invalid_args` for a `path`
    /// that does not start with a single `/`, is over 1 KiB, or would move
    /// the URL off `origin`; `invalid_fix` for a URL a fix does not take; [`CoreError::TokenExpired`] when the ticket
    /// ended since it was read.
    pub fn attach_preview(
        &self,
        ctx: &AuditCtx,
        ticket: &FixTicket,
        origin: &str,
        path: Option<&str>,
    ) -> Result<Attached> {
        let origin = normalize_origin(origin)?;
        let path = path.unwrap_or("/");
        let bad_path = |m: &str| CoreError::invalid("invalid_args", m.to_string());
        if !path.starts_with('/') || path.starts_with("//") || path.starts_with("/\\") {
            return Err(bad_path("a preview's path starts with a single /"));
        }
        if path.len() > MAX_PREVIEW_PATH_BYTES {
            return Err(bad_path("a preview's path is at most 1024 bytes"));
        }
        let url = format!("{origin}{path}");
        if clax_core::origin::origin_of_url(&url).as_deref() != Some(origin.as_str()) {
            return Err(bad_path("a preview's path keeps the URL on its origin"));
        }
        if !queue_allows_origin(&ticket.origins, &origin) {
            return Err(CoreError::WrongOrigin);
        }
        let (qid, tid, n) = (
            ticket.queue_id.as_str(),
            ticket.thread_id.as_str(),
            ticket.n,
        );
        let now = self.now();
        // The fix as it is: a URL it names already is no change.
        let current: Option<String> = self.with_read(|c| {
            let fix: Option<String> = c
                .query_row(
                    "SELECT fix_json FROM report_fixes WHERE thread_id = ?1 AND n = ?2",
                    params![tid, n],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(fix
                .and_then(|j| serde_json::from_str::<clax_core::report::NewFix>(&j).ok())
                .and_then(|f| f.verify_url))
        })?;
        let unchanged = current.as_deref() == Some(url.as_str());
        let comment = match &ticket.issuer_session_id {
            Some(sid) => NewComment {
                author_kind: AUTHOR_AGENT,
                author_name: ticket.issuer_name.clone(),
                author_public_id: None,
                via_session_id: Some(sid.clone()),
                body: notice(n, &url),
                via_page: false,
            },
            None => NewComment {
                author_kind: AUTHOR_VIEWER,
                author_name: ticket.issuer_name.clone(),
                author_public_id: ticket.issuer_person.clone(),
                via_session_id: None,
                body: notice(n, &url),
                via_page: false,
            },
        };
        // Checked in the transaction that writes: the ticket still holds,
        // its fix still waits, and the queue still allows the origin.
        let check = |tx: &rusqlite::Connection| -> Result<(String, clax_core::report::NewFix)> {
            let q = queue_in(tx, qid)?.ok_or(CoreError::TokenExpired)?;
            if !queue_allows_origin(&q.origins, &origin) {
                return Err(CoreError::WrongOrigin);
            }
            let aid = report_artifact_in(tx, qid, tid).map_err(|_| CoreError::TokenExpired)?;
            let live: Option<(String, String)> = tx
                .query_row(
                    "SELECT f.outcome, f.fix_json FROM queue_fix_tickets k
                     JOIN report_fixes f ON f.thread_id = k.thread_id AND f.n = k.n
                     WHERE k.id = ?1 AND k.expires_at > ?2",
                    params![ticket.id, now],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match live {
                Some((outcome, fix)) if outcome == FixOutcome::Pending.as_str() => {
                    let fix = serde_json::from_str(&fix).map_err(|_| CoreError::Corrupt {
                        artifact_id: aid.clone(),
                        column: "report_fixes.fix_json",
                        version: None,
                    })?;
                    Ok((aid, fix))
                }
                _ => Err(CoreError::TokenExpired),
            }
        };
        if unchanged {
            self.with_read(|c| check(c).map(|_| ()))?;
            let report = self
                .report(qid, tid, Visibility::All)?
                .ok_or(CoreError::NotFound)?;
            return Ok(Attached {
                report,
                url,
                comment_id: None,
            });
        }
        // The URL as a fix takes one (an http(s) URL of bounded length).
        clax_core::report::NewFix {
            summary: "preview".into(),
            url: None,
            commit: None,
            release: None,
            verify_url: Some(url.clone()),
            verify_after: None,
        }
        .check()?;
        let for_person = ticket.issuer_person.as_deref().unwrap_or("owner");
        let made = self.add_comment_then(ctx, tid, comment, |tx, _, _| {
            let (aid, mut fix) = check(tx)?;
            fix.verify_url = Some(url.clone());
            tx.execute(
                "UPDATE report_fixes SET fix_json = ?3 WHERE thread_id = ?1 AND n = ?2",
                params![
                    tid,
                    n,
                    serde_json::to_string(&fix).expect("fixes serialise")
                ],
            )?;
            let mut rec = queue_record(AuditKind::QueuePreview, &now, qid)
                .with("thread_id", tid)
                .with("n", n)
                .with("origin", origin.as_str())
                .with("url", url.as_str())
                .with("ticket_id", ticket.id.as_str())
                .with("issued_by", ticket.issuer_name.as_str())
                .with("for_person", for_person);
            rec.ids.artifact = Some(aid);
            rec.ids.thread = Some(tid.to_string());
            self.record_audit(tx, ctx, rec)?;
            Ok(())
        })?;
        let report = self
            .report(qid, tid, Visibility::All)?
            .ok_or(CoreError::NotFound)?;
        Ok(Attached {
            report,
            url,
            comment_id: Some(made.id),
        })
    }

    /// The reports of queue `qid` that reporter `reporter_id` filed whose
    /// fix waits for its verdict at a `verify_url` on exact `origin` (the
    /// reporter token's, never one a request names), oldest first, as the
    /// reporter reads them: what a preview deployment's page offers its
    /// reporter to check there.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn preview_reports(
        &self,
        qid: &str,
        reporter_id: &str,
        origin: &str,
    ) -> Result<Vec<Report>> {
        let ids: Vec<String> = self.with_read(|c| {
            queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            Ok(c.prepare_cached(
                "SELECT t.id FROM threads t JOIN artifacts a ON a.id = t.artifact_id
                 WHERE a.deleted_at IS NULL AND t.queue_id = ?1 AND t.reporter_id = ?2
                   AND t.status = 'open' AND t.report_phase = 'fix_proposed'
                 ORDER BY t.created_at, t.id",
            )?
            .query_map(params![qid, reporter_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
        })?;
        let mut out = Vec::new();
        for tid in ids {
            let Some(r) = self.report(qid, &tid, Visibility::Reporter(reporter_id))? else {
                continue;
            };
            let here = r.state == State::FixProposed
                && r.fixes.last().is_some_and(|f| {
                    f.outcome == FixOutcome::Pending
                        && f.fix
                            .verify_url
                            .as_deref()
                            .and_then(clax_core::origin::origin_of_url)
                            .is_some_and(|o| o == origin)
                });
            if here {
                out.push(r);
            }
        }
        Ok(out)
    }
}
