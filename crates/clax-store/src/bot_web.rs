//! ClaxBot's web tools in the store (migration 63, spec
//! 2026-11-02-claxbot-web-tools).
//!
//! - A person's own choice (§9.2), now their grants of each web
//!   capability to their runs (migration 67, spec
//!   2026-11-19-claxbot-browser-design §11.2, [`crate::bot_grants`]):
//!   [`Store::web_choice`] and [`Store::set_web_choice`] read and set both
//!   web capabilities' at once, [`Store::browser_choice`] and
//!   [`Store::set_browser_choice`] the three `browser.open` kinds'.
//!   `bot_person_prefs` is no longer read.
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
use crate::bot_grants::{BROWSER_CAPABILITIES, Grantee, WEB_CAPABILITIES};
use crate::bot_runs::{
    CALL_COLS, CallEntry, DriverId, Run, call_in, check_claim, insert_call, open_step_in,
    row_to_call, run_in, same_call,
};
use crate::credits::{LINE_BROWSER_TIME, Metered, charge_in, charge_rows_in};
use clax_core::audit::{Actor, AuditCtx, AuditRecord};
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
    /// The format it was fetched in: `markdown`, `html` or `raw`. The
    /// store keeps a page per URL and format.
    pub format: String,
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
    /// What a browser session call does to its session's row: opens it,
    /// moves its charged point, or closes it. `usage` then holds the
    /// segment, one item of [`LINE_BROWSER_TIME`] whose quantity is the
    /// point the call charges through less the session's charged point.
    pub session: Option<SessionSpend>,
    /// The call's `bot.spend` record: the store sets its artifact, `run`,
    /// `step` and `credits_milli`.
    pub audit: AuditRecord,
}

/// What a browser session is open on (spec 2026-11-19-claxbot-browser-design
/// §6.2): a public URL, the run's clax, or content the script supplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionKind {
    Url,
    Artifact,
    Content,
}

impl SessionKind {
    /// Every kind.
    pub const ALL: [SessionKind; 3] = [SessionKind::Url, SessionKind::Artifact, SessionKind::Content];

    /// Its name in `bot_browser_sessions.kind`: `url`, `artifact`, `content`.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Url => "url",
            SessionKind::Artifact => "artifact",
            SessionKind::Content => "content",
        }
    }

    /// The kind named `s`.
    pub fn parse(s: &str) -> Option<SessionKind> {
        SessionKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The capability a session of this kind needs: `browser.open.<kind>`.
    pub fn capability(self) -> &'static str {
        match self {
            SessionKind::Url => "browser.open.url",
            SessionKind::Artifact => "browser.open.artifact",
            SessionKind::Content => "browser.open.content",
        }
    }
}

/// Why a browser session ended (spec 2026-11-19-claxbot-browser-design §6.3,
/// §6.4): the reason `browser_session_closed` names, or `lost`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionEnd {
    /// The script closed it (`page.close()`).
    Closed,
    /// No call for the idle time.
    Idle,
    /// Open for its lifetime.
    Lifetime,
    /// A revoke the run's capabilities no longer cover.
    Revoked,
    /// A call refused `credits_exhausted`.
    Credits,
    /// The run ended.
    RunEnded,
    /// Its browser is gone: the daemon restarted, the socket dropped, the
    /// page crashed or detached.
    Lost,
}

impl SessionEnd {
    /// Every reason.
    pub const ALL: [SessionEnd; 7] = [
        SessionEnd::Closed,
        SessionEnd::Idle,
        SessionEnd::Lifetime,
        SessionEnd::Revoked,
        SessionEnd::Credits,
        SessionEnd::RunEnded,
        SessionEnd::Lost,
    ];

    /// Its name: `closed`, `idle`, `lifetime`, `revoked`, `credits`,
    /// `run_ended`, `lost`.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionEnd::Closed => "closed",
            SessionEnd::Idle => "idle",
            SessionEnd::Lifetime => "lifetime",
            SessionEnd::Revoked => "revoked",
            SessionEnd::Credits => "credits",
            SessionEnd::RunEnded => "run_ended",
            SessionEnd::Lost => "lost",
        }
    }

    /// The reason named `s`.
    pub fn parse(s: &str) -> Option<SessionEnd> {
        SessionEnd::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What a browser session call of a [`Spend`] does to its session's row
/// (spec 2026-11-19-claxbot-browser-design §10.3). Points are milliseconds
/// since the session opened (the WebSocket's upgrade); each call charges
/// the segment from the session's charged point to its own `through_ms`
/// and moves the point there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSpend {
    /// `browser.open`: the session's row, its first segment `0..through_ms`.
    Open {
        session: String,
        kind: SessionKind,
        opened_at: chrono::DateTime<chrono::Utc>,
        through_ms: i64,
    },
    /// A call on an open session; `kind` when the call changed what the
    /// session is (a `goto` out of its origin makes it a URL session).
    Call {
        session: String,
        through_ms: i64,
        kind: Option<SessionKind>,
    },
    /// `browser.close`: the last segment, and the session closed for
    /// `reason`.
    Close {
        session: String,
        through_ms: i64,
        reason: SessionEnd,
    },
}

