//! The provider failure log (migration 66): each failed request made with
//! the instance's model key, kept for the instance's admins.
//!
//! A person whose run used the instance's key is told, in Clax's own
//! words, that ClaxBot could not use this Clax's model provider; what the
//! provider answered is kept here instead, with when, the person the
//! request was for, the run it was made for (or the admin action:
//! `provider.test`, `provider.models`), the model, the API, the base URL's
//! host and the key's source. The provider's words are untrusted: plain
//! text, each member cut to [`crate::secrets::PROBLEM_FIELD_BYTES`], never
//! the key (the caller's detail holds none).
//!
//! # Bounds
//!
//! At most [`FAILURES_PER_PERSON`] entries of one person, so that one
//! person's failing runs push no one else's out, [`FAILURES_KEPT`] in all,
//! none older than [`FAILURE_KEEP`]. Each [`Store::log_provider_failure`]
//! deletes, in its own transaction, at most [`PRUNE_PER_LOG`] rows of each
//! bound it finds over (one entry is added, so the log never grows past a
//! bound for long); [`Store::prune_ended_runs`] deletes the entries past
//! their age, a bounded number per transaction.

use super::Store;
use clax_core::Result;
use rusqlite::{Connection, Row, params};
use serde_json::Value;

/// The most entries kept of one person.
pub const FAILURES_PER_PERSON: usize = 100;

/// The most entries kept in all.
pub const FAILURES_KEPT: usize = 1000;

/// How long an entry is kept.
pub const FAILURE_KEEP: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

/// The most rows [`Store::log_provider_failure`] deletes of each bound it
/// finds over.
pub const PRUNE_PER_LOG: usize = 16;

/// The most entries one page of [`Store::provider_failures`] holds.
pub const MAX_PAGE: usize = 200;

/// A failed request to log ([`Store::log_provider_failure`]).
#[derive(Clone, Debug)]
pub struct NewFailure<'a> {
    /// The person key the request was for: a run's person, or the admin
    /// who asked.
    pub person: &'a str,
    /// The run it was made for; `None` for an admin action.
    pub run_id: Option<&'a str>,
    /// A run's kind (`clax.thread`, `clax.brief`, a host surface's), or
    /// the admin action (`provider.test`, `provider.models`).
    pub request: &'a str,
    pub model: Option<&'a str>,
    /// The wire API (`anthropic`, `openai`, `gemini`).
    pub api: &'a str,
    /// The base URL's host (never the URL's path or query).
    pub host: Option<&'a str>,
    /// `instance`.
    pub key_source: &'a str,
    /// What the provider answered: `{class, status, type, message,
    /// request_id}` (`clax_sample`'s `ErrorDetail::to_json`).
    pub detail: &'a Value,
}

/// One entry of the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderFailure {
    pub id: i64,
    pub at: String,
    pub person: String,
    pub run_id: Option<String>,
    pub request: String,
    pub model: Option<String>,
    pub api: String,
    pub host: Option<String>,
    pub key_source: String,
    pub class: String,
    pub status: Option<i64>,
    pub provider_type: Option<String>,
    pub message: Option<String>,
    pub request_id: Option<String>,
}

/// What [`Store::provider_failures`] lists: each `Some` narrows it.
#[derive(Clone, Debug, Default)]
pub struct FailureQuery {
    pub person: Option<String>,
    pub run_id: Option<String>,
    pub model: Option<String>,
    pub class: Option<String>,
    pub status: Option<i64>,
    /// At or after (a stamp).
    pub since: Option<String>,
    /// Before (a stamp).
    pub until: Option<String>,
    /// The page after this cursor ([`FailurePage::next`]).
    pub before: Option<String>,
    /// The most entries of the page, at most [`MAX_PAGE`].
    pub limit: usize,
}

/// One page of the log, newest first, and the cursor of the next when
/// there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailurePage {
    pub failures: Vec<ProviderFailure>,
    pub next: Option<String>,
}

/// The cursor of the page after entry `f`: its time and ID.
fn cursor_of(f: &ProviderFailure) -> String {
    format!("{}/{}", f.at, f.id)
}

/// The time and ID cursor `c` holds, or `None` for one this store did not
/// make.
fn parse_cursor(c: &str) -> Option<(String, i64)> {
    let (at, id) = c.rsplit_once('/')?;
    Some((at.to_string(), id.parse().ok()?))
}

/// `v` as a string of at most [`crate::secrets::PROBLEM_FIELD_BYTES`]
/// bytes, cut on a character boundary; `None` for anything but a string.
fn bounded(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    let mut end = s.len().min(crate::secrets::PROBLEM_FIELD_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].to_string())
}

