//! ClaxBot's web tools in the store (migration 63, spec
//! 2026-11-02-claxbot-web-tools).
//!
//! - A person's own choice (§9.2): web search and web fetch are on for a
//!   person unless they turn them off, whatever the instance's
//!   `web.*.enabled` settings say; the instance settings still decide
//!   whether the tools exist at all. The choice is a row of
//!   `bot_person_prefs`, kept per person key.
//! - The Spend transaction (§4.2, §7.2): a web call's journal entry, its
//!   usage row and (for a fetch) its page commit together, before the
//!   script sees the answer ([`Store::spend_journaled`]). The usage row is
//!   charged on the run's open step and counted in the run's
//!   `credits_milli` at once; the step's commit adds its own rows on top
//!   and never rewrites it.
//! - The page store (§6.2): each page a fetch of the run received, in
//!   `bot_web_pages` by normalised URL, removed with the run's call
//!   journal ([`Store::prune_ended_runs`]). A URL the run keeps is served
//!   from the store, so a second page of it is refused (`page_exists`).

use super::Store;
use crate::bot_runs::{
    CALL_COLS, CallEntry, DriverId, Run, call_in, check_claim, insert_call, open_step_in,
    row_to_call, run_in, same_call,
};
use crate::credits::{Metered, charge_in};
use clax_core::audit::{Actor, AuditCtx, AuditKind, AuditRecord};
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::BTreeMap;

/// One page of a run's page store: what a fetch at journal place
/// `(step_n, seq)` received from `url` (normalised), its converted text cut
/// at the store's limit and `total_chars` the length of the whole, in
/// Unicode scalar values, and the links it held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebPageRow {
    pub url: String,
    pub step_n: i64,
    pub seq: i64,
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub text: String,
    pub total_chars: i64,
    pub links: Vec<String>,
}

/// A web call to journal with what it cost and kept
/// ([`Store::spend_journaled`]).
#[derive(Clone, Debug)]
pub struct Spend {
    /// The call's journal entry, answered by the caller
    /// ([`CallEntry::answered`]).
    pub entry: CallEntry,
    /// What to charge the run's person; `None` charges nothing (a search
    /// that failed).
    pub usage: Option<Metered>,
    /// The page a fetch received, kept at the entry's journal place.
    pub page: Option<WebPageRow>,
    /// The call's `bot.web` record: the store sets its artifact, `run`,
    /// `step` and `credits_milli`.
    pub audit: AuditRecord,
}

/// What [`Store::spend_journaled`] answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Spent {
    /// The journal's entry for the call: the one just written, or the one
    /// already there.
    pub entry: CallEntry,
    /// What this call charged: 0 when it was replayed or not charged.
    pub credits_milli: i64,
    /// The journal already held the call, and nothing was written.
    pub replayed: bool,
}

const PAGE_COLS: &str =
    "url, step_n, seq, final_url, status, content_type, text, total_chars, links_json";

fn row_to_page(r: &Row<'_>) -> rusqlite::Result<WebPageRow> {
    let links: String = r.get(8)?;
    Ok(WebPageRow {
        url: r.get(0)?,
        step_n: r.get(1)?,
        seq: r.get(2)?,
        final_url: r.get(3)?,
        status: r.get(4)?,
        content_type: r.get(5)?,
        text: r.get(6)?,
        total_chars: r.get(7)?,
        links: serde_json::from_str(&links).unwrap_or_default(),
    })
}