impl SessionSpend {
    /// The session the call is on.
    pub fn session(&self) -> &str {
        match self {
            SessionSpend::Open { session, .. }
            | SessionSpend::Call { session, .. }
            | SessionSpend::Close { session, .. } => session,
        }
    }

    /// The point the call charges through.
    pub fn through_ms(&self) -> i64 {
        match self {
            SessionSpend::Open { through_ms, .. }
            | SessionSpend::Call { through_ms, .. }
            | SessionSpend::Close { through_ms, .. } => *through_ms,
        }
    }
}

/// A browser session of a run, as `bot_browser_sessions` keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub run_id: String,
    pub session_id: String,
    pub kind: SessionKind,
    pub opened_at: String,
    /// The point its time is charged through, in milliseconds since it
    /// opened.
    pub charged_ms: i64,
    pub closed_at: Option<String>,
    pub reason: Option<SessionEnd>,
}

impl SessionRow {
    /// Whether it is still open.
    pub fn is_open(&self) -> bool {
        self.closed_at.is_none()
    }
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

const SESSION_COLS: &str =
    "run_id, session_id, kind, opened_at, charged_ms, closed_at, reason";

fn row_to_session(r: &Row<'_>) -> rusqlite::Result<SessionRow> {
    let kind: String = r.get(2)?;
    let reason: Option<String> = r.get(6)?;
    Ok(SessionRow {
        run_id: r.get(0)?,
        session_id: r.get(1)?,
        // The table's CHECK holds the kind to the three names.
        kind: SessionKind::parse(&kind).unwrap_or(SessionKind::Url),
        opened_at: r.get(3)?,
        charged_ms: r.get(4)?,
        closed_at: r.get(5)?,
        reason: reason.as_deref().and_then(SessionEnd::parse),
    })
}

fn session_in(c: &Connection, run_id: &str, session: &str) -> Result<Option<SessionRow>> {
    Ok(c
        .prepare_cached(&format!(
            "SELECT {SESSION_COLS} FROM bot_browser_sessions
              WHERE run_id = ?1 AND session_id = ?2"
        ))?
        .query_row(params![run_id, session], row_to_session)
        .optional()?)
}

fn session_mismatch(run_id: &str, msg: String) -> CoreError {
    CoreError::invalid("session_mismatch", format!("run {run_id}: {msg}"))
}

/// Applies `s`, a session call of run `run_id` whose `usage` is `usage`,
/// to its session's row on `c` at `now` (see [`Spend::session`]).
///
/// # Errors
/// `session_mismatch` for an open of a session the run has, a call or
/// close of one it does not have or that is closed, a point before the
/// session's charged one, or a `usage` that does not hold the segment;
/// the store's.
fn apply_session(
    c: &Connection,
    run_id: &str,
    s: &SessionSpend,
    usage: Option<&Metered>,
    now: &str,
) -> Result<()> {
    let id = s.session();
    let through = s.through_ms();
    let row = session_in(c, run_id, id)?;
    let charged = match (s, &row) {
        (SessionSpend::Open { .. }, None) => 0,
        (SessionSpend::Open { .. }, Some(_)) => {
            return Err(session_mismatch(run_id, format!("session {id} is already open")));
        }
        (_, None) => return Err(session_mismatch(run_id, format!("no session {id}"))),
        (_, Some(r)) if !r.is_open() => {
            return Err(session_mismatch(run_id, format!("session {id} is closed")));
        }
        (_, Some(r)) => r.charged_ms,
    };
    if through < charged {
        return Err(session_mismatch(
            run_id,
            format!("session {id} is charged through {charged} ms, past {through} ms"),
        ));
    }
    let segment = through - charged;
    if !usage.is_some_and(|m| m.line == LINE_BROWSER_TIME && m.qty == segment) {
        return Err(session_mismatch(
            run_id,
            format!("a call of session {id} through {through} ms charges {segment} ms of {LINE_BROWSER_TIME}"),
        ));
    }
    match s {
        SessionSpend::Open {
            kind, opened_at, ..
        } => {
            c.execute(
                "INSERT INTO bot_browser_sessions (run_id, session_id, kind, opened_at, charged_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, id, kind.as_str(), clax_core::time::stamp(*opened_at), through],
            )?;
        }
        SessionSpend::Call { kind, .. } => {
            c.execute(
                "UPDATE bot_browser_sessions SET charged_ms = ?3, kind = COALESCE(?4, kind)
                  WHERE run_id = ?1 AND session_id = ?2",
                params![run_id, id, through, kind.map(SessionKind::as_str)],
            )?;
        }
        SessionSpend::Close { reason, .. } => {
            close_session_in(c, run_id, id, through, *reason, now)?;
        }
    }
    Ok(())
}

/// Closes session `id` of `run_id` on `c` at `at`, for `reason`, charged
/// through `through`: whether it was open.
fn close_session_in(
    c: &Connection,
    run_id: &str,
    id: &str,
    through: i64,
    reason: SessionEnd,
    at: &str,
) -> Result<bool> {
    Ok(c.execute(
        "UPDATE bot_browser_sessions SET charged_ms = MAX(charged_ms, ?3), closed_at = ?4,
                reason = ?5
          WHERE run_id = ?1 AND session_id = ?2 AND closed_at IS NULL",
        params![run_id, id, through, at, reason.as_str()],
    )? == 1)
}

const PAGE_COLS: &str =
    "url, step_n, seq, final_url, status, content_type, text, total_chars, links_json, format";

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
        format: r.get(9)?,
    })
}