const COLS: &str = "id, at, person, run_id, request, model, api, host, key_source, class, \
                    status, provider_type, message, request_id";

fn row_to_failure(r: &Row<'_>) -> rusqlite::Result<ProviderFailure> {
    Ok(ProviderFailure {
        id: r.get(0)?,
        at: r.get(1)?,
        person: r.get(2)?,
        run_id: r.get(3)?,
        request: r.get(4)?,
        model: r.get(5)?,
        api: r.get(6)?,
        host: r.get(7)?,
        key_source: r.get(8)?,
        class: r.get(9)?,
        status: r.get(10)?,
        provider_type: r.get(11)?,
        message: r.get(12)?,
        request_id: r.get(13)?,
    })
}

/// Deletes, on `c`, at most `limit` of the oldest entries `scope` holds
/// past `keep` of them (`scope`: one person's, or all with `None`).
fn trim_in(c: &Connection, person: Option<&str>, keep: usize, limit: usize) -> Result<usize> {
    let held: i64 = match person {
        Some(p) => c
            .prepare_cached("SELECT count(*) FROM bot_provider_failures WHERE person = ?1")?
            .query_row([p], |r| r.get(0))?,
        None => c
            .prepare_cached("SELECT count(*) FROM bot_provider_failures")?
            .query_row([], |r| r.get(0))?,
    };
    let over = usize::try_from(held).unwrap_or(0).saturating_sub(keep);
    if over == 0 {
        return Ok(0);
    }
    let n = i64::try_from(over.min(limit)).unwrap_or(i64::MAX);
    Ok(match person {
        Some(p) => c
            .prepare_cached(
                "DELETE FROM bot_provider_failures WHERE id IN
                   (SELECT id FROM bot_provider_failures WHERE person = ?1
                     ORDER BY at, id LIMIT ?2)",
            )?
            .execute(params![p, n])?,
        None => c
            .prepare_cached(
                "DELETE FROM bot_provider_failures WHERE id IN
                   (SELECT id FROM bot_provider_failures ORDER BY at, id LIMIT ?1)",
            )?
            .execute(params![n])?,
    })
}

/// Deletes, on `c`, at most `limit` entries from before `cut` (a stamp),
/// the oldest first; how many.
pub(crate) fn prune_aged_in(c: &Connection, cut: &str, limit: usize) -> Result<usize> {
    Ok(c.prepare_cached(
        "DELETE FROM bot_provider_failures WHERE id IN
           (SELECT id FROM bot_provider_failures WHERE at < ?1 ORDER BY at, id LIMIT ?2)",
    )?
    .execute(params![cut, i64::try_from(limit).unwrap_or(i64::MAX)])?)
}

/// The stamp before which an entry is past [`FAILURE_KEEP`] at `now`.
pub(crate) fn aged_cut(now: chrono::DateTime<chrono::Utc>) -> String {
    clax_core::time::stamp(
        now - chrono::Duration::from_std(FAILURE_KEEP).unwrap_or(chrono::Duration::MAX),
    )
}