/// Keeps `page` for run `run_id` at journal place `(step_n, seq)` on `c`.
///
/// # Errors
/// `page_exists` when the run already keeps a page of its URL (a fetch of
/// it is answered from the store, never fetched again); the store's.
fn insert_page(
    c: &Connection,
    run_id: &str,
    step_n: i64,
    seq: i64,
    page: &WebPageRow,
) -> Result<()> {
    let inserted = c.execute(
        "INSERT INTO bot_web_pages (run_id, url, step_n, seq, final_url, status, content_type,
            text, total_chars, links_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (run_id, url) DO NOTHING",
        params![
            run_id,
            page.url,
            step_n,
            seq,
            page.final_url,
            page.status,
            page.content_type,
            page.text,
            page.total_chars,
            serde_json::to_string(&page.links).expect("serialisable links"),
        ],
    )?;
    if inserted == 0 {
        return Err(CoreError::invalid(
            "page_exists",
            format!(
                "run {run_id} already keeps a page of {}; answer from the store",
                page.url
            ),
        ));
    }
    Ok(())
}

impl Store {
    /// Whether `person` allows ClaxBot the web tools: `true` when they
    /// never chose.
    ///
    /// # Errors
    /// The store's.
    pub fn web_choice(&self, person: &str) -> Result<bool> {
        self.with_read(|c| {
            Ok(
                c.prepare_cached("SELECT web FROM bot_person_prefs WHERE person = ?1")?
                    .query_row(params![person], |r| r.get::<_, bool>(0))
                    .optional()?
                    .unwrap_or(true),
            )
        })
    }

