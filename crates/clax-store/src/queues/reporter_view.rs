//! What a reporter reads of its queue, and its view of the queue in Clax.
//!
//! Every reporter-facing answer is a [`ReporterReport`] made by
//! [`project`]: the store's one place that decides what a reporter sees of
//! the queue's other reporters. Under `reporters_see: own` a reporter reads
//! only its own reports (the read's own filter, [`super::reporter_filter`])
//! and no `submitter`. Under `all` it reads every report; another
//! reporter's has no `reporter_id` and no `metadata` (an app's diagnostics,
//! which may carry its user's details), each reporter's comment carries the
//! name its app gave that reporter ([`ANOTHER_REPORTER`] when none), and
//! every report names its [`Submitter`], `you` on the reader's own. A
//! member's view of the queue is not this module's and is unchanged.
//!
//! A reporter's view in Clax: the reporter's app asks, with its reporter
//! token, for a view link ([`Store::mint_view_code`]): a single-use code
//! of [`VIEW_CODE_SECS`] for the token's queue and reporter. Clax trades the
//! code ([`Store::redeem_view_code`]) for a view session of
//! [`VIEW_SESSION_SECS`] that the browser keeps as a cookie
//! ([`Store::view_session`]). Codes and sessions are kept only as SHA-256;
//! revoking the grant or deleting the queue ends them.

use super::{QueueRow, Store, Visibility, later, queue_in, report_in};
use clax_core::queue::{
    ANOTHER_REPORTER, AUTHOR_REPORTER, Report, ReporterReport, ReportersSee, Submitter,
    VIEW_CODE_PREFIX, VIEW_CODE_SECS, VIEW_SESSION_PREFIX, VIEW_SESSION_SECS,
};
use clax_core::secret::Secret;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashMap;

/// A live view session ([`Store::view_session`]): reporter `reporter_id`
/// of queue `queue_id`, through service grant `service_grant_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewSession {
    pub queue_id: String,
    pub service_grant_id: String,
    pub reporter_id: String,
    pub expires_at: String,
}

/// Drops every view code and session whose `column` (`queue_id` or
/// `service_grant_id`) is `value`, in `tx`.
pub(super) fn forget_in(tx: &Transaction<'_>, column: &'static str, value: &str) -> Result<()> {
    debug_assert!(matches!(column, "queue_id" | "service_grant_id"));
    for table in ["queue_view_codes", "queue_view_sessions"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE {column} = ?1"),
            params![value],
        )?;
    }
    Ok(())
}