impl Store {
    /// Logs failure `f` now (the store's clock) and keeps the log within
    /// its bounds (the module's documentation); the entry's ID.
    ///
    /// # Errors
    /// The store's.
    pub fn log_provider_failure(&self, f: &NewFailure<'_>) -> Result<i64> {
        let now = self.now_utc();
        self.with_tx(|tx| {
            let d = f.detail;
            tx.prepare_cached(
                "INSERT INTO bot_provider_failures (at, person, run_id, request, model, api, host,
                   key_source, class, status, provider_type, message, request_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?
            .execute(params![
                clax_core::time::stamp(now),
                f.person,
                f.run_id,
                f.request,
                f.model,
                f.api,
                f.host,
                f.key_source,
                bounded(d.get("class")).unwrap_or_else(|| "api".into()),
                d.get("status").and_then(Value::as_i64),
                bounded(d.get("type")),
                bounded(d.get("message")),
                bounded(d.get("request_id")),
            ])?;
            let id = tx.last_insert_rowid();
            prune_aged_in(tx, &aged_cut(now), PRUNE_PER_LOG)?;
            trim_in(tx, Some(f.person), FAILURES_PER_PERSON, PRUNE_PER_LOG)?;
            trim_in(tx, None, FAILURES_KEPT, PRUNE_PER_LOG)?;
            Ok(id)
        })
    }

    /// One page of the log as `q` narrows it, newest first (by time, then
    /// ID); a cursor this store did not make lists from the newest.
    ///
    /// # Errors
    /// The store's.
    pub fn provider_failures(&self, q: &FailureQuery) -> Result<FailurePage> {
        let limit = q.limit.clamp(1, MAX_PAGE);
        let cursor = q.before.as_deref().and_then(parse_cursor);
        self.with_read(|c| {
            let mut sql = format!("SELECT {COLS} FROM bot_provider_failures WHERE 1");
            let mut args: Vec<rusqlite::types::Value> = Vec::new();
            let mut arg = |sql: &mut String, cond: &str, v: rusqlite::types::Value| {
                args.push(v);
                sql.push_str(&cond.replace('?', &format!("?{}", args.len())));
            };
            use rusqlite::types::Value as V;
            if let Some(p) = &q.person {
                arg(&mut sql, " AND person = ?", V::Text(p.clone()));
            }
            if let Some(r) = &q.run_id {
                arg(&mut sql, " AND run_id = ?", V::Text(r.clone()));
            }
            if let Some(m) = &q.model {
                arg(&mut sql, " AND model = ?", V::Text(m.clone()));
            }
            if let Some(k) = &q.class {
                arg(&mut sql, " AND class = ?", V::Text(k.clone()));
            }
            if let Some(s) = q.status {
                arg(&mut sql, " AND status = ?", V::Integer(s));
            }
            if let Some(s) = &q.since {
                arg(&mut sql, " AND at >= ?", V::Text(s.clone()));
            }
            if let Some(u) = &q.until {
                arg(&mut sql, " AND at < ?", V::Text(u.clone()));
            }
            if let Some((at, id)) = cursor {
                arg(&mut sql, " AND (at < ?", V::Text(at.clone()));
                arg(&mut sql, " OR (at = ?", V::Text(at));
                arg(&mut sql, " AND id < ?))", V::Integer(id));
            }
            arg(
                &mut sql,
                " ORDER BY at DESC, id DESC LIMIT ?",
                V::Integer(i64::try_from(limit + 1).unwrap_or(i64::MAX)),
            );
            let mut failures = c
                .prepare(&sql)?
                .query_map(rusqlite::params_from_iter(args), row_to_failure)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let next = if failures.len() > limit {
                failures.truncate(limit);
                failures.last().map(cursor_of)
            } else {
                None
            };
            Ok(FailurePage { failures, next })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::working::ManualClock;
    use serde_json::json;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store, Arc<ManualClock>) {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at("2026-11-01T00:00:00Z"));
        let st = Store::open_with_clock(&clax_core::Home::at(dir.path().join("ax")), clock.clone())
            .unwrap();
        (dir, st, clock)
    }

    fn tick(c: &ManualClock, secs: i64) {
        c.advance(secs);
    }

    fn log(st: &Store, person: &str, run: Option<&str>, message: &str) -> i64 {
        st.log_provider_failure(&NewFailure {
            person,
            run_id: run,
            request: if run.is_some() {
                "clax.thread"
            } else {
                "provider.test"
            },
            model: Some("claude-sonnet-5-5"),
            api: "anthropic",
            host: Some("api.anthropic.com"),
            key_source: "instance",
            detail: &json!({"class": "invalid_request", "status": 400,
                            "type": "invalid_request_error", "message": message,
                            "request_id": "req_1"}),
        })
        .unwrap()
    }

    fn all(st: &Store, q: FailureQuery) -> Vec<ProviderFailure> {
        let mut out = Vec::new();
        let mut q = FailureQuery { limit: 7, ..q };
        loop {
            let page = st.provider_failures(&q).unwrap();
            out.extend(page.failures);
            match page.next {
                Some(n) => q.before = Some(n),
                None => return out,
            }
        }
    }

    #[test]
    fn an_entry_keeps_what_was_logged_bounded_and_lists_newest_first() {
        let (_d, st, clock) = store();
        let long = format!("{}€tail", "x".repeat(511));
        let first = log(&st, "u_mia", Some("r1"), &long);
        tick(&clock, 1);
        let second = log(&st, "u_sam", None, "said <b>this</b>");
        let page = st
            .provider_failures(&FailureQuery {
                limit: 50,
                ..FailureQuery::default()
            })
            .unwrap();
        assert_eq!(page.next, None);
        let ids: Vec<i64> = page.failures.iter().map(|f| f.id).collect();
        assert_eq!(ids, [second, first]);
        let f = &page.failures[1];
        assert_eq!(f.person, "u_mia");
        assert_eq!(f.run_id.as_deref(), Some("r1"));
        assert_eq!(f.request, "clax.thread");
        assert_eq!(f.class, "invalid_request");
        assert_eq!(f.status, Some(400));
        assert_eq!(f.provider_type.as_deref(), Some("invalid_request_error"));
        assert_eq!(f.message.as_deref(), Some("x".repeat(511).as_str()));
        assert_eq!(f.request_id.as_deref(), Some("req_1"));
        assert_eq!(f.host.as_deref(), Some("api.anthropic.com"));
        assert_eq!(
            page.failures[0].message.as_deref(),
            Some("said <b>this</b>")
        );
    }

    #[test]
    fn the_log_narrows_by_person_run_model_class_status_and_time_and_pages() {
        let (_d, st, clock) = store();
        for i in 0..10 {
            log(
                &st,
                if i % 2 == 0 { "u_mia" } else { "u_sam" },
                Some(&format!("r{i}")),
                "m",
            );
            tick(&clock, 60);
        }
        let mia = all(
            &st,
            FailureQuery {
                person: Some("u_mia".into()),
                ..FailureQuery::default()
            },
        );
        assert_eq!(mia.len(), 5);
        assert!(mia.iter().all(|f| f.person == "u_mia"));
        assert!(mia.windows(2).all(|w| w[0].at > w[1].at), "newest first");
        let r3 = all(
            &st,
            FailureQuery {
                run_id: Some("r3".into()),
                ..FailureQuery::default()
            },
        );
        assert_eq!(r3.len(), 1);
        assert_eq!(r3[0].person, "u_sam");
        assert_eq!(
            all(
                &st,
                FailureQuery {
                    model: Some("other".into()),
                    ..FailureQuery::default()
                }
            )
            .len(),
            0
        );
        assert_eq!(
            all(
                &st,
                FailureQuery {
                    class: Some("invalid_request".into()),
                    ..FailureQuery::default()
                }
            )
            .len(),
            10
        );
        assert_eq!(
            all(
                &st,
                FailureQuery {
                    status: Some(500),
                    ..FailureQuery::default()
                }
            )
            .len(),
            0
        );
        let since = all(
            &st,
            FailureQuery {
                since: Some("2026-11-01T00:05:00.000Z".into()),
                until: Some("2026-11-01T00:08:00.000Z".into()),
                ..FailureQuery::default()
            },
        );
        let runs: Vec<&str> = since.iter().filter_map(|f| f.run_id.as_deref()).collect();
        assert_eq!(runs, ["r7", "r6", "r5"]);
        // Paging visits each entry once.
        let every = all(&st, FailureQuery::default());
        assert_eq!(every.len(), 10);
        // A cursor this store did not make lists from the newest.
        let page = st
            .provider_failures(&FailureQuery {
                before: Some("junk".into()),
                limit: 3,
                ..FailureQuery::default()
            })
            .unwrap();
        assert_eq!(page.failures[0].run_id.as_deref(), Some("r9"));
    }

    #[test]
    fn one_persons_failures_push_no_one_elses_out_and_the_log_is_bounded_in_all_and_by_age() {
        let (_d, st, clock) = store();
        log(&st, "u_sam", Some("r_sam"), "sam's");
        for i in 0..(FAILURES_PER_PERSON + 30) {
            log(&st, "u_mia", Some(&format!("r{i}")), "mia's");
        }
        let mia = all(
            &st,
            FailureQuery {
                person: Some("u_mia".into()),
                ..FailureQuery::default()
            },
        );
        assert_eq!(mia.len(), FAILURES_PER_PERSON);
        // The newest are kept.
        assert_eq!(
            mia[0].run_id.as_deref(),
            Some(format!("r{}", FAILURES_PER_PERSON + 29).as_str())
        );
        let sam = all(
            &st,
            FailureQuery {
                person: Some("u_sam".into()),
                ..FailureQuery::default()
            },
        );
        assert_eq!(sam.len(), 1);

        // In all: many people, each under their own bound.
        for i in 0..(FAILURES_KEPT + 40) {
            log(&st, &format!("u_{}", i % 50), None, "x");
        }
        let count = |st: &Store| {
            st.with_read(|c| {
                Ok(
                    c.query_row("SELECT count(*) FROM bot_provider_failures", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                )
            })
            .unwrap()
        };
        assert_eq!(count(&st), i64::try_from(FAILURES_KEPT).unwrap());

        // By age: a later entry deletes what is past it, and a prune pass
        // the rest.
        tick(&clock, i64::try_from(FAILURE_KEEP.as_secs()).unwrap() + 1);
        log(&st, "u_new", None, "late");
        assert_eq!(
            count(&st),
            i64::try_from(FAILURES_KEPT - PRUNE_PER_LOG + 1).unwrap()
        );
        loop {
            let p = st.prune_ended_runs().unwrap();
            if !p.more {
                break;
            }
        }
        assert_eq!(count(&st), 1);
    }
}