/// Keeps `page` for run `run_id` at journal place `(step_n, seq)` on `c`.
///
/// # Errors
/// `page_exists` when the run already keeps a page of its URL in its
/// format (a fetch of it is answered from the store, never fetched
/// again); the store's.
fn insert_page(
    c: &Connection,
    run_id: &str,
    step_n: i64,
    seq: i64,
    page: &WebPageRow,
) -> Result<()> {
    let inserted = c.execute(
        "INSERT INTO bot_web_pages (run_id, url, step_n, seq, final_url, status, content_type,
            text, total_chars, links_json, format)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (run_id, url, format) DO NOTHING",
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
            page.format,
        ],
    )?;
    if inserted == 0 {
        return Err(CoreError::invalid(
            "page_exists",
            format!(
                "run {run_id} already keeps a page of {} as {}; answer from the store",
                page.url, page.format
            ),
        ));
    }
    Ok(())
}

impl Store {
    /// Whether both web capabilities reach `person`'s runs: each held by
    /// them and granted to their runs ([`Store::person_caps`]).
    ///
    /// # Errors
    /// The store's.
    pub fn web_choice(&self, person: &str) -> Result<bool> {
        self.family_reaches_runs(person, &WEB_CAPABILITIES)
    }

    /// Turns `person`'s grants of both web capabilities to their runs on
    /// or off, as `ctx` ([`Store::set_family_choice`]). Returns whether
    /// any grant changed.
    ///
    /// # Errors
    /// The store's.
    pub fn set_web_choice(&self, ctx: &AuditCtx, person: &str, on: bool) -> Result<bool> {
        self.set_family_choice(ctx, person, &WEB_CAPABILITIES, on)
    }

    /// Whether every `browser.open` kind reaches `person`'s runs.
    ///
    /// # Errors
    /// The store's.
    pub fn browser_choice(&self, person: &str) -> Result<bool> {
        self.family_reaches_runs(person, &BROWSER_CAPABILITIES)
    }

    /// Turns `person`'s grants of the three `browser.open` kinds to their
    /// runs on or off, as `ctx` ([`Store::set_family_choice`]). Returns
    /// whether any grant changed.
    ///
    /// # Errors
    /// The store's.
    pub fn set_browser_choice(&self, ctx: &AuditCtx, person: &str, on: bool) -> Result<bool> {
        self.set_family_choice(ctx, person, &BROWSER_CAPABILITIES, on)
    }

    /// Turns `person`'s grant of capability `cap` to their runs on or off,
    /// as `ctx`: off revokes it; on issues it under the admin grant that
    /// gives them `cap`, a capability they do not hold left as it is.
    /// Returns whether a grant changed.
    ///
    /// # Errors
    /// `unknown_capability`; the store's.
    pub fn set_cap_choice(&self, ctx: &AuditCtx, person: &str, cap: &str, on: bool) -> Result<bool> {
        let Some(cap) = crate::bot_grants::CAPABILITIES.iter().find(|c| **c == cap) else {
            return Err(clax_core::CoreError::invalid(
                "unknown_capability",
                format!("no capability is named {cap}"),
            ));
        };
        self.set_family_choice(ctx, person, &[*cap], on)
    }

    fn family_reaches_runs(&self, person: &str, caps: &[&str]) -> Result<bool> {
        let held = self.person_caps(person)?;
        Ok(caps.iter().all(|c| held[*c].reaches_runs()))
    }