/// The names queue `qid`'s app gave its reporters, by reporter ID, on `c`.
fn names_in(c: &Connection, qid: &str) -> Result<HashMap<String, String>> {
    Ok(c
        .prepare_cached("SELECT reporter_id, name FROM queue_reporters WHERE queue_id = ?1")?
        .query_map(params![qid], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// `r` as reporter `viewer` of queue `q` may read it, `names` being the
/// queue's reporters' names; `None` for a report `viewer` may not read
/// (another reporter's, under `own`).
fn project(
    mut r: Report,
    viewer: &str,
    q: &QueueRow,
    names: &HashMap<String, String>,
) -> Option<ReporterReport> {
    // Never a members-only comment nor the reporter's profile (§5.4).
    r = r.for_reporter();
    let mine = r.reporter_id.as_deref() == Some(viewer);
    if !mine && q.reporters_see == ReportersSee::Own {
        return None;
    }
    let name_of = |rid: &str| names.get(rid).cloned();
    let submitter = (q.reporters_see == ReportersSee::All).then(|| Submitter {
        name: r.reporter_id.as_deref().and_then(name_of),
        you: mine,
    });
    for c in &mut r.comments {
        if c.author_kind != AUTHOR_REPORTER {
            continue;
        }
        match c.reporter_id.as_deref() {
            Some(rid) if rid == viewer => {
                if let Some(n) = name_of(rid) {
                    c.author_name = n;
                }
            }
            other => {
                c.author_name = other
                    .and_then(name_of)
                    .unwrap_or_else(|| ANOTHER_REPORTER.to_string());
            }
        }
    }
    if !mine {
        r.reporter_id = None;
        r.metadata = serde_json::Value::Null;
    }
    Some(ReporterReport {
        report: r,
        submitter,
    })
}

/// A view session's row on `c`, by its token's SHA-256, for queue `qid`.
fn session_in(c: &Connection, qid: &str, sha256: &str) -> Result<Option<ViewSession>> {
    Ok(c
        .prepare_cached(
            "SELECT s.queue_id, s.service_grant_id, s.reporter_id, s.expires_at
             FROM queue_view_sessions s
             JOIN queue_service_grants g ON g.id = s.service_grant_id
             JOIN queues q ON q.id = s.queue_id
             WHERE s.token_sha256 = ?1 AND s.queue_id = ?2
               AND g.revoked_at IS NULL AND q.deleted_at IS NULL",
        )?
        .query_row(params![sha256, qid], |r| {
            Ok(ViewSession {
                queue_id: r.get(0)?,
                service_grant_id: r.get(1)?,
                reporter_id: r.get(2)?,
                expires_at: r.get(3)?,
            })
        })
        .optional()?)
}

impl Store {
    /// A page of the reports reporter `rid` of queue `qid` reads, after
    /// report `cursor`, each as [`project`] makes it; and the next page's
    /// cursor.
    ///
    /// # Errors
    /// [`Store::reports`]'s.
    pub fn reporter_reports(
        &self,
        qid: &str,
        rid: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<ReporterReport>, Option<String>)> {
        let (reports, next) = self.reports(qid, Visibility::Reporter(rid), None, cursor)?;
        let (q, names) = self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            Ok((q, names_in(c, qid)?))
        })?;
        let page = reports
            .into_iter()
            .filter_map(|r| project(r, rid, &q, &names))
            .collect();
        Ok((page, next))
    }

    /// Report `tid` of queue `qid` as reporter `rid` reads it; `None` for
    /// one it does not see.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue.
    pub fn reporter_report(
        &self,
        qid: &str,
        tid: &str,
        rid: &str,
    ) -> Result<Option<ReporterReport>> {
        let now = self.now();
        self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            let r = report_in(c, &q, tid, Visibility::Reporter(rid), &now)?;
            let names = names_in(c, qid)?;
            Ok(r.and_then(|r| project(r, rid, &q, &names)))
        })
    }

    /// `r`, a report of queue `qid` a write of reporter `rid` answers, as
    /// that reporter reads it.
    ///
    /// # Errors
    /// [`CoreError::NotAMember`] for no such queue; `NotFound` for a report
    /// `rid` does not see.
    pub fn as_reporter(&self, qid: &str, rid: &str, r: Report) -> Result<ReporterReport> {
        let (q, names) = self.with_read(|c| {
            let q = queue_in(c, qid)?.ok_or(CoreError::NotAMember)?;
            Ok((q, names_in(c, qid)?))
        })?;
        project(r, rid, &q, &names).ok_or(CoreError::NotFound)
    }

    /// A view-link code ([`VIEW_CODE_PREFIX`], valid for
    /// [`VIEW_CODE_SECS`], single-use) for reporter `rid` of queue `qid`
    /// through service grant `gid`, opening on report `report_id` when
    /// given; and its expiry. Codes that expired are dropped.
    ///
    /// # Errors
    /// `NotFound` for a report `rid` does not see, or a grant or queue that
    /// is gone.
    pub fn mint_view_code(
        &self,
        qid: &str,
        gid: &str,
        rid: &str,
        report_id: Option<&str>,
    ) -> Result<(Secret, String)> {
        if let Some(tid) = report_id {
            self.report(qid, tid, Visibility::Reporter(rid))?
                .ok_or(CoreError::NotFound)?;
        }
        let code = clax_core::secret::token(VIEW_CODE_PREFIX);
        let now = self.now_utc();
        let expires_at = later(now, VIEW_CODE_SECS);
        self.with_tx(|tx| {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_service_grants g JOIN queues q ON q.id = g.queue_id
                    WHERE g.id = ?1 AND g.queue_id = ?2 AND g.revoked_at IS NULL AND q.deleted_at IS NULL)",
                params![gid, qid],
                |r| r.get(0),
            )?;
            if !live {
                return Err(CoreError::NotFound);
            }
            tx.execute(
                "DELETE FROM queue_view_codes WHERE expires_at <= ?1",
                params![clax_core::time::stamp(now)],
            )?;
            tx.execute(
                "INSERT INTO queue_view_codes (code_sha256, queue_id, service_grant_id, reporter_id,
                    report_id, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![code.sha256_hex(), qid, gid, rid, report_id, expires_at],
            )?;
            Ok(())
        })?;
        Ok((code, expires_at))
    }

    /// Trades view-link `code` of queue `qid` for a view session
    /// ([`VIEW_SESSION_PREFIX`], valid for [`VIEW_SESSION_SECS`]): its token,
    /// the session, and the report the link opens on. The code is spent
    /// whatever the answer. Sessions that expired are dropped.
    ///
    /// # Errors
    /// `NotFound` for a code that is unknown, spent, of another queue, or
    /// whose grant or queue is gone; [`CoreError::TokenExpired`] for one
    /// past its expiry.
    pub fn redeem_view_code(
        &self,
        qid: &str,
        code: &str,
    ) -> Result<(Secret, ViewSession, Option<String>)> {
        let sha = clax_core::secret::sha256_hex(code);
        let now_utc = self.now_utc();
        let now = clax_core::time::stamp(now_utc);
        let token = clax_core::secret::token(VIEW_SESSION_PREFIX);
        let expires_at = later(now_utc, VIEW_SESSION_SECS);
        self.with_tx(|tx| {
            let row: Option<(String, String, String, Option<String>, String)> = tx
                .query_row(
                    "SELECT c.queue_id, c.service_grant_id, c.reporter_id, c.report_id, c.expires_at
                     FROM queue_view_codes c
                     JOIN queue_service_grants g ON g.id = c.service_grant_id
                     JOIN queues q ON q.id = c.queue_id
                     WHERE c.code_sha256 = ?1 AND g.revoked_at IS NULL AND q.deleted_at IS NULL",
                    params![sha],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            tx.execute(
                "DELETE FROM queue_view_codes WHERE code_sha256 = ?1",
                params![sha],
            )?;
            let Some((queue_id, gid, rid, report_id, code_expiry)) = row else {
                return Err(CoreError::NotFound);
            };
            if queue_id != qid {
                return Err(CoreError::NotFound);
            }
            if code_expiry <= now {
                return Err(CoreError::TokenExpired);
            }
            tx.execute(
                "DELETE FROM queue_view_sessions WHERE expires_at <= ?1",
                params![now],
            )?;
            tx.execute(
                "INSERT INTO queue_view_sessions (token_sha256, queue_id, service_grant_id,
                    reporter_id, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![token.sha256_hex(), qid, gid, rid, expires_at],
            )?;
            Ok((
                ViewSession {
                    queue_id,
                    service_grant_id: gid,
                    reporter_id: rid,
                    expires_at: expires_at.clone(),
                },
                report_id,
            ))
        })
        .map(|(s, report)| (token, s, report))
    }

    /// The live view session of queue `qid` whose token's SHA-256
    /// (lowercase hex) is `sha256`; `None` for no such session, or one
    /// past its expiry.
    ///
    /// # Errors
    /// The store's.
    pub fn view_session(&self, qid: &str, sha256: &str) -> Result<Option<ViewSession>> {
        let now = self.now();
        let s = self.with_read(|c| session_in(c, qid, sha256))?;
        Ok(s.filter(|s| s.expires_at > now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::queue::{ReportArtifact, ReportComment, State};

    fn queue(see: ReportersSee) -> QueueRow {
        QueueRow {
            id: "q".into(),
            name: "Q".into(),
            person: None,
            origins: vec![],
            reporters_see: see,
            created_at: String::new(),
        }
    }

    fn comment(kind: &str, name: &str, rid: Option<&str>) -> ReportComment {
        ReportComment {
            id: format!("c-{}", name.len()),
            author_kind: kind.into(),
            author_name: name.into(),
            body: "b".into(),
            created_at: String::new(),
            reporter_id: rid.map(str::to_string),
        }
    }

    fn report(rid: &str, comments: Vec<ReportComment>) -> Report {
        Report {
            id: "t-1".into(),
            queue_id: "q".into(),
            artifact: serde_json::from_value::<ReportArtifact>(serde_json::json!({
                "id": "a", "external_id": "x", "url": "https://app.example/x",
                "title": "X", "metadata": null, "metadata_version": 1, "revision": null
            }))
            .unwrap(),
            anchor: serde_json::from_value(serde_json::json!({"kind": "artifact"})).unwrap(),
            status: "open".into(),
            state: State::Open,
            reporter_id: Some(rid.into()),
            metadata: serde_json::json!({"logs": ["alice@example.com"]}),
            metadata_version: 1,
            has_clip: false,
            comments,
            claim: None,
            created_at: String::new(),
        }
    }

    fn names() -> HashMap<String, String> {
        HashMap::from([("emp-a".to_string(), "alice".to_string())])
    }

    #[test]
    fn under_own_another_reporters_report_is_not_read_and_no_submitter_is_named() {
        let q = queue(ReportersSee::Own);
        let a = report("emp-a", vec![comment(AUTHOR_REPORTER, "emp-a", Some("emp-a"))]);
        assert_eq!(project(a.clone(), "emp-b", &q, &names()), None);
        let mine = project(a, "emp-a", &q, &names()).unwrap();
        assert_eq!(mine.submitter, None);
        assert_eq!(mine.report.reporter_id.as_deref(), Some("emp-a"));
        assert_eq!(mine.report.comments[0].author_name, "alice");
        let v = serde_json::to_value(&mine).unwrap();
        assert!(v.get("submitter").is_none(), "{v}");
    }

    #[test]
    fn under_all_another_reporter_is_its_name_never_its_id_or_metadata() {
        let q = queue(ReportersSee::All);
        let a = report(
            "emp-a",
            vec![
                comment(AUTHOR_REPORTER, "emp-a", Some("emp-a")),
                comment("agent", "triage bot", None),
                comment(AUTHOR_REPORTER, "emp-c", Some("emp-c")),
            ],
        );
        let seen = project(a.clone(), "emp-b", &q, &names()).unwrap();
        assert_eq!(
            seen.submitter,
            Some(Submitter {
                name: Some("alice".into()),
                you: false
            })
        );
        assert_eq!(seen.report.reporter_id, None);
        assert_eq!(seen.report.metadata, serde_json::Value::Null);
        let authors: Vec<_> = seen
            .report
            .comments
            .iter()
            .map(|c| c.author_name.as_str())
            .collect();
        assert_eq!(authors, ["alice", "triage bot", ANOTHER_REPORTER]);
        let text = serde_json::to_string(&seen).unwrap();
        for leak in ["emp-a", "emp-c", "alice@example.com", "reporter_id\":\"emp"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }

        let mine = project(a, "emp-a", &q, &names()).unwrap();
        assert_eq!(
            mine.submitter,
            Some(Submitter {
                name: Some("alice".into()),
                you: true
            })
        );
        assert_eq!(mine.report.reporter_id.as_deref(), Some("emp-a"));
        assert!(mine.report.metadata.is_object());
    }
}