    /// Sets whether `person` allows ClaxBot the web tools, as `ctx`; a
    /// change is recorded as `me.web {person, value}`. Returns whether the
    /// choice changed.
    ///
    /// # Errors
    /// The store's.
    pub fn set_web_choice(&self, ctx: &AuditCtx, person: &str, on: bool) -> Result<bool> {
        self.with_tx(|tx| {
            let was: bool = tx
                .query_row(
                    "SELECT web FROM bot_person_prefs WHERE person = ?1",
                    params![person],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(true);
            if was == on {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO bot_person_prefs (person, web, set_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (person) DO UPDATE SET web = excluded.web, set_at = excluded.set_at",
                params![person, on, self.now()],
            )?;
            let rec = AuditRecord::new(AuditKind::MeWeb, self.now())
                .with("person", person)
                .with("value", on);
            self.record_audit(tx, ctx, rec)?;
            Ok(true)
        })
    }

    /// Journals web call `s.entry` of `run` for `driver`, which holds the
    /// run's claim, with its charge, its page and its audit record, in one
    /// transaction (spec 2026-11-02-claxbot-web-tools §4.2):
    ///
    /// - When the journal already holds the entry's place `(run, step,
    ///   seq)` for the same op and arguments, answers with that entry,
    ///   `replayed`, and writes nothing.
    /// - Else charges `s.usage` to the run's person on the entry's step, as
    ///   a step's commit charges its rows (rates, free allowance, snapshot),
    ///   adds the charge to the run's `credits_milli`, keeps `s.page` at
    ///   the entry's place, journals the entry, and records `s.audit` with
    ///   the run's artifact, `run`, `step` and `credits_milli` (the charge
    ///   just computed, over any the caller set) set, as ClaxBot for the
    ///   run's person.
    ///
    /// # Errors
    /// `NotFound`; `run_not_claimed` when `driver` does not hold the run;
    /// `run_invalid` when the run has ended; `run_mismatch` for an entry of
    /// another run; `key_reused` when the place holds another call;
    /// `step_mismatch` when the entry's step is not the run's open step;
    /// `page_exists` when the run already keeps a page of `s.page`'s URL;
    /// the charge's (`unknown_cost_line`, `invalid_usage`); the store's.
    /// A refused Spend writes nothing.
    pub fn spend_journaled(&self, run: &Run, driver: &DriverId, s: Spend) -> Result<Spent> {
        let at = self.now_utc();
        let ov = self.bot_overrides();
        let Spend {
            entry,
            usage,
            page,
            audit,
        } = s;
        self.with_tx(|tx| {
            let run = run_in(tx, &run.id)?.ok_or(CoreError::NotFound)?;
            check_claim(&run, driver.as_str())?;
            if run.status.is_terminal() {
                return Err(CoreError::invalid(
                    "run_invalid",
                    format!("run {} has ended", run.id),
                ));
            }
            if entry.run_id() != run.id {
                return Err(CoreError::invalid(
                    "run_mismatch",
                    format!(
                        "call {} is of run {}, not {}",
                        entry.seq(),
                        entry.run_id(),
                        run.id
                    ),
                ));
            }
            if let Some(prev) = call_in(tx, &entry)? {
                same_call(&prev, &entry)?;
                return Ok(Spent {
                    entry: prev,
                    credits_milli: 0,
                    replayed: true,
                });
            }
            let n = entry.step_n();
            if open_step_in(tx, &run.id)?.is_none_or(|open| open.n != n) {
                return Err(CoreError::invalid(
                    "step_mismatch",
                    format!("step {n} of run {} is not begun and uncommitted", run.id),
                ));
            }
            let rows = charge_in(
                tx,
                &ov,
                &run.person,
                Some((run.id.as_str(), n)),
                usage.as_slice(),
                at,
            )?;
            let credits: i64 = rows.iter().map(|u| u.credits_milli).sum();
            let now = clax_core::time::stamp(at);
            if credits != 0 {
                tx.execute(
                    "UPDATE bot_runs SET credits_milli = credits_milli + ?2, updated_at = ?3
                      WHERE id = ?1",
                    params![run.id, credits, now],
                )?;
            }
            if let Some(page) = &page {
                insert_page(tx, &run.id, n, entry.seq(), page)?;
            }
            insert_call(tx, &entry, &now)?;
            let mut rec = audit;
            rec.ids.artifact = Some(run.artifact_id.clone());
            rec.body.insert("run".into(), run.id.as_str().into());
            rec.body.insert("step".into(), n.into());
            rec.body.insert("credits_milli".into(), credits.into());
            let ctx = AuditCtx {
                actor: Actor::Bot {
                    on_behalf_of: run.person.clone(),
                },
                ..AuditCtx::DAEMON
            };
            self.record_audit(tx, &ctx, rec)?;
            Ok(Spent {
                entry,
                credits_milli: credits,
                replayed: false,
            })
        })
    }

    /// The page run `run_id` keeps of `url` (normalised), or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn web_page(&self, run_id: &str, url: &str) -> Result<Option<WebPageRow>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {PAGE_COLS} FROM bot_web_pages WHERE run_id = ?1 AND url = ?2"
            ))?
            .query_row(params![run_id, url], row_to_page)
            .optional()?)
        })
    }

    /// Every web call (an op starting `web.`) journaled for run `id`, in
    /// all its steps, by step and then `seq`: in one read, where
    /// [`Store::bot_calls`] takes one per step.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_web_calls(&self, id: &str) -> Result<Vec<CallEntry>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {CALL_COLS} FROM bot_calls
                  WHERE run_id = ?1 AND op LIKE 'web.%'
                  ORDER BY step_n, seq"
            ))?;
            let rows = q.query_map(params![id], row_to_call)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// How many of run `id`'s journaled web calls each op made that its
    /// entry marks `counted` (a result whose `counted` is true: the call
    /// reached its source), by op; an op with none is absent. Counted in
    /// the database, without reading the entries' results out.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_web_counts(&self, id: &str) -> Result<BTreeMap<String, u32>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT op, COUNT(*) FROM bot_calls
                  WHERE run_id = ?1 AND op LIKE 'web.%'
                    AND json_extract(result_json, '$.counted') = 1
                  GROUP BY op",
            )?;
            let rows = q.query_map(params![id], |r| {
                let n: i64 = r.get(1)?;
                Ok((r.get(0)?, u32::try_from(n).unwrap_or(u32::MAX)))
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Every page run `run_id` keeps, in the order its fetches made them.
    ///
    /// # Errors
    /// The store's.
    pub fn web_pages(&self, run_id: &str) -> Result<Vec<WebPageRow>> {
        self.with_read(|c| {
            let mut q = c.prepare_cached(&format!(
                "SELECT {PAGE_COLS} FROM bot_web_pages WHERE run_id = ?1 ORDER BY step_n, seq"
            ))?;
            let rows = q.query_map(params![run_id], row_to_page)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::bot_runs::{CallEntry, DriverId, NewRun, RequestKind, Run, StepCommit, StepKind};
    use crate::credits::{KeySource, LINE_HARNESS_FEE, LINE_WEB_FETCH, Metered};
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, artifact, store};
    use clax_core::CoreError;
    use clax_core::audit::Actor;
    use serde_json::json;

    fn bot() -> AuditCtx {
        AuditCtx {
            actor: Actor::Bot {
                on_behalf_of: OWNER.into(),
            },
            ..DAEMON.clone()
        }
    }

    /// A run on `st` claimed by driver `d1`, its script step 1 begun.
    fn running(st: &Store) -> (Run, DriverId) {
        let aid = artifact(st, None).to_string();
        let run = st
            .create_run(
                &bot(),
                NewRun {
                    artifact_id: aid,
                    person: OWNER.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec!["t1".into()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                    web: "search,fetch".into(),
                },
            )
            .unwrap();
        let d = DriverId::named("d1");
        st.claim_run(&run.id, &d, 60).unwrap().unwrap();
        st.begin_step(&run.id, &d, 1, StepKind::Script).unwrap();
        let run = st.run(&run.id).unwrap().unwrap();
        (run, d)
    }

    fn page(url: &str) -> WebPageRow {
        WebPageRow {
            url: url.into(),
            step_n: 1,
            seq: 0,
            final_url: format!("{url}/"),
            status: 200,
            content_type: "text/html".into(),
            text: "# Hello\n\nA page.".into(),
            total_chars: 17,
            links: vec!["https://example.com/next".into()],
        }
    }

    fn fetch(st: &Store, run: &Run, d: &DriverId, seq: i64, url: &str) -> Spend {
        let entry = st
            .call_entry(&run.id, d, 1, 1, seq, "web.fetch", &format!("h-{url}"))
            .unwrap()
            .answered(true, json!({"url": url, "text": "# Hello", "offset": 0}));
        Spend {
            entry,
            usage: Some(Metered::new(LINE_WEB_FETCH, 1, KeySource::Instance)),
            page: Some(WebPageRow { seq, ..page(url) }),
            audit: AuditRecord::new(AuditKind::BotWeb, st.now())
                .with("op", "web.fetch")
                .with("url", url)
                .with("source", "comment"),
        }
    }

    fn web_events(st: &Store) -> Vec<crate::AuditRow> {
        st.events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "bot.web")
            .collect()
    }

    fn pages(st: &Store, run: &Run) -> usize {
        st.web_pages(&run.id).unwrap().len()
    }

    #[test]
    fn a_spend_journals_charges_and_stores_its_page_together() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        assert_eq!(run.web, "search,fetch");
        let url = "https://example.com/a";
        let spent = st
            .spend_journaled(&run, &d, fetch(&st, &run, &d, 0, url))
            .unwrap();
        assert!(!spent.replayed);
        assert!(spent.credits_milli > 0, "{spent:?}");
        assert_eq!(spent.entry.result()["url"], url);

        let rows = st.usage_of_run(&run.id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].line.as_str(), rows[0].step_n),
            (LINE_WEB_FETCH, Some(1))
        );
        assert_eq!(rows[0].credits_milli, spent.credits_milli);
        let after = st.run(&run.id).unwrap().unwrap();
        assert_eq!(after.credits_milli, run.credits_milli + spent.credits_milli);

        assert_eq!(st.web_page(&run.id, url).unwrap(), Some(page(url)));
        assert_eq!(st.web_page(&run.id, "https://example.com/b").unwrap(), None);
        let journal = st.bot_calls(&run.id, 1).unwrap();
        assert_eq!(journal, vec![spent.entry.clone()]);

        let events = web_events(&st);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].ids.artifact.as_deref(),
            Some(run.artifact_id.as_str())
        );
        let body: serde_json::Value = serde_json::from_str(&events[0].body).unwrap();
        assert_eq!(body["run"], run.id.as_str());
        assert_eq!(body["step"], 1);
        assert_eq!(body["url"], url);
        assert_eq!(body["credits_milli"], spent.credits_milli);
    }

    #[test]
    fn a_spend_at_a_journaled_place_writes_nothing() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let url = "https://example.com/a";
        let first = st
            .spend_journaled(&run, &d, fetch(&st, &run, &d, 0, url))
            .unwrap();
        let again = st
            .spend_journaled(&run, &d, fetch(&st, &run, &d, 0, url))
            .unwrap();
        assert!(again.replayed);
        assert_eq!((again.entry.clone(), again.credits_milli), (first.entry, 0));
        assert_eq!(st.usage_of_run(&run.id).unwrap().len(), 1);
        assert_eq!(pages(&st, &run), 1);
        assert_eq!(web_events(&st).len(), 1);
        assert_eq!(
            st.run(&run.id).unwrap().unwrap().credits_milli,
            first.credits_milli
        );

        let other = fetch(&st, &run, &d, 0, "https://example.com/other");
        match st.spend_journaled(&run, &d, other) {
            Err(CoreError::Invalid { code, .. }) => assert_eq!(code, "key_reused"),
            r => panic!("not refused: {r:?}"),
        }
        assert_eq!(st.usage_of_run(&run.id).unwrap().len(), 1);
        assert_eq!(pages(&st, &run), 1);
        assert_eq!(web_events(&st).len(), 1);
    }

    #[test]
    fn the_runs_web_calls_are_read_at_once_in_order() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let journal = |n: i64, seq: i64, op: &str| {
            let e = st
                .call_entry(&run.id, &d, n, 1, seq, op, &format!("h-{n}-{seq}"))
                .unwrap()
                .answered(true, json!({"n": n, "seq": seq}));
            st.record_bot_call(&e).unwrap()
        };
        // Written out of order, across steps, with calls that are not web
        // calls between them.
        let b = journal(2, 0, "web.fetch");
        journal(1, 1, "files.read");
        let a2 = journal(1, 2, "web.search");
        let a0 = journal(1, 0, "web.search");
        journal(2, 1, "webs.other");
        let (other, d2) = running(&st);
        let theirs = st
            .call_entry(&other.id, &d2, 1, 1, 0, "web.search", "h-other")
            .unwrap()
            .answered(true, json!({}));
        let theirs = st.record_bot_call(&theirs).unwrap();
        assert_eq!(st.bot_web_calls(&run.id).unwrap(), vec![a0, a2, b]);
        assert_eq!(st.bot_web_calls(&other.id).unwrap(), vec![theirs]);
        assert!(st.bot_web_calls("no-such-run").unwrap().is_empty());
    }

    #[test]
    fn the_runs_counted_web_calls_are_counted_by_op() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let journal = |seq: i64, op: &str, result: serde_json::Value| {
            let e = st
                .call_entry(&run.id, &d, 1, 1, seq, op, &format!("h-{seq}"))
                .unwrap()
                .answered(false, result);
            st.record_bot_call(&e).unwrap();
        };
        assert!(st.bot_web_counts(&run.id).unwrap().is_empty());
        journal(
            0,
            "web.search",
            json!({"code": "web_unavailable", "counted": true}),
        );
        journal(1, "web.search", json!({"code": "web_cap"}));
        journal(2, "web.search", json!({"code": "x", "counted": false}));
        journal(
            3,
            "web.fetch",
            json!({"code": "web_timeout", "counted": true}),
        );
        journal(4, "web.search", json!({"value": {}, "counted": true}));
        journal(5, "files.read", json!({"counted": true}));
        let counts = st.bot_web_counts(&run.id).unwrap();
        assert_eq!(
            counts.into_iter().collect::<Vec<_>>(),
            vec![("web.fetch".to_string(), 1), ("web.search".to_string(), 2)]
        );
    }

    #[test]
    fn a_spend_for_a_run_not_claimed_is_refused() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let s = fetch(&st, &run, &d, 0, "https://example.com/a");
        match st.spend_journaled(&run, &DriverId::named("d2"), s) {
            Err(CoreError::Invalid { code, .. }) => assert_eq!(code, "run_not_claimed"),
            r => panic!("not refused: {r:?}"),
        }
        assert!(st.usage_of_run(&run.id).unwrap().is_empty());
        assert_eq!(pages(&st, &run), 0);
        assert!(st.bot_calls(&run.id, 1).unwrap().is_empty());
        assert!(web_events(&st).is_empty());
        assert_eq!(st.run(&run.id).unwrap().unwrap().credits_milli, 0);
    }

    #[test]
    fn step_commit_counts_spend_on_top() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let c = st
            .spend_journaled(&run, &d, fetch(&st, &run, &d, 0, "https://example.com/a"))
            .unwrap()
            .credits_milli;
        let done = st
            .commit_bot_step(
                &bot(),
                &run.id,
                &d,
                1,
                StepCommit {
                    payload: json!({}),
                    usage: vec![Metered::new(LINE_HARNESS_FEE, 1, KeySource::Instance)],
                    ..Default::default()
                },
            )
            .unwrap();
        let k = done.credits_milli;
        assert!(c > 0 && k > 0, "{c} {k}");
        assert_eq!(done.run.credits_milli, c + k);
        assert_eq!(st.run(&run.id).unwrap().unwrap().credits_milli, c + k);
        assert_eq!(st.usage_of_run(&run.id).unwrap().len(), 2);
    }

    #[test]
    fn an_uncharged_spend_writes_no_row() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let entry = st
            .call_entry(&run.id, &d, 1, 1, 0, "web.search", "h-q")
            .unwrap()
            .answered(false, json!({"code": "web_unavailable", "message": "down"}));
        let s = Spend {
            entry,
            usage: None,
            page: None,
            audit: AuditRecord::new(AuditKind::BotWeb, st.now())
                .with("op", "web.search")
                .with("query", "rust"),
        };
        let spent = st.spend_journaled(&run, &d, s).unwrap();
        assert_eq!((spent.credits_milli, spent.replayed), (0, false));
        assert!(!spent.entry.ok());
        assert_eq!(st.bot_calls(&run.id, 1).unwrap(), vec![spent.entry]);
        assert!(st.usage_of_run(&run.id).unwrap().is_empty());
        assert_eq!(pages(&st, &run), 0);
        assert_eq!(st.run(&run.id).unwrap().unwrap().credits_milli, 0);
        assert_eq!(web_events(&st).len(), 1);
    }

    /// The code `s` is refused with, checking it wrote nothing: `run`'s
    /// usage rows, pages, journal entries of steps 1 and 2, `bot.web`
    /// records and credits are as they were.
    fn refused(st: &Store, run: &Run, d: &DriverId, s: Spend) -> &'static str {
        let counts = |st: &Store| {
            (
                st.usage_of_run(&run.id).unwrap().len(),
                pages(st, run),
                st.bot_calls(&run.id, 1).unwrap().len() + st.bot_calls(&run.id, 2).unwrap().len(),
                web_events(st).len(),
                st.run(&run.id).unwrap().unwrap().credits_milli,
            )
        };
        let before = counts(st);
        let code = match st.spend_journaled(run, d, s) {
            Err(CoreError::Invalid { code, .. }) => code,
            r => panic!("not refused: {r:?}"),
        };
        assert_eq!(counts(st), before, "{code}: nothing written");
        code
    }

    #[test]
    fn a_spend_on_an_ended_run_is_refused() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        // An ended run whose claim is still set: only the status refuses it.
        st.with_write(|c| {
            c.execute(
                "UPDATE bot_runs SET status = 'finished' WHERE id = ?1",
                rusqlite::params![run.id],
            )?;
            Ok(())
        })
        .unwrap();
        let s = fetch(&st, &run, &d, 0, "https://example.com/a");
        assert_eq!(refused(&st, &run, &d, s), "run_invalid");
    }

    #[test]
    fn a_second_page_of_a_kept_url_is_refused() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let url = "https://example.com/a";
        st.spend_journaled(&run, &d, fetch(&st, &run, &d, 0, url))
            .unwrap();
        let again = fetch(&st, &run, &d, 1, url);
        assert_eq!(refused(&st, &run, &d, again), "page_exists");
        assert_eq!(st.web_page(&run.id, url).unwrap(), Some(page(url)));
    }

    #[test]
    fn the_audit_record_carries_the_stores_charge() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let mut s = fetch(&st, &run, &d, 0, "https://example.com/a");
        s.audit = s.audit.with("credits_milli", 999_999);
        let spent = st.spend_journaled(&run, &d, s).unwrap();
        let body: serde_json::Value = serde_json::from_str(&web_events(&st)[0].body).unwrap();
        assert_eq!(body["credits_milli"], spent.credits_milli);
        assert_ne!(spent.credits_milli, 999_999);
    }

    #[test]
    fn a_spend_off_the_open_step_is_refused() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let mut ahead = fetch(&st, &run, &d, 0, "https://example.com/a");
        ahead.entry = CallEntry {
            step_n: 2,
            ..ahead.entry
        };
        assert_eq!(refused(&st, &run, &d, ahead), "step_mismatch");

        st.commit_bot_step(&bot(), &run.id, &d, 1, StepCommit::default())
            .unwrap();
        let behind = fetch(&st, &run, &d, 0, "https://example.com/a");
        assert_eq!(refused(&st, &run, &d, behind), "step_mismatch");
    }

    #[test]
    fn an_entry_of_another_run_is_refused() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let (other, _) = running(&st);
        let s = fetch(&st, &other, &d, 0, "https://example.com/a");
        assert_eq!(refused(&st, &run, &d, s), "run_mismatch");
        assert_eq!(pages(&st, &other), 0);
        assert!(st.bot_calls(&other.id, 1).unwrap().is_empty());
    }

    #[test]
    fn pages_go_with_the_journal_of_an_ended_run() {
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-11-02T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, fetch(&st, &run, &d, 0, "https://example.com/a"))
            .unwrap();
        let (going, d2) = running(&st);
        st.spend_journaled(
            &going,
            &d2,
            fetch(&st, &going, &d2, 0, "https://example.com/a"),
        )
        .unwrap();
        st.commit_bot_step(&bot(), &run.id, &d, 1, StepCommit::default())
            .unwrap();
        st.begin_step(&run.id, &d, 2, StepKind::GiveUp).unwrap();
        st.commit_bot_step(&bot(), &run.id, &d, 2, StepCommit::default())
            .unwrap();
        clock.advance(crate::bot_runs::BOT_JOURNAL_KEEP.as_secs() as i64 + 1);
        let p = st.prune_ended_runs().unwrap();
        assert_eq!((p.calls, p.pages), (1, 1));
        assert_eq!((pages(&st, &run), pages(&st, &going)), (0, 1));
    }

    #[test]
    fn a_person_chooses_web_off_and_back() {
        let (_d, st) = store();
        assert!(st.web_choice(OWNER).unwrap());
        assert!(!st.set_web_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.set_web_choice(DAEMON, OWNER, false).unwrap());
        assert!(!st.web_choice(OWNER).unwrap());
        assert!(!st.set_web_choice(DAEMON, OWNER, false).unwrap());
        let events: Vec<String> = st
            .events_after(0, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "me.web")
            .map(|e| e.body)
            .collect();
        assert_eq!(events.len(), 1, "one change, one event");
        assert!(events[0].contains("\"value\":false"), "{}", events[0]);
        assert!(st.set_web_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.web_choice(OWNER).unwrap());
        assert!(st.web_choice("someone-else").unwrap());
    }
}