    /// Off revokes each live grant of `caps` to `person`'s runs; on issues
    /// one, under the admin grant that gives them the capability, for each
    /// they hold and have not granted to their runs. A capability they do
    /// not hold is left as it is. Returns whether any grant changed.
    fn set_family_choice(
        &self,
        ctx: &AuditCtx,
        person: &str,
        caps: &[&str],
        on: bool,
    ) -> Result<bool> {
        let held = self.person_caps(person)?;
        let runs = Grantee::RunsOf(person.to_string()).as_text();
        let mut changed = false;
        for cap in caps {
            let h = &held[*cap];
            match (on, &h.granted_by, &h.runs_grant) {
                (false, _, Some(g)) => {
                    changed |= !self.revoke_cap_grant(ctx, g)?.is_empty();
                }
                (true, Some(parent), None) => {
                    self.issue_cap_grant(ctx, cap, person, &runs, Some(parent))?;
                    changed = true;
                }
                _ => {}
            }
        }
        Ok(changed)
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
    ///   the entry's place, applies `s.session` to its session's row (a
    ///   replayed call touches no session), journals the entry, and
    ///   records `s.audit` with
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
    /// `session_mismatch` when `s.session` does not fit its session's row
    /// ([`Spend::session`]: a closed or missing session, a point before
    /// its charged one, a usage that is not the segment);
    /// the charge's (`unknown_cost_line`, `invalid_usage`); the store's.
    /// A refused Spend writes nothing.
    pub fn spend_journaled(&self, run: &Run, driver: &DriverId, s: Spend) -> Result<Spent> {
        let at = self.now_utc();
        let ov = self.bot_overrides();
        let Spend {
            entry,
            usage,
            page,
            session,
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
            let now = clax_core::time::stamp(at);
            if let Some(session) = &session {
                apply_session(tx, &run.id, session, usage.as_ref(), &now)?;
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

    /// Session `session` of run `run_id`, open or closed, or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_browser_session(&self, run_id: &str, session: &str) -> Result<Option<SessionRow>> {
        self.with_read(|c| session_in(c, run_id, session))
    }

    /// Every browser session of run `run_id`, in the order they opened
    /// (by `opened_at`, then ID).
    ///
    /// # Errors
    /// The store's.
    pub fn bot_browser_sessions(&self, run_id: &str) -> Result<Vec<SessionRow>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {SESSION_COLS} FROM bot_browser_sessions
                  WHERE run_id = ?1 ORDER BY opened_at, session_id"
            ))?
            .query_map(params![run_id], row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Ends session `session` of run `run_id` for `reason`, an end the host
    /// makes rather than a call (spec 2026-11-19-claxbot-browser-design
    /// §10.3: idle, lifetime, revoke, credits, the run's end), its browser
    /// closed `at_ms` milliseconds after it opened: charges the last
    /// segment, from its charged point to `at_ms` (nothing when `at_ms` is
    /// not past it), as [`LINE_BROWSER_TIME`] to the run's person on the
    /// run, on its open step if one is open and else on none, adds it to
    /// the run's `credits_milli`, records `bot.spend` with `op:
    /// "browser.session.end"`, its capability, session, kind, reason and
    /// `ms`, and closes the row, in one transaction. Only while the session
    /// is open: a session closed already, or one the run never had, is
    /// left as it is and charged nothing. Answers the credits charged.
    ///
    /// # Errors
    /// The charge's (`unknown_cost_line`); the store's. A refused end
    /// writes nothing.
    pub fn end_bot_browser_session(
        &self,
        run_id: &str,
        session: &str,
        at_ms: i64,
        reason: SessionEnd,
    ) -> Result<i64> {
        let at = self.now_utc();
        let ov = self.bot_overrides();
        self.with_tx(|tx| {
            let Some(row) = session_in(tx, run_id, session)?.filter(SessionRow::is_open) else {
                return Ok(0);
            };
            let run = run_in(tx, run_id)?.ok_or(CoreError::NotFound)?;
            let step = open_step_in(tx, run_id)?.map(|s| s.n);
            let through = at_ms.max(row.charged_ms);
            let segment = through - row.charged_ms;
            let rows = charge_rows_in(
                tx,
                &ov,
                &run.person,
                Some(run_id),
                step,
                &[Metered::new(LINE_BROWSER_TIME, segment, run.key_source)],
                at,
            )?;
            let credits: i64 = rows.iter().map(|u| u.credits_milli).sum();
            let now = clax_core::time::stamp(at);
            if credits != 0 {
                tx.execute(
                    "UPDATE bot_runs SET credits_milli = credits_milli + ?2, updated_at = ?3
                      WHERE id = ?1",
                    params![run_id, credits, now],
                )?;
            }
            close_session_in(tx, run_id, session, through, reason, &now)?;
            let mut rec = AuditRecord::new(clax_core::audit::AuditKind::BotSpend, self.now())
                .with("capability", row.kind.capability())
                .with("op", "browser.session.end")
                .with("ok", true)
                .with("session", session)
                .with("kind", row.kind.as_str())
                .with("reason", reason.as_str())
                .with("ms", through)
                .with("run", run_id)
                .with("step", step)
                .with("credits_milli", credits);
            rec.ids.artifact = Some(run.artifact_id.clone());
            let ctx = AuditCtx {
                actor: Actor::Bot {
                    on_behalf_of: run.person.clone(),
                },
                ..AuditCtx::DAEMON
            };
            self.record_audit(tx, &ctx, rec)?;
            Ok(credits)
        })
    }

    /// Marks every browser session the store holds open as lost, closed
    /// at `at`, charging nothing (spec 2026-11-19-claxbot-browser-design
    /// §6.4: a daemon starting holds no session, and the time after a
    /// session's last charged point is the instance's loss). Answers how
    /// many it marked.
    ///
    /// # Errors
    /// The store's.
    pub fn lose_open_bot_browser_sessions(&self, at: chrono::DateTime<chrono::Utc>) -> Result<u32> {
        let at = clax_core::time::stamp(at);
        self.with_write(|c| {
            let n = c.execute(
                "UPDATE bot_browser_sessions SET closed_at = ?1, reason = ?2
                  WHERE closed_at IS NULL",
                params![at, SessionEnd::Lost.as_str()],
            )?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
    }

    /// The page run `run_id` keeps of `url` (normalised) in `format`
    /// (`markdown`, `html`, `raw`), or `None`.
    ///
    /// # Errors
    /// The store's.
    pub fn web_page(&self, run_id: &str, url: &str, format: &str) -> Result<Option<WebPageRow>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(&format!(
                "SELECT {PAGE_COLS} FROM bot_web_pages
                  WHERE run_id = ?1 AND url = ?2 AND format = ?3"
            ))?
            .query_row(params![run_id, url, format], row_to_page)
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
    /// entry marks `counted`, by op ([`Store::bot_spend_counts`] of `web`).
    ///
    /// # Errors
    /// The store's.
    pub fn bot_web_counts(&self, id: &str) -> Result<BTreeMap<String, u32>> {
        self.bot_spend_counts(id, "web")
    }

    /// How many of run `id`'s journaled calls of the operation family
    /// `namespace` (`web`, `browser`: the ops `<namespace>.<name>`) each
    /// op made that its entry marks `counted` (a result whose `counted` is
    /// true: the call reached its source), by op, or by the entry's
    /// `count_as` when it has one (a browser session's open and a test
    /// are counted under the capability of their source kind,
    /// `browser.open.url`); a key with none is absent. Counted in the
    /// database, without reading the entries' results out. `namespace` is
    /// one of the crate's callers' names, never a script's: it holds no
    /// `%` or `_`.
    ///
    /// # Errors
    /// The store's.
    pub fn bot_spend_counts(&self, id: &str, namespace: &str) -> Result<BTreeMap<String, u32>> {
        debug_assert!(!namespace.contains(['%', '_']), "{namespace}");
        self.with_read(|c| {
            let mut q = c.prepare_cached(
                "SELECT COALESCE(json_extract(result_json, '$.count_as'), op), COUNT(*)
                   FROM bot_calls
                  WHERE run_id = ?1 AND op LIKE ?2 || '.%'
                    AND json_extract(result_json, '$.counted') = 1
                  GROUP BY 1",
            )?;
            let rows = q.query_map(params![id, namespace], |r| {
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
    use crate::credits::{KeySource, LINE_BROWSER_TIME, LINE_HARNESS_FEE, LINE_WEB_FETCH, Metered};
    use crate::roles::OWNER;
    use crate::test_util::{DAEMON, artifact, store};
    use clax_core::CoreError;
    use clax_core::audit::Actor;
    use clax_core::audit::AuditKind;
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
                    caps: "web.fetch,web.search".into(),
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
            format: "markdown".into(),
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
            session: None,
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

        assert_eq!(st.web_page(&run.id, url, "markdown").unwrap(), Some(page(url)));
        assert_eq!(st.web_page(&run.id, "https://example.com/b", "markdown").unwrap(), None);
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
    fn spend_counts_are_per_namespace() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let journal = |seq: i64, op: &str, result: serde_json::Value| {
            let e = st
                .call_entry(&run.id, &d, 1, 1, seq, op, &format!("h-{seq}"))
                .unwrap()
                .answered(false, result);
            st.record_bot_call(&e).unwrap();
        };
        assert!(st.bot_spend_counts(&run.id, "browser").unwrap().is_empty());
        journal(0, "web.fetch", json!({"value": {}, "counted": true}));
        journal(1, "browser.test", json!({"value": {}, "counted": true}));
        journal(
            2,
            "browser.test",
            json!({"code": "browser_unavailable", "counted": true}),
        );
        journal(3, "browser.open", json!({"code": "browser_cap"}));
        journal(4, "browserx.test", json!({"value": {}, "counted": true}));
        // An entry with `count_as` is counted under it, not its op.
        journal(
            5,
            "browser.open",
            json!({"value": {}, "counted": true, "count_as": "browser.open.url"}),
        );
        journal(
            6,
            "browser.test",
            json!({"value": {}, "counted": true, "count_as": "browser.open.artifact"}),
        );
        let pairs = |m: BTreeMap<String, u32>| m.into_iter().collect::<Vec<_>>();
        assert_eq!(
            pairs(st.bot_spend_counts(&run.id, "browser").unwrap()),
            vec![
                ("browser.open.artifact".to_string(), 1),
                ("browser.open.url".to_string(), 1),
                ("browser.test".to_string(), 2)
            ]
        );
        assert_eq!(
            pairs(st.bot_spend_counts(&run.id, "web").unwrap()),
            vec![("web.fetch".to_string(), 1)]
        );
        assert_eq!(
            st.bot_web_counts(&run.id).unwrap(),
            st.bot_spend_counts(&run.id, "web").unwrap()
        );
        assert!(st.bot_spend_counts(&run.id, "files").unwrap().is_empty());
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
            session: None,
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
        assert_eq!(st.web_page(&run.id, url, "markdown").unwrap(), Some(page(url)));
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

    fn everyone(st: &Store, caps: &[&str]) {
        for cap in caps {
            st.issue_cap_grant(
                DAEMON,
                cap,
                crate::bot_grants::GRANTER_INSTANCE,
                crate::bot_grants::EVERYONE,
                None,
            )
            .unwrap();
        }
    }

    fn grant_ops(st: &Store) -> Vec<(String, String)> {
        st.events_after(0, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "bot.grant")
            .map(|e| {
                let b: serde_json::Value = serde_json::from_str(&e.body).unwrap();
                (
                    b["op"].as_str().unwrap().to_string(),
                    b["capability"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_person_chooses_web_off_and_back() {
        let (_d, st) = store();
        assert!(!st.web_choice(OWNER).unwrap(), "no grant, no web");
        assert!(
            !st.set_web_choice(DAEMON, OWNER, true).unwrap(),
            "nothing held to turn on"
        );
        everyone(&st, &["web.search"]);
        assert!(!st.web_choice(OWNER).unwrap(), "fetch is not granted");
        everyone(&st, &["web.fetch"]);
        assert!(st.web_choice(OWNER).unwrap());
        let before = grant_ops(&st).len();
        assert!(!st.set_web_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.set_web_choice(DAEMON, OWNER, false).unwrap());
        assert!(!st.web_choice(OWNER).unwrap());
        assert!(!st.set_web_choice(DAEMON, OWNER, false).unwrap());
        assert_eq!(
            grant_ops(&st)[before..],
            [
                ("revoke".to_string(), "web.search".to_string()),
                ("revoke".to_string(), "web.fetch".to_string())
            ],
            "each runs grant revoked, recorded once"
        );
        assert!(st.set_web_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.web_choice(OWNER).unwrap());
        assert!(!st.web_choice("someone-else").unwrap(), "not a person");
    }

    #[test]
    fn a_person_turns_the_browser_off_and_back() {
        let (_d, st) = store();
        everyone(&st, &crate::bot_grants::CAPABILITIES);
        assert!(st.browser_choice(OWNER).unwrap());
        assert!(!st.set_browser_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.set_browser_choice(DAEMON, OWNER, false).unwrap());
        assert!(!st.browser_choice(OWNER).unwrap());
        assert!(st.web_choice(OWNER).unwrap(), "the web choice is its own");
        assert!(!st.set_browser_choice(DAEMON, OWNER, false).unwrap());
        assert!(st.set_web_choice(DAEMON, OWNER, false).unwrap());
        assert!(
            !st.browser_choice(OWNER).unwrap(),
            "the web choice keeps it"
        );
        assert!(st.set_browser_choice(DAEMON, OWNER, true).unwrap());
        assert!(st.browser_choice(OWNER).unwrap());
        assert!(
            !st.web_choice(OWNER).unwrap(),
            "the browser choice keeps it"
        );
    }

    /// A browser session call of `run` at `(step, seq)`: `s` with its
    /// segment, `qty` milliseconds of browser time.
    fn session_spend(
        st: &Store,
        run: &Run,
        d: &DriverId,
        (step, seq): (i64, i64),
        qty: i64,
        s: SessionSpend,
    ) -> Spend {
        let entry = st
            .call_entry(&run.id, d, step, 1, seq, "browser.page", &format!("h-{step}-{seq}"))
            .unwrap()
            .answered(true, json!({"value": {"ok": true}}));
        Spend {
            entry,
            usage: Some(Metered::new(LINE_BROWSER_TIME, qty, KeySource::Instance)),
            page: None,
            session: Some(s),
            audit: AuditRecord::new(AuditKind::BotSpend, st.now())
                .with("capability", "browser.open.artifact")
                .with("op", "browser.page"),
        }
    }

    fn open(st: &Store, through_ms: i64) -> SessionSpend {
        SessionSpend::Open {
            session: "s1".into(),
            kind: SessionKind::Artifact,
            opened_at: st.now_utc(),
            through_ms,
        }
    }

    fn call(through_ms: i64) -> SessionSpend {
        SessionSpend::Call {
            session: "s1".into(),
            through_ms,
            kind: None,
        }
    }

    /// The milliseconds of browser time charged to `run`, and the rows.
    fn browser_ms(st: &Store, run: &Run) -> (i64, usize) {
        let rows: Vec<_> = st
            .usage_of_run(&run.id)
            .unwrap()
            .into_iter()
            .filter(|u| u.line == LINE_BROWSER_TIME)
            .collect();
        (rows.iter().map(|u| u.qty).sum(), rows.len())
    }

    fn spend_events(st: &Store) -> Vec<serde_json::Value> {
        st.events_after(0, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "bot.spend")
            .map(|e| serde_json::from_str(&e.body).unwrap())
            .collect()
    }

    #[test]
    fn segments_of_a_session_sum_to_its_open_time() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 1_200, open(&st, 1_200)))
            .unwrap();
        let row = st.bot_browser_session(&run.id, "s1").unwrap().unwrap();
        assert_eq!(
            (row.kind, row.charged_ms, row.is_open()),
            (SessionKind::Artifact, 1_200, true)
        );
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 1), 3_800, call(5_000)))
            .unwrap();
        // A call that took no time charges an empty segment.
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 2), 0, call(5_000)))
            .unwrap();
        // A goto out makes it a URL session.
        let out = SessionSpend::Call {
            session: "s1".into(),
            through_ms: 7_000,
            kind: Some(SessionKind::Url),
        };
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 3), 2_000, out))
            .unwrap();
        assert_eq!(
            st.bot_browser_session(&run.id, "s1").unwrap().unwrap().kind,
            SessionKind::Url
        );
        let close = SessionSpend::Close {
            session: "s1".into(),
            through_ms: 9_000,
            reason: SessionEnd::Closed,
        };
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 4), 2_000, close))
            .unwrap();
        assert_eq!(browser_ms(&st, &run), (9_000, 5));
        let row = st.bot_browser_session(&run.id, "s1").unwrap().unwrap();
        assert_eq!(
            (row.charged_ms, row.is_open(), row.reason),
            (9_000, false, Some(SessionEnd::Closed))
        );
        let charged: i64 = st
            .usage_of_run(&run.id)
            .unwrap()
            .iter()
            .map(|u| u.credits_milli)
            .sum();
        assert!(charged > 0);
        assert_eq!(st.run(&run.id).unwrap().unwrap().credits_milli, charged);
        // The host's end of a closed session charges nothing.
        assert_eq!(
            st.end_bot_browser_session(&run.id, "s1", 20_000, SessionEnd::Idle).unwrap(),
            0
        );
        assert_eq!(browser_ms(&st, &run), (9_000, 5));
    }

    #[test]
    fn a_replayed_call_charges_nothing() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 1_000, open(&st, 1_000)))
            .unwrap();
        let first = st
            .spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 1), 2_000, call(3_000)))
            .unwrap();
        // The same call again, as a step run again makes it: the journal
        // answers, though its segment no longer fits the session's row.
        let again = st
            .spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 1), 2_000, call(3_000)))
            .unwrap();
        assert!(again.replayed);
        assert_eq!((again.entry, again.credits_milli), (first.entry, 0));
        assert_eq!(browser_ms(&st, &run), (3_000, 2));
        assert_eq!(st.bot_browser_session(&run.id, "s1").unwrap().unwrap().charged_ms, 3_000);
        assert_eq!(spend_events(&st).len(), 2);
    }

    #[test]
    fn a_host_end_with_no_open_step_charges_once() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 1_500, open(&st, 1_500)))
            .unwrap();
        st.commit_bot_step(&bot(), &run.id, &d, 1, StepCommit::default())
            .unwrap();
        let before = st.run(&run.id).unwrap().unwrap().credits_milli;
        let credits = st
            .end_bot_browser_session(&run.id, "s1", 301_500, SessionEnd::Idle)
            .unwrap();
        assert!(credits > 0);
        let tail: Vec<_> = st
            .usage_of_run(&run.id)
            .unwrap()
            .into_iter()
            .filter(|u| u.line == LINE_BROWSER_TIME && u.qty == 300_000)
            .collect();
        assert_eq!(tail.len(), 1);
        assert_eq!(
            (tail[0].run_id.as_deref(), tail[0].step_n, tail[0].credits_milli),
            (Some(run.id.as_str()), None, credits)
        );
        assert_eq!(st.run(&run.id).unwrap().unwrap().credits_milli, before + credits);
        let row = st.bot_browser_session(&run.id, "s1").unwrap().unwrap();
        assert_eq!(
            (row.charged_ms, row.reason, row.is_open()),
            (301_500, Some(SessionEnd::Idle), false)
        );
        let ends: Vec<_> = spend_events(&st)
            .into_iter()
            .filter(|b| b["op"] == "browser.session.end")
            .collect();
        assert_eq!(ends.len(), 1);
        let e = &ends[0];
        assert_eq!(
            (&e["capability"], &e["reason"], &e["session"], &e["kind"]),
            (
                &json!("browser.open.artifact"),
                &json!("idle"),
                &json!("s1"),
                &json!("artifact")
            )
        );
        assert_eq!((&e["step"], &e["ms"], &e["run"]), (&json!(null), &json!(301_500), &json!(run.id)));
        assert_eq!(e["credits_milli"], credits);

        // An end while a step is open charges on that step.
        st.begin_step(&run.id, &d, 2, StepKind::Script).unwrap();
        let s2 = SessionSpend::Open {
            session: "s2".into(),
            kind: SessionKind::Content,
            opened_at: st.now_utc(),
            through_ms: 10,
        };
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (2, 0), 10, s2))
            .unwrap();
        st.end_bot_browser_session(&run.id, "s2", 1_010, SessionEnd::RunEnded)
            .unwrap();
        let on_step: Vec<_> = st
            .usage_of_run(&run.id)
            .unwrap()
            .into_iter()
            .filter(|u| u.qty == 1_000)
            .collect();
        assert_eq!(on_step.len(), 1);
        assert_eq!(on_step[0].step_n, Some(2));
    }

    #[test]
    fn a_second_end_charges_nothing() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 100, open(&st, 100)))
            .unwrap();
        let first = st.end_bot_browser_session(&run.id, "s1", 60_100, SessionEnd::Revoked).unwrap();
        assert!(first > 0);
        assert_eq!(st.end_bot_browser_session(&run.id, "s1", 90_000, SessionEnd::Idle).unwrap(), 0);
        assert_eq!(
            st.end_bot_browser_session(&run.id, "no-such", 90_000, SessionEnd::Idle).unwrap(),
            0
        );
        assert_eq!(browser_ms(&st, &run), (60_100, 2));
        assert_eq!(
            st.bot_browser_session(&run.id, "s1").unwrap().unwrap().reason,
            Some(SessionEnd::Revoked)
        );
        let ends = spend_events(&st)
            .into_iter()
            .filter(|b| b["op"] == "browser.session.end")
            .count();
        assert_eq!(ends, 1);
        // A call of the closed session is refused, writing nothing.
        let late = session_spend(&st, &run, &d, (1, 1), 1, call(60_101));
        assert_eq!(refused(&st, &run, &d, late), "session_mismatch");
    }

    #[test]
    fn a_point_before_the_charged_one_is_refused() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 5_000, open(&st, 5_000)))
            .unwrap();
        let before = browser_ms(&st, &run);
        let back = session_spend(&st, &run, &d, (1, 1), 0, call(4_000));
        assert_eq!(refused(&st, &run, &d, back), "session_mismatch");
        // A usage that is not the segment.
        let wrong = session_spend(&st, &run, &d, (1, 1), 999, call(6_000));
        assert_eq!(refused(&st, &run, &d, wrong), "session_mismatch");
        // A session the run does not have, and an open of one it has.
        let missing = SessionSpend::Call {
            session: "s9".into(),
            through_ms: 10,
            kind: None,
        };
        let missing = session_spend(&st, &run, &d, (1, 1), 10, missing);
        assert_eq!(refused(&st, &run, &d, missing), "session_mismatch");
        let twice = session_spend(&st, &run, &d, (1, 1), 6_000, open(&st, 6_000));
        assert_eq!(refused(&st, &run, &d, twice), "session_mismatch");
        assert_eq!(browser_ms(&st, &run), before);
        assert_eq!(st.bot_browser_session(&run.id, "s1").unwrap().unwrap().charged_ms, 5_000);
    }

    #[test]
    fn a_start_marks_open_sessions_lost() {
        let (_d, st) = store();
        let (run, d) = running(&st);
        let (other, d2) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 10, open(&st, 10)))
            .unwrap();
        let closed = SessionSpend::Open {
            session: "s2".into(),
            kind: SessionKind::Url,
            opened_at: st.now_utc(),
            through_ms: 10,
        };
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 1), 10, closed))
            .unwrap();
        st.end_bot_browser_session(&run.id, "s2", 20, SessionEnd::Closed).unwrap();
        st.spend_journaled(&other, &d2, session_spend(&st, &other, &d2, (1, 0), 10, open(&st, 10)))
            .unwrap();
        let usage = st.usage_of_run(&run.id).unwrap().len() + st.usage_of_run(&other.id).unwrap().len();
        assert_eq!(st.lose_open_bot_browser_sessions(st.now_utc()).unwrap(), 2);
        for r in [&run, &other] {
            let row = st.bot_browser_session(&r.id, "s1").unwrap().unwrap();
            assert_eq!((row.reason, row.charged_ms), (Some(SessionEnd::Lost), 10));
            assert!(!row.is_open());
        }
        assert_eq!(
            st.bot_browser_session(&run.id, "s2").unwrap().unwrap().reason,
            Some(SessionEnd::Closed)
        );
        assert_eq!(
            st.usage_of_run(&run.id).unwrap().len() + st.usage_of_run(&other.id).unwrap().len(),
            usage,
            "a lost session charges nothing"
        );
        assert_eq!(st.lose_open_bot_browser_sessions(st.now_utc()).unwrap(), 0);
        let listed: Vec<_> = st
            .bot_browser_sessions(&run.id)
            .unwrap()
            .into_iter()
            .map(|r| (r.session_id, r.reason))
            .collect();
        assert_eq!(
            listed,
            [("s1".to_string(), Some(SessionEnd::Lost)), ("s2".to_string(), Some(SessionEnd::Closed))]
        );
    }

    #[test]
    fn session_rows_go_with_the_journal_of_an_ended_run() {
        let clock =
            std::sync::Arc::new(clax_core::working::ManualClock::at("2026-11-19T10:00:00Z"));
        let (_d, st) = crate::test_util::store_with_clock(clock.clone());
        let (run, d) = running(&st);
        st.spend_journaled(&run, &d, session_spend(&st, &run, &d, (1, 0), 10, open(&st, 10)))
            .unwrap();
        st.commit_bot_step(&bot(), &run.id, &d, 1, StepCommit::default())
            .unwrap();
        st.begin_step(&run.id, &d, 2, StepKind::GiveUp).unwrap();
        st.commit_bot_step(&bot(), &run.id, &d, 2, StepCommit::default())
            .unwrap();
        clock.advance(crate::bot_runs::BOT_JOURNAL_KEEP.as_secs() as i64 + 1);
        assert_eq!(st.prune_ended_runs().unwrap().browser_sessions, 1);
        assert_eq!(st.bot_browser_session(&run.id, "s1").unwrap(), None);
    }

}
