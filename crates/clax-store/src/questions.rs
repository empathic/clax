//! Agent questions (spec 2026-10-06-agent-questions-and-inbox-design §5.1):
//! rows, their one-way transitions (each in one write transaction, so of two
//! racing changes the first wins and the other is `question_closed`), the
//! open limits, and delivery bookkeeping.
//!
//! Each transition records its `question.*` audit event in its own
//! transaction (audit spec §6.6), built by the builders here, which the
//! backfill shares.

use super::Store;
use clax_core::audit::{AuditCtx, AuditIds, AuditKind, AuditRecord, SystemReason};
use clax_core::questions::{Answer, Question, validate_answers};
use clax_core::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use serde_json::Value;

/// Open questions one session may have at once.
pub const MAX_OPEN_PER_SESSION: u32 = 8;
/// Open questions of one person's agents (the owner's, or one connected
/// person's): each person's agents share their own cap.
pub const MAX_OPEN: u32 = 100;

/// Where a question is in its life. `Open` moves to any other status;
/// `Released` (handed to the terminal) moves only to `Answered`; the rest
/// are final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Answered,
    Declined,
    Released,
    Withdrawn,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Answered => "answered",
            Status::Declined => "declined",
            Status::Released => "released",
            Status::Withdrawn => "withdrawn",
        }
    }

    fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "open" => Status::Open,
            "answered" => Status::Answered,
            "declined" => Status::Declined,
            "released" => Status::Released,
            "withdrawn" => Status::Withdrawn,
            _ => return None,
        })
    }
}

/// How a question arrived: the `ask` tool, or Claude Code's
/// AskUserQuestion mirrored by the hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Ask,
    Hook,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Ask => "ask",
            Source::Hook => "hook",
        }
    }
}

/// One stored question set.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestionRow {
    pub id: String,
    pub session_id: String,
    pub artifact_id: Option<String>,
    pub source: Source,
    /// The mirrored AskUserQuestion call's ID (`hook` questions).
    pub tool_use_id: Option<String>,
    pub questions: Vec<Question>,
    pub status: Status,
    /// One per question, when answered.
    pub answers: Option<Vec<Answer>>,
    /// Where the answer came from: `shell`, `extension`, `cli` or `terminal`.
    pub answered_via: Option<String>,
    pub created_at: String,
    pub closed_at: Option<String>,
    /// When the asking session received the outcome.
    pub taken_at: Option<String>,
    /// The thread of `artifact_id` it was asked in, shown in that thread's card.
    pub thread_id: Option<String>,
    /// An in-terminal question: says a question waits in the terminal or in
    /// chat, and takes no answer in Clax.
    pub in_terminal: bool,
    /// When the person asked for the in-terminal question in Clax instead.
    pub pulled_at: Option<String>,
}

/// A question to record. The caller has validated `questions`.
pub struct NewQuestion {
    pub session_id: String,
    pub artifact_id: Option<String>,
    pub source: Source,
    pub tool_use_id: Option<String>,
    pub questions: Vec<Question>,
    /// Created already moved to the terminal (the hook's `terminal` mode).
    pub released: bool,
    /// A thread of `artifact_id`; it must be one (`invalid_thread`).
    pub thread_id: Option<String>,
    /// Record an in-terminal question ([`QuestionRow::in_terminal`]); never
    /// created released.
    pub in_terminal: bool,
}

/// A transition out of `Open` (or, for `Terminal`, out of `Released`, or
/// out of `Open` for a hook in-terminal question). An in-terminal question
/// takes no `Answer`, `Decline`, `Release` or `Expire`.
pub enum Close {
    /// Answered in Clax; `via` is `shell`, `extension` or `cli`. The
    /// answers are checked against the questions
    /// ([`validate_answers`](clax_core::questions::validate_answers)) and stored
    /// trimmed.
    Answer {
        answers: Vec<Answer>,
        via: &'static str,
    },
    Decline,
    /// Handed to the terminal by the owner ("Answer in the terminal"); only
    /// for `hook` questions.
    Release,
    /// Handed to the terminal by the hook's timer, the owner not having
    /// acted; only for `hook` questions. Its inbox item stays unread.
    Expire,
    Withdraw,
    /// Withdrawn by the daemon: a mirrored question no poll held for the
    /// grace. Its inbox item stays unread.
    Unwaited,
    /// Withdrawn by the daemon as it shuts down: a mirrored question a poll
    /// held. Its inbox item stays unread.
    Shutdown,
    /// A released question, or an open hook in-terminal question, answered
    /// in the terminal, stored as Claude Code recorded it (it may leave a
    /// question unanswered).
    Terminal {
        answers: Vec<Answer>,
    },
    /// "Ask me here" on an in-terminal question: it is withdrawn, and its
    /// session is told once, at its next delivery, that the person wants
    /// the question in Clax. Its inbox item is read.
    Pull,
    /// A hook in-terminal question whose terminal dialog took no answer:
    /// its person typed a new prompt to the session. Its inbox item is read.
    Lapse,
}

/// Why a question was released or withdrawn: the `reason` of its
/// `question.release` or `question.withdraw` event (audit spec §6.6), a
/// fixed phrase, so redaction classes it safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Released: the owner chose to answer in the terminal.
    Owner,
    /// Released: the hook's timer ran out before the owner acted.
    Timer,
    /// Released as it was asked: no owner surface was open, or the terminal
    /// was chosen up front.
    Created,
    /// Withdrawn by its session.
    Explicit,
    /// Withdrawn by the daemon: a mirrored question no poll held for the
    /// grace.
    Unwaited,
    /// Withdrawn because its session ended (explicitly or by the reaper).
    SessionEnd,
    /// Withdrawn at daemon start: its hook waited on the previous daemon.
    DaemonStart,
    /// Withdrawn as the daemon shut down: its hook was waiting on it.
    DaemonStop,
    /// Withdrawn: the person asked for the in-terminal question in Clax.
    Pulled,
    /// Withdrawn: the hook in-terminal question's dialog closed unanswered
    /// (a new prompt came).
    Lapsed,
    /// Withdrawn: the job that asked it was cancelled or done, so nothing
    /// reads its outcome.
    JobEnd,
}

impl Reason {
    /// Every reason.
    pub const ALL: [Reason; 11] = [
        Reason::Owner,
        Reason::Timer,
        Reason::Created,
        Reason::Explicit,
        Reason::Unwaited,
        Reason::SessionEnd,
        Reason::DaemonStart,
        Reason::DaemonStop,
        Reason::Pulled,
        Reason::Lapsed,
        Reason::JobEnd,
    ];

    /// The phrase recorded.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Owner => "owner",
            Reason::Timer => "timer",
            Reason::Created => "created",
            Reason::Explicit => "explicit",
            Reason::Unwaited => "unwaited",
            Reason::SessionEnd => "session_end",
            Reason::DaemonStart => "daemon_start",
            Reason::DaemonStop => "daemon_stop",
            Reason::Pulled => "pulled",
            Reason::Lapsed => "lapsed",
            Reason::JobEnd => "job_end",
        }
    }

    /// The reason recorded as `s`; `None` for any other phrase.
    pub fn parse(s: &str) -> Option<Reason> {
        Reason::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// The kind of event this reason belongs to: `question.release` or
    /// `question.withdraw`.
    pub fn kind(self) -> AuditKind {
        match self {
            Reason::Owner | Reason::Timer | Reason::Created => AuditKind::QuestionRelease,
            _ => AuditKind::QuestionWithdraw,
        }
    }
}

/// The IDs of question `qid`'s events: the question, its asking session and
/// its artifact.
fn question_ids(qid: &str, sid: &str, aid: Option<&str>) -> AuditIds {
    AuditIds {
        question: Some(qid.to_string()),
        session: Some(sid.to_string()),
        artifact: aid.map(str::to_string),
        ..AuditIds::default()
    }
}

/// A `question.ask` event (audit spec §6.6): how it arrived, the mirrored
/// call's ID, and the questions as stored.
pub(crate) fn ask_record(
    at: &str,
    qid: &str,
    sid: &str,
    aid: Option<&str>,
    source: &str,
    tool_use_id: Option<&str>,
    questions: Value,
) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::QuestionAsk, at)
        .with("source", source)
        .with("tool_use_id", tool_use_id)
        .with("questions", questions);
    rec.ids = question_ids(qid, sid, aid);
    rec
}

/// A `question.answer` event: the answers as stored and where they came
/// from.
pub(crate) fn answer_record(
    at: &str,
    qid: &str,
    sid: &str,
    aid: Option<&str>,
    answers: Value,
    answered_via: Option<&str>,
) -> AuditRecord {
    let mut rec = AuditRecord::new(AuditKind::QuestionAnswer, at)
        .with("answers", answers)
        .with("answered_via", answered_via);
    rec.ids = question_ids(qid, sid, aid);
    rec
}

/// A `question.decline` (no body), `question.release` or
/// `question.withdraw` event; the latter two carry `reason`, `null` when
/// the history does not say.
///
/// # Errors
/// `invalid_reason` for another kind, a decline with a reason, or a
/// reason of the other kind (a release's reason on a withdrawal, say).
pub(crate) fn close_record(
    kind: AuditKind,
    at: &str,
    qid: &str,
    sid: &str,
    aid: Option<&str>,
    why: Option<Reason>,
) -> Result<AuditRecord> {
    let fits = match (kind, why) {
        (AuditKind::QuestionDecline, None) => true,
        (AuditKind::QuestionRelease | AuditKind::QuestionWithdraw, None) => true,
        (AuditKind::QuestionRelease | AuditKind::QuestionWithdraw, Some(r)) => r.kind() == kind,
        _ => false,
    };
    if !fits {
        return Err(CoreError::invalid(
            "invalid_reason",
            "a question's close records a reason of its own kind",
        ));
    }
    let mut rec = AuditRecord::new(kind, at);
    if kind != AuditKind::QuestionDecline {
        rec = rec.with("reason", why.map(Reason::as_str));
    }
    rec.ids = question_ids(qid, sid, aid);
    Ok(rec)
}

/// `text`, stored JSON, as a value; `null` when absent or when it does not
/// parse.
pub(super) fn json_or_null(text: Option<&str>) -> Value {
    text.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

/// Which questions [`Store::list_questions`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStatus {
    Open,
    Closed,
    All,
}

/// Where to continue a listing of closed questions: the last row shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedCursor {
    pub closed_at: String,
    pub id: String,
}

impl ClosedCursor {
    /// The cursor after `q`, when `q` is closed.
    pub fn after(q: &QuestionRow) -> Option<ClosedCursor> {
        Some(ClosedCursor {
            closed_at: q.closed_at.clone()?,
            id: q.id.clone(),
        })
    }
}

macro_rules! select {
    () => {
        "SELECT id, session_id, artifact_id, source, tool_use_id, questions_json, status,
            answers_json, answered_via, created_at, closed_at, taken_at, thread_id, in_terminal,
            pulled_at FROM questions "
    };
}

/// The late answers' filter: session `?1`'s outcomes not yet received:
/// its answered and declined `ask` questions, and its pulled in-terminal
/// questions.
macro_rules! late {
    () => {
        "WHERE session_id = ?1 AND taken_at IS NULL AND ((status IN ('answered', 'declined')
            AND source = 'ask' AND in_terminal = 0) OR pulled_at IS NOT NULL)"
    };
}

pub(crate) const BY_ID: &str = concat!(select!(), "WHERE id = ?1");
pub(crate) const BY_TOOL_USE: &str =
    concat!(select!(), "WHERE session_id = ?1 AND tool_use_id = ?2");
/// A person's lists hold the questions of their own agents only (spec
/// §7.5, §18: person-scoped): the recipient `?1` (`questions.person`,
/// the asking session's person: empty for the owner, else a public ID),
/// walked through `questions_open_by_person` or `questions_closed_by_person`,
/// whose conditions each query repeats word for word.
macro_rules! of_person {
    () => {
        " AND COALESCE(person, '') = ?1"
    };
}
/// Recipient `?1`'s open questions: what [`MAX_OPEN`] caps.
pub(crate) const OPEN_COUNT: &str = concat!(
    "SELECT COUNT(*) FROM questions INDEXED BY questions_open_by_person WHERE status = 'open'",
    of_person!()
);
pub(crate) const OPEN_COUNT_OF_SESSION: &str =
    "SELECT COUNT(*) FROM questions WHERE session_id = ?1 AND status = 'open'";
pub(crate) const OPEN_OLDEST: &str = concat!(
    select!(),
    "INDEXED BY questions_open_by_person WHERE status = 'open'",
    of_person!(),
    " ORDER BY created_at, id LIMIT ?2"
);
pub(crate) const OPEN_NEWEST: &str = concat!(
    select!(),
    "INDEXED BY questions_open_by_person WHERE status = 'open'",
    of_person!(),
    " ORDER BY created_at DESC, id DESC LIMIT ?2"
);
/// Closed questions use the partial index `questions_closed`, whose
/// condition (`status <> 'open'`) each query repeats word for word.
pub(crate) const CLOSED_NEWEST: &str = concat!(
    select!(),
    "INDEXED BY questions_closed_by_person WHERE status <> 'open'",
    of_person!(),
    " ORDER BY closed_at DESC, id DESC LIMIT ?2"
);
pub(crate) const CLOSED_BEFORE: &str = concat!(
    select!(),
    "INDEXED BY questions_closed_by_person WHERE status <> 'open' AND (closed_at, id) < (?2, ?3)",
    of_person!(),
    " ORDER BY closed_at DESC, id DESC LIMIT ?4"
);
pub(crate) const LATE_ANSWERS: &str = concat!(select!(), late!());
pub(crate) const OPEN_OF_SESSION: &str = "SELECT id, session_id, artifact_id FROM questions
    WHERE session_id = ?1 AND status = 'open'";
pub(crate) const OPEN_HOOKS: &str = "SELECT id, session_id, artifact_id FROM questions
    WHERE status = 'open' AND source = 'hook' AND in_terminal = 0";
pub(crate) const CLOSE: &str = "UPDATE questions SET status = ?3, answers_json = ?4,
    answered_via = ?5, closed_at = ?6 WHERE id = ?1 AND status = ?2";
pub(crate) const TAKE: &str =
    "UPDATE questions SET taken_at = ?2 WHERE id = ?1 AND taken_at IS NULL";
/// Thread `?1`'s questions are about artifact `?2`, its new page.
pub(crate) const MOVE_THREAD: &str = "UPDATE questions SET artifact_id = ?2 WHERE thread_id = ?1";

/// A row this code could not have written. The message names the column,
/// never its content, so no question text reaches an error or a log.
fn corrupt(column: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("questions.{column} is not a value this store writes").into(),
    )
}

fn to_json<T: Serialize + ?Sized>(v: &T) -> Result<String> {
    serde_json::to_string(v)
        .map_err(|e| CoreError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))
}

fn row(r: &Row<'_>) -> rusqlite::Result<QuestionRow> {
    let qs: String = r.get("questions_json")?;
    let ans: Option<String> = r.get("answers_json")?;
    let source = match r.get_ref("source")?.as_str()? {
        "ask" => Source::Ask,
        "hook" => Source::Hook,
        _ => return Err(corrupt("source")),
    };
    Ok(QuestionRow {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        artifact_id: r.get("artifact_id")?,
        source,
        tool_use_id: r.get("tool_use_id")?,
        questions: serde_json::from_str(&qs).map_err(|_| corrupt("questions_json"))?,
        status: Status::parse(r.get_ref("status")?.as_str()?).ok_or_else(|| corrupt("status"))?,
        answers: ans
            .map(|a| serde_json::from_str(&a))
            .transpose()
            .map_err(|_| corrupt("answers_json"))?,
        answered_via: r.get("answered_via")?,
        created_at: r.get("created_at")?,
        closed_at: r.get("closed_at")?,
        taken_at: r.get("taken_at")?,
        thread_id: r.get("thread_id")?,
        in_terminal: r.get("in_terminal")?,
        pulled_at: r.get("pulled_at")?,
    })
}

fn rows(c: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<QuestionRow>> {
    let mut stmt = c.prepare_cached(sql)?;
    Ok(stmt
        .query_map(p, row)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn fetch(c: &Connection, id: &str) -> Result<Option<QuestionRow>> {
    Ok(c.query_row(BY_ID, params![id], row).optional()?)
}

/// The IDs of the open questions `sql` selects (its first column), in
/// order (ULIDs sort in creation order).
fn open_ids(c: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<String>> {
    let mut stmt = c.prepare_cached(sql)?;
    let mut out = stmt
        .query_map(p, |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    out.sort_unstable();
    Ok(out)
}

/// What a close makes of a question: its status after, the answers it
/// stores (their JSON text), how they came, and the reason its event
/// records.
struct Closing {
    status: Status,
    answers: Option<String>,
    via: Option<&'static str>,
    why: Option<Reason>,
}

impl Store {
    /// Withdraws session `sid`'s open questions inside the caller's
    /// transaction, its session having ended under `ctx` (the reaper's is
    /// `system:ttl`), each by [`Store::withdraw_question_in`] (reason
    /// `session_end`) at `now`; returns their IDs.
    pub(crate) fn withdraw_session_questions(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        sid: &str,
        now: &str,
    ) -> Result<Vec<String>> {
        let open = open_ids(tx, OPEN_OF_SESSION, params![sid])?;
        for id in &open {
            self.withdraw_at(tx, ctx, id, Reason::SessionEnd, now)?;
        }
        Ok(open)
    }

    /// Withdraws question `qid` for `why` inside the caller's transaction,
    /// under `ctx`, by [`Store::close_in`]; the question after it, or
    /// `None` when there is none or it is not open. Every withdrawal goes
    /// through it: a session's (`withdraw`, its end), the daemon's, and a
    /// final job's (`Store::ending`).
    pub(crate) fn withdraw_question_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        qid: &str,
        why: Reason,
    ) -> Result<Option<QuestionRow>> {
        self.withdraw_at(tx, ctx, qid, why, &self.now())
    }

    /// [`Store::withdraw_question_in`] at `now`.
    fn withdraw_at(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        qid: &str,
        why: Reason,
        now: &str,
    ) -> Result<Option<QuestionRow>> {
        let Some(q) = fetch(tx, qid)?.filter(|q| q.status == Status::Open) else {
            return Ok(None);
        };
        let to = Closing {
            status: Status::Withdrawn,
            answers: None,
            via: None,
            why: Some(why),
        };
        self.close_in(tx, ctx, &q, to, false, now).map(Some)
    }

    /// Closes question `q`, as read in `tx`, as `to` at `now` under `ctx`:
    /// the one write that closes a question. In `tx` it stores the close
    /// (and, for [`Reason::Pulled`], the time), stores the outcome in the
    /// job that asked it ([`Store::close_job_question_tx`]), records its
    /// event (`question.answer`, `question.decline`, or `question.release`
    /// / `question.withdraw` with `to.why`), and updates its inbox item,
    /// read when `seen` and the question was answered, declined or
    /// released, or when it is in the terminal.
    ///
    /// # Errors
    /// `question_closed` when `q`'s status moved since it was read.
    fn close_in(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        q: &QuestionRow,
        to: Closing,
        seen: bool,
        now: &str,
    ) -> Result<QuestionRow> {
        let qid = q.id.as_str();
        let n = tx.execute(
            CLOSE,
            params![
                qid,
                q.status.as_str(),
                to.status.as_str(),
                to.answers,
                to.via,
                now
            ],
        )?;
        if n != 1 {
            return Err(closed(q));
        }
        if to.why == Some(Reason::Pulled) {
            tx.execute(
                "UPDATE questions SET pulled_at = ?2 WHERE id = ?1",
                params![qid, now],
            )?;
        }
        let outcome = super::jobs::JobAnswer::of(to.status.as_str(), to.answers.clone())?;
        if outcome != super::jobs::JobAnswer::Pending {
            self.close_job_question_tx(tx, qid, &outcome)?;
        }
        let (sid, aid) = (&q.session_id, q.artifact_id.as_deref());
        let rec = match to.status {
            Status::Answered => answer_record(
                now,
                qid,
                sid,
                aid,
                json_or_null(to.answers.as_deref()),
                to.via,
            ),
            Status::Declined => close_record(AuditKind::QuestionDecline, now, qid, sid, aid, None)?,
            Status::Released => {
                close_record(AuditKind::QuestionRelease, now, qid, sid, aid, to.why)?
            }
            _ => close_record(AuditKind::QuestionWithdraw, now, qid, sid, aid, to.why)?,
        };
        self.record_question(tx, ctx, rec, sid)?;
        let q = fetch(tx, qid)?.ok_or(CoreError::NotFound)?;
        super::inbox::question_changed(tx, &q, seen, now)?;
        Ok(q)
    }

    /// Records `rec`, an event of a question of session `sid`, under `ctx`;
    /// a change Clax makes itself names the session's agent in `for_actor`.
    fn record_question(
        &self,
        tx: &Transaction<'_>,
        ctx: &AuditCtx,
        rec: AuditRecord,
        sid: &str,
    ) -> Result<()> {
        let rec =
            super::sessions::with_for_actor(tx, ctx, rec, sid, || clax_core::audit::AgentActor {
                session_id: Some(sid.to_string()),
                ..Default::default()
            })?;
        self.record_audit(tx, ctx, rec)?;
        Ok(())
    }
}

fn closed(q: &QuestionRow) -> CoreError {
    CoreError::invalid(
        "question_closed",
        format!("the question is {}", q.status.as_str()),
    )
}

fn not_mirrored() -> CoreError {
    CoreError::invalid(
        "not_mirrored",
        "only a mirrored AskUserQuestion moves to the terminal",
    )
}

fn invalid_thread() -> CoreError {
    CoreError::invalid(
        "invalid_thread",
        "thread_id names no thread of the question's artifact",
    )
}

fn not_answerable() -> CoreError {
    CoreError::invalid(
        "not_answerable",
        "an in-terminal question is answered where it was asked",
    )
}

fn not_in_terminal() -> CoreError {
    CoreError::invalid(
        "not_in_terminal",
        "only an in-terminal question is asked again in Clax or lapses",
    )
}

impl Store {
    /// Records a question for live session `n.session_id`, asked under
    /// `ctx`: `question.ask`, followed by `question.release` (reason
    /// `created`) for one created released. Its inbox item is unread, except
    /// for one created released about no artifact, which is read. A second request with the same
    /// `tool_use_id` for the session returns the first row and `false`, and
    /// records nothing.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session; `not_mirrored` for
    /// an `ask` question created released; `invalid_question` for an
    /// in-terminal question created released; `invalid_thread` when
    /// `thread_id` names no thread of `artifact_id` (or the question has no
    /// artifact); `limit_reached` past
    /// [`MAX_OPEN_PER_SESSION`] or [`MAX_OPEN`] open questions (a question
    /// created released does not count).
    pub fn create_question(&self, ctx: &AuditCtx, n: NewQuestion) -> Result<(QuestionRow, bool)> {
        self.create_question_keyed(ctx, n, None)
    }

    /// [`Store::create_question`], keyed: `key`, an agent token's grant and
    /// the Idempotency-Key it named, is recorded with the question in the
    /// same transaction, and a request whose grant named that key before
    /// returns the question it made and `false`, whatever session asked
    /// it, and records nothing.
    ///
    /// # Errors
    /// [`Store::create_question`]'s.
    pub fn create_question_keyed(
        &self,
        ctx: &AuditCtx,
        n: NewQuestion,
        key: Option<(&str, &str)>,
    ) -> Result<(QuestionRow, bool)> {
        if n.released && n.source == Source::Ask {
            return Err(not_mirrored());
        }
        if n.in_terminal && n.released {
            return Err(CoreError::invalid(
                "invalid_question",
                "an in-terminal question is open until it is answered or withdrawn",
            ));
        }
        if n.thread_id.is_some() && n.artifact_id.is_none() {
            return Err(invalid_thread());
        }
        let json = to_json(&n.questions)?;
        self.with_tx(|tx| {
            let ended: Option<Option<String>> = tx
                .query_row(
                    "SELECT ended_at FROM sessions WHERE id = ?1",
                    params![n.session_id],
                    |r| r.get(0),
                )
                .optional()?;
            if !matches!(ended, Some(None)) {
                return Err(CoreError::invalid(
                    "unknown_session",
                    "no live session has this ID",
                ));
            }
            if let (Some(t), Some(a)) = (&n.thread_id, &n.artifact_id) {
                let on: Option<String> = tx
                    .query_row(
                        "SELECT artifact_id FROM threads WHERE id = ?1",
                        params![t],
                        |r| r.get(0),
                    )
                    .optional()?;
                if on.as_deref() != Some(a.as_str()) {
                    return Err(invalid_thread());
                }
            }
            if let Some((grant, k)) = key {
                let made: Option<String> = tx
                    .query_row(
                        "SELECT question_id FROM question_keys WHERE grant_id = ?1 AND key = ?2",
                        params![grant, k],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(q) = made.map(|id| fetch(tx, &id)).transpose()?.flatten() {
                    return Ok((q, false));
                }
            }
            if let Some(t) = &n.tool_use_id {
                let existing = tx
                    .query_row(BY_TOOL_USE, params![n.session_id, t], row)
                    .optional()?;
                if let Some(q) = existing {
                    return Ok((q, false));
                }
            }
            if !n.released {
                let mine: u32 =
                    tx.query_row(OPEN_COUNT_OF_SESSION, params![n.session_id], |r| r.get(0))?;
                let person = super::inbox::person_of_session(tx, &n.session_id)?;
                let all: u32 = tx.query_row(
                    OPEN_COUNT,
                    params![super::inbox::recipient_key(person.as_deref())],
                    |r| r.get(0),
                )?;
                if mine >= MAX_OPEN_PER_SESSION || all >= MAX_OPEN {
                    return Err(CoreError::invalid(
                        "limit_reached",
                        format!(
                            "at most {MAX_OPEN_PER_SESSION} open questions per session and \
                             {MAX_OPEN} in all; wait for or cancel earlier ones"
                        ),
                    ));
                }
            }
            let id = new_ulid();
            let now = self.now();
            let (status, closed_at) = if n.released {
                (Status::Released, Some(now.clone()))
            } else {
                (Status::Open, None)
            };
            tx.execute(
                "INSERT INTO questions (id, session_id, artifact_id, source, tool_use_id,
                    questions_json, status, created_at, closed_at, person, thread_id, in_terminal)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                         (SELECT person FROM sessions WHERE id = ?2), ?10, ?11)",
                params![
                    id,
                    n.session_id,
                    n.artifact_id,
                    n.source.as_str(),
                    n.tool_use_id,
                    json,
                    status.as_str(),
                    now,
                    closed_at,
                    n.thread_id,
                    n.in_terminal
                ],
            )?;
            if let Some((grant, k)) = key {
                tx.execute(
                    "INSERT INTO question_keys (grant_id, key, question_id, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![grant, k, id, now],
                )?;
            }
            let q = fetch(tx, &id)?.ok_or(CoreError::NotFound)?;
            let (sid, aid) = (&q.session_id, q.artifact_id.as_deref());
            let ask = ask_record(
                &now,
                &id,
                sid,
                aid,
                q.source.as_str(),
                q.tool_use_id.as_deref(),
                serde_json::to_value(&n.questions).map_err(|e| {
                    CoreError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
                })?,
            );
            self.record_question(tx, ctx, ask, sid)?;
            if n.released {
                let rec = close_record(
                    AuditKind::QuestionRelease,
                    &now,
                    &id,
                    sid,
                    aid,
                    Some(Reason::Created),
                )?;
                self.record_question(tx, ctx, rec, sid)?;
            }
            // A mirrored question about no artifact is asked in the
            // terminal (its session has no page): it is never offered in
            // Clax, so its inbox item is read from the start.
            if super::inbox::note_question(tx, &q, &now)?.is_some()
                && n.released
                && q.artifact_id.is_none()
            {
                super::inbox::question_changed(tx, &q, true, &now)?;
            }
            Ok((q, true))
        })
    }

    /// Question `id`, if it exists.
    pub fn question(&self, id: &str) -> Result<Option<QuestionRow>> {
        self.with_read(|c| fetch(c, id))
    }

    /// The IDs of the open questions asked in thread `tid`, oldest first.
    pub fn open_questions_in_thread(&self, tid: &str) -> Result<Vec<String>> {
        self.with_read(|c| {
            let mut s = c.prepare_cached(
                "SELECT id FROM questions WHERE thread_id = ?1 AND status = 'open' ORDER BY id",
            )?;
            Ok(s.query_map(params![tid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Question `id` when one of `recipient`'s agents asked it (`None`: the
    /// owner's agents; spec §18): the asking session's person.
    ///
    /// # Errors
    /// `NotFound` when it does not exist or another person's agent asked it.
    pub fn question_of(&self, id: &str, recipient: Option<&str>) -> Result<QuestionRow> {
        self.with_read(|c| {
            let q = fetch(c, id)?.ok_or(CoreError::NotFound)?;
            if super::inbox::person_of_session(c, &q.session_id)?.as_deref() != recipient {
                return Err(CoreError::NotFound);
            }
            Ok(q)
        })
    }

    /// The person whose agent asked `q` (`None`: the owner's agents): who
    /// its changes are announced to.
    pub fn question_recipient(&self, q: &QuestionRow) -> Result<Option<String>> {
        self.with_read(|c| super::inbox::person_of_session(c, &q.session_id))
    }

    /// Question `qid` as session `sid` sees it.
    ///
    /// # Errors
    /// `NotFound` when it does not exist or another session asked it.
    pub fn session_question(&self, sid: &str, qid: &str) -> Result<QuestionRow> {
        self.question(qid)?
            .filter(|q| q.session_id == sid)
            .ok_or(CoreError::NotFound)
    }

    /// Question `qid` as session `sid` of agent token `grant` sees it: one
    /// `sid` asked, or one any session of `grant` asked under an
    /// Idempotency-Key ([`Store::create_question_keyed`]).
    ///
    /// # Errors
    /// `NotFound` when it does not exist or is neither.
    pub fn session_or_keyed_question(
        &self,
        sid: &str,
        grant: Option<&str>,
        qid: &str,
    ) -> Result<QuestionRow> {
        self.with_read(|c| {
            let q = fetch(c, qid)?.ok_or(CoreError::NotFound)?;
            if q.session_id == sid {
                return Ok(q);
            }
            let Some(g) = grant else {
                return Err(CoreError::NotFound);
            };
            let keyed: bool = c.query_row(
                "SELECT EXISTS (SELECT 1 FROM question_keys k
                   JOIN sessions s ON s.id = ?3 AND s.grant_id = k.grant_id
                 WHERE k.question_id = ?1 AND k.grant_id = ?2)",
                params![qid, g, q.session_id],
                |r| r.get(0),
            )?;
            if keyed {
                Ok(q)
            } else {
                Err(CoreError::NotFound)
            }
        })
    }

    /// The question session `sid` created for tool call `tool_use_id`, if any.
    pub fn question_by_tool_use(
        &self,
        sid: &str,
        tool_use_id: &str,
    ) -> Result<Option<QuestionRow>> {
        self.with_read(|c| {
            Ok(c.query_row(BY_TOOL_USE, params![sid, tool_use_id], row)
                .optional()?)
        })
    }

    /// Applies `c` to question `qid` under `ctx` if its status allows it
    /// (see [`Status`]), recording the time it closed and any answers, and
    /// its event: `question.answer`, `question.decline`, or
    /// `question.release` / `question.withdraw` with the [`reason`] `c`
    /// gives. A refused change records nothing.
    ///
    /// # Errors
    /// `NotFound`; `not_answerable` for an answer, decline, release or
    /// expiry of an in-terminal question; `not_mirrored` for a release of an
    /// `ask` question; `not_in_terminal` for a pull of a question that is
    /// not in the terminal, or a lapse of one that is not a hook in-terminal
    /// question; `question_closed` when the status no longer allows `c`;
    /// `invalid_answer` for answers that do not fit the questions or a `via`
    /// other than `shell`, `extension` or `cli`.
    pub fn close_question(&self, ctx: &AuditCtx, qid: &str, c: Close) -> Result<QuestionRow> {
        // Whether the owner acted (or answered in the terminal): an
        // answered, declined or released question's inbox item is then
        // read. A withdrawn one's stays as it was, and an in-terminal
        // question's is read on every close (`inbox::question_changed`).
        let seen = !matches!(c, Close::Expire);
        self.with_tx(|tx| {
            let q = fetch(tx, qid)?.ok_or(CoreError::NotFound)?;
            let (status, answers, via, why) = match (c, q.status) {
                (Close::Answer { .. } | Close::Decline | Close::Release | Close::Expire, _)
                    if q.in_terminal =>
                {
                    return Err(not_answerable());
                }
                (Close::Pull, _) if !q.in_terminal => return Err(not_in_terminal()),
                (Close::Lapse, _) if !(q.in_terminal && q.source == Source::Hook) => {
                    return Err(not_in_terminal());
                }
                (
                    c @ (Close::Pull
                    | Close::Lapse
                    | Close::Withdraw
                    | Close::Unwaited
                    | Close::Shutdown),
                    Status::Open,
                ) => {
                    let why = match c {
                        Close::Pull => Reason::Pulled,
                        Close::Lapse => Reason::Lapsed,
                        Close::Unwaited => Reason::Unwaited,
                        Close::Shutdown => Reason::DaemonStop,
                        // Close::Withdraw: by its session.
                        _ => Reason::Explicit,
                    };
                    return self
                        .withdraw_question_in(tx, ctx, qid, why)?
                        .ok_or_else(|| closed(&q));
                }
                (Close::Terminal { answers }, Status::Open)
                    if q.in_terminal && q.source == Source::Hook =>
                {
                    (Status::Answered, Some(answers), Some("terminal"), None)
                }
                (Close::Release | Close::Expire, _) if q.source == Source::Ask => {
                    return Err(not_mirrored());
                }
                (Close::Answer { answers, via }, Status::Open) => {
                    if !matches!(via, "shell" | "extension" | "cli") {
                        return Err(CoreError::invalid(
                            "invalid_answer",
                            "an answer comes from the shell, the extension or the CLI",
                        ));
                    }
                    let answers = validate_answers(&q.questions, &answers)?;
                    (Status::Answered, Some(answers), Some(via), None)
                }
                (Close::Decline, Status::Open) => (Status::Declined, None, None, None),
                (Close::Release, Status::Open) => {
                    (Status::Released, None, None, Some(Reason::Owner))
                }
                (Close::Expire, Status::Open) => {
                    (Status::Released, None, None, Some(Reason::Timer))
                }
                (Close::Terminal { answers }, Status::Released) => {
                    (Status::Answered, Some(answers), Some("terminal"), None)
                }
                _ => return Err(closed(&q)),
            };
            let to = Closing {
                status,
                answers: answers.as_deref().map(to_json).transpose()?,
                via,
                why,
            };
            self.close_in(tx, ctx, &q, to, seen, &self.now())
        })
    }

    /// Records that question `qid`'s session received its outcome; a later
    /// call keeps the first time.
    pub fn take_question(&self, qid: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(TAKE, params![qid, self.now()])?;
            Ok(())
        })
    }

    /// Session `sid`'s answered and declined `ask` questions, and its
    /// pulled in-terminal questions, it has not yet received and for which
    /// `skip` is false (given the question ID), in the order they closed,
    /// marked received in the same transaction so each is returned once. A
    /// skipped question stays unreceived; the daemon skips those a question
    /// poll holds, so that poll alone hands them over.
    pub fn take_late_answers(
        &self,
        sid: &str,
        skip: impl Fn(&str) -> bool,
    ) -> Result<Vec<QuestionRow>> {
        self.with_tx(|tx| {
            let mut out = rows(tx, LATE_ANSWERS, params![sid])?;
            out.retain(|q| !skip(&q.id));
            let now = self.now();
            for q in &out {
                tx.execute(TAKE, params![q.id, now])?;
            }
            out.sort_unstable_by(|a, b| (&a.closed_at, &a.id).cmp(&(&b.closed_at, &b.id)));
            Ok(out)
        })
    }

    /// At most `limit` of `recipient`'s questions (those its agents asked;
    /// `None`: the owner's) with the number of them open: open ones oldest
    /// first; closed ones most recently closed first; or all, open ones
    /// newest first followed by closed ones most recently closed first.
    pub fn list_questions(
        &self,
        recipient: Option<&str>,
        which: ListStatus,
        limit: u32,
    ) -> Result<(Vec<QuestionRow>, u32)> {
        self.list_questions_leaving_out(recipient, which, limit, &[])
    }

    /// [`Store::list_questions`] without the questions about the artifacts
    /// `hidden` (left out before the limit, so a page holds `limit`
    /// questions while there are that many others); the count of open
    /// ones still counts every one.
    pub fn list_questions_leaving_out(
        &self,
        recipient: Option<&str>,
        which: ListStatus,
        limit: u32,
        hidden: &[String],
    ) -> Result<(Vec<QuestionRow>, u32)> {
        let r = super::inbox::recipient_key(recipient);
        let ids = super::feedback::id_array(hidden);
        self.with_read(|c| {
            let open: u32 = c.query_row(OPEN_COUNT, params![r], |x| x.get(0))?;
            let page = |base: &str, n: u32| {
                if hidden.is_empty() {
                    return rows(c, base, params![r, n]);
                }
                // The base query with the filter before its ORDER BY.
                let (head, tail) = base.split_at(base.rfind(" ORDER BY").unwrap_or(base.len()));
                let sql = format!(
                    "{head} AND (artifact_id IS NULL OR artifact_id NOT IN (SELECT value FROM json_each(?3))){tail}"
                );
                rows(c, &sql, params![r, n, ids])
            };
            let out = match which {
                ListStatus::Open => page(OPEN_OLDEST, limit)?,
                ListStatus::Closed => page(CLOSED_NEWEST, limit)?,
                ListStatus::All => {
                    let mut out = page(OPEN_NEWEST, limit)?;
                    let left = limit.saturating_sub(out.len() as u32);
                    if left > 0 {
                        out.extend(page(CLOSED_NEWEST, left)?);
                    }
                    out
                }
            };
            Ok((out, open))
        })
    }

    /// How many of `recipient`'s open questions are about the artifacts
    /// `artifacts` (`None`: the owner's).
    pub fn open_questions_about(
        &self,
        recipient: Option<&str>,
        artifacts: &[String],
    ) -> Result<u32> {
        if artifacts.is_empty() {
            return Ok(0);
        }
        let r = super::inbox::recipient_key(recipient);
        let ids = super::feedback::id_array(artifacts);
        self.with_read(|c| {
            let sql = format!("{OPEN_COUNT} AND +artifact_id IN (SELECT value FROM json_each(?2))");
            Ok(c.query_row(&sql, params![r, ids], |x| x.get(0))?)
        })
    }

    /// At most `limit` of `recipient`'s closed questions, most recently
    /// closed first, continuing after `cursor` when given.
    pub fn list_closed_questions(
        &self,
        recipient: Option<&str>,
        cursor: Option<&ClosedCursor>,
        limit: u32,
    ) -> Result<Vec<QuestionRow>> {
        let r = super::inbox::recipient_key(recipient);
        self.with_read(|c| match cursor {
            None => rows(c, CLOSED_NEWEST, params![r, limit]),
            Some(k) => rows(c, CLOSED_BEFORE, params![r, k.closed_at, k.id, limit]),
        })
    }

    /// The IDs of every open `hook` question.
    pub fn open_hook_question_ids(&self) -> Result<Vec<String>> {
        self.with_read(|c| open_ids(c, OPEN_HOOKS, []))
    }

    /// At daemon start: withdraws every open `hook` question, whose hook
    /// was waiting on the previous daemon. Returns their IDs. Each records
    /// `question.withdraw` (reason `daemon_start`) as `system:daemon`,
    /// naming the asking agent in `for_actor`, and updates its inbox item
    /// (`inbox::question_changed`).
    pub fn withdraw_hook_questions_on_start(&self) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let open = open_ids(tx, OPEN_HOOKS, [])?;
            let (ctx, now) = (AuditCtx::system(SystemReason::Daemon), self.now());
            for id in &open {
                self.withdraw_at(tx, &ctx, id, Reason::DaemonStart, &now)?;
            }
            Ok(open)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{DAEMON, session, store};
    use clax_core::CoreError;
    use clax_core::questions::{Answer, Question};
    use rusqlite::params;

    fn qs() -> Vec<Question> {
        serde_json::from_value(serde_json::json!([{"question": "Which?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]))
        .unwrap()
    }

    fn new(sid: &str, source: Source, tool_use_id: Option<&str>) -> NewQuestion {
        NewQuestion {
            session_id: sid.into(),
            artifact_id: None,
            source,
            tool_use_id: tool_use_id.map(Into::into),
            questions: qs(),
            released: false,
            thread_id: None,
            in_terminal: false,
        }
    }

    fn a() -> Vec<Answer> {
        vec![Answer {
            selected: vec!["A".into()],
            text: None,
        }]
    }

    fn thread_on(st: &Store, aid: &clax_core::ArtifactId) -> String {
        st.create_thread(
            DAEMON,
            aid,
            crate::threads::NewThread {
                author_public_id: Some("u_mia".into()),
                version_n: 1,
                anchor: crate::test_util::anchor(),
                body: "here".into(),
                author_name: "Mia".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn invalid(r: Result<impl std::fmt::Debug>, want: &str) {
        match r {
            Err(CoreError::Invalid { code, .. }) => assert_eq!(code, want),
            other => panic!("expected {want}, got {other:?}"),
        }
    }

    /// Whether question `qid`'s inbox item is read.
    fn item_read(st: &Store, qid: &str) -> bool {
        st.with_read(|c| {
            Ok(c.query_row(
                "SELECT read_at IS NOT NULL FROM inbox_items WHERE question_id = ?1",
                params![qid],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    }

    #[test]
    fn a_thread_question_is_refused_off_its_artifact() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let a = crate::test_util::artifact(&st, Some(&sid));
        let b = crate::test_util::artifact(&st, Some(&sid));
        let t = thread_on(&st, &a);
        let on = |aid: Option<&clax_core::ArtifactId>, tid: &str| {
            st.create_question(
                DAEMON,
                NewQuestion {
                    artifact_id: aid.map(|x| x.as_str().to_string()),
                    thread_id: Some(tid.into()),
                    ..new(&sid, Source::Ask, None)
                },
            )
        };
        invalid(on(Some(&b), &t), "invalid_thread");
        invalid(on(None, &t), "invalid_thread");
        invalid(on(Some(&a), "tmissing"), "invalid_thread");
        let (q, made) = on(Some(&a), &t).unwrap();
        assert!(made);
        assert_eq!(q.thread_id.as_deref(), Some(t.as_str()));
        assert!(!q.in_terminal);
    }

    #[test]
    fn an_in_terminal_question_takes_no_answer_and_is_pulled_once() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let in_terminal = || {
            st.create_question(
                DAEMON,
                NewQuestion {
                    in_terminal: true,
                    ..new(&sid, Source::Ask, None)
                },
            )
            .unwrap()
            .0
        };
        let n = in_terminal();
        assert!(n.in_terminal && n.status == Status::Open);
        invalid(
            st.close_question(
                DAEMON,
                &n.id,
                Close::Answer {
                    answers: a(),
                    via: "shell",
                },
            ),
            "not_answerable",
        );
        invalid(
            st.close_question(DAEMON, &n.id, Close::Decline),
            "not_answerable",
        );
        let q = st
            .create_question(DAEMON, new(&sid, Source::Ask, None))
            .unwrap()
            .0;
        invalid(
            st.close_question(DAEMON, &q.id, Close::Pull),
            "not_in_terminal",
        );
        let unread = st.inbox_unread(None).unwrap();
        let pulled = st.close_question(DAEMON, &n.id, Close::Pull).unwrap();
        assert_eq!(pulled.status, Status::Withdrawn);
        assert!(pulled.pulled_at.is_some());
        assert_eq!(
            st.inbox_unread(None).unwrap(),
            unread - 1,
            "the person acted: its item is read"
        );
        invalid(
            st.close_question(DAEMON, &n.id, Close::Pull),
            "question_closed",
        );
        let late: Vec<String> = st
            .take_late_answers(&sid, |_| false)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect();
        assert_eq!(
            late,
            vec![n.id.clone()],
            "handed over once, as a late answer"
        );
        assert!(st.take_late_answers(&sid, |_| false).unwrap().is_empty());
        // Cancelled by its agent (answered in chat): its item is read too.
        let unread = st.inbox_unread(None).unwrap();
        let w = in_terminal();
        assert_eq!(st.inbox_unread(None).unwrap(), unread + 1);
        let w = st.close_question(DAEMON, &w.id, Close::Withdraw).unwrap();
        assert_eq!(w.status, Status::Withdrawn);
        assert!(w.pulled_at.is_none());
        assert_eq!(st.inbox_unread(None).unwrap(), unread);
        assert!(st.take_late_answers(&sid, |_| false).unwrap().is_empty());
    }

    #[test]
    fn an_in_terminal_questions_item_is_read_when_its_session_ends() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let n = st
            .create_question(
                DAEMON,
                NewQuestion {
                    in_terminal: true,
                    ..new(&sid, Source::Ask, None)
                },
            )
            .unwrap()
            .0;
        let q = st
            .create_question(DAEMON, new(&sid, Source::Ask, None))
            .unwrap()
            .0;
        st.end_session(DAEMON, &sid).unwrap();
        assert!(item_read(&st, &n.id), "nothing waits in the terminal now");
        assert!(
            !item_read(&st, &q.id),
            "a Clax question's item stays as it was"
        );
    }

    #[test]
    fn a_hook_in_terminal_question_takes_the_terminal_answer_or_lapses_and_outlives_a_restart() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let in_terminal = |t: &str| {
            st.create_question(
                DAEMON,
                NewQuestion {
                    in_terminal: true,
                    ..new(&sid, Source::Hook, Some(t))
                },
            )
            .unwrap()
            .0
        };
        let answered = in_terminal("t1");
        let q = st
            .close_question(DAEMON, &answered.id, Close::Terminal { answers: a() })
            .unwrap();
        assert_eq!(
            (q.status, q.answered_via.as_deref()),
            (Status::Answered, Some("terminal"))
        );
        assert!(item_read(&st, &q.id));
        let kept = in_terminal("t2");
        assert!(
            st.withdraw_hook_questions_on_start().unwrap().is_empty(),
            "an in-terminal question's hook is not waiting on the daemon"
        );
        assert!(
            st.open_hook_question_ids().unwrap().is_empty(),
            "no grace for an in-terminal question"
        );
        invalid(
            st.close_question(DAEMON, &kept.id, Close::Release),
            "not_answerable",
        );
        let q = st.close_question(DAEMON, &kept.id, Close::Lapse).unwrap();
        assert_eq!(q.status, Status::Withdrawn);
        assert!(q.pulled_at.is_none());
        assert!(item_read(&st, &q.id));
        let plain = st
            .create_question(DAEMON, new(&sid, Source::Hook, Some("t3")))
            .unwrap()
            .0;
        invalid(
            st.close_question(DAEMON, &plain.id, Close::Lapse),
            "not_in_terminal",
        );
        let asked = st
            .create_question(
                DAEMON,
                NewQuestion {
                    in_terminal: true,
                    ..new(&sid, Source::Ask, None)
                },
            )
            .unwrap()
            .0;
        invalid(
            st.close_question(DAEMON, &asked.id, Close::Lapse),
            "not_in_terminal",
        );
        // An in-terminal question is never created released.
        invalid(
            st.create_question(
                DAEMON,
                NewQuestion {
                    in_terminal: true,
                    released: true,
                    ..new(&sid, Source::Hook, Some("t4"))
                },
            ),
            "invalid_question",
        );
    }

    #[test]
    fn a_persons_agents_questions_are_theirs_alone() {
        use crate::people::tests::approved;
        let (_d, _c, st) = crate::people::tests::store();
        let (g, _, _) = approved(&st, "Sam", [7; 32]);
        let sam = st.viewer_of_grant(&g).unwrap().public_id;
        let sams = st
            .register_session(
                DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some("sam-h1".into()),
                    cwd: "/w".into(),
                    grant_id: Some(g),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        let owners = session(&st, "claude", "h1");
        let mine = st
            .create_question(DAEMON, new(&sams, Source::Ask, None))
            .unwrap()
            .0;
        let theirs = st
            .create_question(DAEMON, new(&owners, Source::Ask, None))
            .unwrap()
            .0;
        let ids = |r: Option<&str>| -> (Vec<String>, u32) {
            let (rows, open) = st.list_questions(r, ListStatus::All, 50).unwrap();
            (rows.into_iter().map(|q| q.id).collect(), open)
        };
        assert_eq!(ids(Some(&sam)), (vec![mine.id.clone()], 1));
        assert_eq!(ids(None), (vec![theirs.id.clone()], 1));
        assert!(st.question_of(&mine.id, Some(&sam)).is_ok());
        assert!(matches!(
            st.question_of(&mine.id, None),
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            st.question_of(&theirs.id, Some(&sam)),
            Err(CoreError::NotFound)
        ));
        assert_eq!(st.question_recipient(&mine).unwrap(), Some(sam.clone()));
        assert_eq!(st.question_recipient(&theirs).unwrap(), None);
        st.close_question(DAEMON, &mine.id, Close::Decline).unwrap();
        let closed = st.list_closed_questions(Some(&sam), None, 10).unwrap();
        assert_eq!(closed.len(), 1);
        assert!(st.list_closed_questions(None, None, 10).unwrap().is_empty());
    }

    #[test]
    fn creates_once_per_tool_use() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q1, made) = st
            .create_question(DAEMON, new(&s, Source::Hook, Some("toolu_1")))
            .unwrap();
        assert!(made && q1.status == Status::Open);
        assert_eq!(q1.questions, qs());
        let (q2, made) = st
            .create_question(DAEMON, new(&s, Source::Hook, Some("toolu_1")))
            .unwrap();
        assert!(!made && q2.id == q1.id);
        assert_eq!(
            st.question_by_tool_use(&s, "toolu_1")
                .unwrap()
                .map(|q| q.id),
            Some(q1.id)
        );
    }

    #[test]
    fn an_ended_session_cannot_ask() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        st.end_session(&clax_core::audit::AuditCtx::DAEMON, &s)
            .unwrap();
        assert!(matches!(
            st.create_question(DAEMON, new(&s, Source::Ask, None)),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        assert!(matches!(
            st.create_question(DAEMON, new("01J00000000000000000000000", Source::Ask, None)),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
    }

    #[test]
    fn only_the_asking_session_reaches_it() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let t = session(&st, "codex", "h2");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        assert!(st.session_question(&s, &q.id).is_ok());
        assert!(matches!(
            st.session_question(&t, &q.id),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn answer_and_release_race_has_one_winner() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Hook, Some("t")))
            .unwrap();
        let st = std::sync::Arc::new(st);
        let (x, y) = (st.clone(), st.clone());
        let (qa, qb) = (q.id.clone(), q.id.clone());
        let h1 = std::thread::spawn(move || {
            x.close_question(
                DAEMON,
                &qa,
                Close::Answer {
                    answers: a(),
                    via: "shell",
                },
            )
        });
        let h2 = std::thread::spawn(move || y.close_question(DAEMON, &qb, Close::Release));
        let (r1, r2) = (h1.join().unwrap(), h2.join().unwrap());
        assert!(r1.is_ok() ^ r2.is_ok(), "exactly one transition wins");
        let loser = if r1.is_ok() { r2 } else { r1 };
        assert!(matches!(
            loser,
            Err(CoreError::Invalid {
                code: "question_closed",
                ..
            })
        ));
    }

    #[test]
    fn transitions_follow_the_table() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (ask, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        assert!(matches!(
            st.close_question(DAEMON, &ask.id, Close::Release),
            Err(CoreError::Invalid {
                code: "not_mirrored",
                ..
            })
        ));
        let (hook, _) = st
            .create_question(DAEMON, new(&s, Source::Hook, Some("t")))
            .unwrap();
        let r = st.close_question(DAEMON, &hook.id, Close::Release).unwrap();
        assert_eq!(r.status, Status::Released);
        assert!(r.closed_at.is_some());
        let r = st
            .close_question(DAEMON, &hook.id, Close::Terminal { answers: a() })
            .unwrap();
        assert_eq!(
            (r.status, r.answered_via.as_deref(), r.answers),
            (Status::Answered, Some("terminal"), Some(a()))
        );
        assert!(matches!(
            st.close_question(DAEMON, &hook.id, Close::Decline),
            Err(CoreError::Invalid {
                code: "question_closed",
                ..
            })
        ));
        assert!(matches!(
            st.close_question(DAEMON, "01J00000000000000000000000", Close::Decline),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn a_released_creation_moves_straight_to_the_terminal() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let mut n = new(&s, Source::Hook, Some("t"));
        n.released = true;
        let (q, _) = st.create_question(DAEMON, n).unwrap();
        assert_eq!(q.status, Status::Released);
        assert!(q.closed_at.is_some());
        // About no artifact, it was never offered in Clax: its item is read.
        assert_eq!(st.inbox_unread(None).unwrap(), 0);
        // About one, it was (Clax only mirrors it): its item is unread.
        let mut n = new(&s, Source::Hook, Some("t2"));
        n.released = true;
        n.artifact_id = Some(crate::test_util::artifact(&st, None).as_str().into());
        st.create_question(DAEMON, n).unwrap();
        assert_eq!(st.inbox_unread(None).unwrap(), 1);
    }

    #[test]
    fn limits_open_questions() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        for _ in 0..MAX_OPEN_PER_SESSION {
            st.create_question(DAEMON, new(&s, Source::Ask, None))
                .unwrap();
        }
        assert!(matches!(
            st.create_question(DAEMON, new(&s, Source::Ask, None)),
            Err(CoreError::Invalid {
                code: "limit_reached",
                ..
            })
        ));
        // Another session still has room.
        let t = session(&st, "codex", "h2");
        st.create_question(DAEMON, new(&t, Source::Ask, None))
            .unwrap();
    }

    #[test]
    fn late_answers_are_taken_once() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (d, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (taken, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        st.close_question(
            DAEMON,
            &q.id,
            Close::Answer {
                answers: a(),
                via: "shell",
            },
        )
        .unwrap();
        st.close_question(DAEMON, &d.id, Close::Decline).unwrap();
        st.take_question(&taken.id).unwrap();
        st.close_question(DAEMON, &taken.id, Close::Decline)
            .unwrap();
        let late: Vec<String> = st
            .take_late_answers(&s, |_| false)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect();
        assert_eq!(late, vec![q.id.clone(), d.id.clone()]);
        assert!(st.take_late_answers(&s, |_| false).unwrap().is_empty());
        assert!(st.question(&q.id).unwrap().unwrap().taken_at.is_some());
    }

    #[test]
    fn a_skipped_late_answer_stays_for_a_later_take() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (held, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (free, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        for q in [&held, &free] {
            st.close_question(DAEMON, &q.id, Close::Decline).unwrap();
        }
        let ids = |v: Vec<QuestionRow>| v.into_iter().map(|q| q.id).collect::<Vec<_>>();
        let first = st.take_late_answers(&s, |id| id == held.id).unwrap();
        assert_eq!(ids(first), vec![free.id.clone()]);
        assert!(st.question(&held.id).unwrap().unwrap().taken_at.is_none());
        assert_eq!(
            ids(st.take_late_answers(&s, |_| false).unwrap()),
            vec![held.id.clone()]
        );
    }

    /// A withdrawal at session end or daemon start updates the question's
    /// inbox item, as every other close does, so the inbox listener hears
    /// of it.
    #[test]
    fn session_end_and_daemon_start_withdrawals_touch_the_inbox_items() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (h, _) = st
            .create_question(DAEMON, new(&s, Source::Hook, Some("t")))
            .unwrap();
        let seq_of = |id: &str| -> i64 {
            st.with_read(|c| Ok(c.query_row(crate::inbox::OF_QUESTION, [id], |r| r.get(0))?))
                .unwrap()
        };
        let (q_seq, h_seq) = (seq_of(&q.id), seq_of(&h.id));
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let got = heard.clone();
        st.set_inbox_listener(Box::new(move |c| got.lock().unwrap().extend(c)));
        let seqs = || -> Vec<i64> { heard.lock().unwrap().drain(..).map(|c| c.seq).collect() };
        st.withdraw_hook_questions_on_start().unwrap();
        assert_eq!(seqs(), vec![h_seq], "daemon start");
        st.end_session(DAEMON, &s).unwrap();
        assert!(seqs().contains(&q_seq), "session end");
    }

    #[test]
    fn ending_a_session_withdraws_and_start_sweeps_hooks() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (h, _) = st
            .create_question(DAEMON, new(&s, Source::Hook, Some("t")))
            .unwrap();
        assert_eq!(
            st.withdraw_hook_questions_on_start().unwrap(),
            vec![h.id.clone()]
        );
        let ended = st
            .end_session_touched(&clax_core::audit::AuditCtx::DAEMON, &s)
            .unwrap();
        assert_eq!(ended.withdrawn_questions, vec![q.id.clone()]);
        assert!(ended.session.ended_at.is_some());
        assert_eq!(
            st.question(&q.id).unwrap().unwrap().status,
            Status::Withdrawn
        );
        // Ending it again withdraws nothing more.
        assert!(
            st.end_session_touched(&clax_core::audit::AuditCtx::DAEMON, &s)
                .unwrap()
                .withdrawn_questions
                .is_empty()
        );
    }

    #[test]
    fn reaping_a_session_withdraws_its_questions() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        st.with_write(|c| {
            Ok(c.execute(
                "UPDATE sessions SET last_seen_at = '2000-01-01T00:00:00.000Z'",
                [],
            )?)
        })
        .unwrap();
        let reaped = st
            .reap_sessions(std::time::Duration::from_secs(60), &|_| false)
            .unwrap();
        assert_eq!(reaped.ended, vec![s.clone()]);
        assert_eq!(reaped.withdrawn_questions, vec![q.id.clone()]);
        assert_eq!(
            st.question(&q.id).unwrap().unwrap().status,
            Status::Withdrawn
        );
    }

    #[test]
    fn answers_are_checked_and_trimmed_before_storage() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let bad = |c| {
            matches!(
                c,
                Err(CoreError::Invalid {
                    code: "invalid_answer",
                    ..
                })
            )
        };
        assert!(bad(st.close_question(
            DAEMON,
            &q.id,
            Close::Answer {
                answers: vec![],
                via: "shell"
            }
        )));
        assert!(bad(st.close_question(
            DAEMON,
            &q.id,
            Close::Answer {
                answers: a(),
                via: "terminal"
            }
        )));
        let r = st
            .close_question(
                DAEMON,
                &q.id,
                Close::Answer {
                    answers: vec![Answer {
                        selected: vec![],
                        text: Some("  mine  ".into()),
                    }],
                    via: "cli",
                },
            )
            .unwrap();
        assert_eq!(
            r.answers,
            Some(vec![Answer {
                selected: vec![],
                text: Some("mine".into())
            }])
        );
    }

    #[test]
    fn only_a_mirrored_question_is_created_released() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let mut n = new(&s, Source::Ask, None);
        n.released = true;
        assert!(matches!(
            st.create_question(DAEMON, n),
            Err(CoreError::Invalid {
                code: "not_mirrored",
                ..
            })
        ));
    }

    #[test]
    fn corrupt_rows_fail_without_quoting_them() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (r, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        st.with_write(|c| {
            c.execute_batch("PRAGMA ignore_check_constraints = ON")?;
            c.execute(
                "UPDATE questions SET status = 'bogus' WHERE id = ?1",
                params![q.id],
            )?;
            c.execute(
                "UPDATE questions SET questions_json = '\"a secret\"' WHERE id = ?1",
                params![r.id],
            )?;
            c.execute_batch("PRAGMA ignore_check_constraints = OFF")?;
            Ok(())
        })
        .unwrap();
        assert!(st.question(&q.id).is_err(), "an unknown status is not open");
        let e = st.question(&r.id).unwrap_err().to_string();
        assert!(!e.contains("secret"), "{e}");
    }

    #[test]
    fn closed_questions_page_by_cursor() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let mut ids = Vec::new();
        for _ in 0..5 {
            let (q, _) = st
                .create_question(DAEMON, new(&s, Source::Ask, None))
                .unwrap();
            st.close_question(DAEMON, &q.id, Close::Decline).unwrap();
            ids.push(q.id);
        }
        ids.reverse();
        let first = st.list_closed_questions(None, None, 2).unwrap();
        let got: Vec<_> = first.iter().map(|q| q.id.clone()).collect();
        assert_eq!(got, ids[..2]);
        let cursor = ClosedCursor::after(first.last().unwrap()).unwrap();
        let rest: Vec<_> = st
            .list_closed_questions(None, Some(&cursor), 10)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect();
        assert_eq!(rest, ids[2..]);
    }

    #[test]
    fn lists_open_oldest_first_and_counts() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q1, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (q2, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        let (q3, _) = st
            .create_question(DAEMON, new(&s, Source::Ask, None))
            .unwrap();
        st.close_question(DAEMON, &q1.id, Close::Decline).unwrap();
        let (open, n) = st.list_questions(None, ListStatus::Open, 50).unwrap();
        let ids: Vec<_> = open.iter().map(|q| q.id.clone()).collect();
        assert_eq!((ids, n), (vec![q2.id.clone(), q3.id.clone()], 2));
        let (closed, _) = st.list_questions(None, ListStatus::Closed, 50).unwrap();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].id, q1.id);
        let (all, n) = st.list_questions(None, ListStatus::All, 50).unwrap();
        let ids: Vec<_> = all.iter().map(|q| q.id.clone()).collect();
        assert_eq!(ids, vec![q3.id.clone(), q2.id.clone(), q1.id.clone()]);
        assert_eq!(n, 2);
        let (two, _) = st.list_questions(None, ListStatus::All, 2).unwrap();
        assert_eq!(two.len(), 2, "the open page fills the limit");
        let (one, n) = st.list_questions(None, ListStatus::Open, 1).unwrap();
        assert_eq!((one.len(), n), (1, 2));
    }

    #[test]
    fn a_list_leaves_out_its_hidden_artifacts_before_the_limit() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (hidden, shown) = (
            crate::test_util::artifact(&st, Some(&s)),
            crate::test_util::artifact(&st, Some(&s)),
        );
        let ask = |aid: Option<&clax_core::ArtifactId>| {
            let n = NewQuestion {
                artifact_id: aid.map(|x| x.as_str().to_string()),
                ..new(&s, Source::Ask, None)
            };
            st.create_question(DAEMON, n).unwrap().0.id
        };
        let (h1, h2) = (ask(Some(&hidden)), ask(Some(&hidden)));
        let (on_shown, on_none) = (ask(Some(&shown)), ask(None));
        let out = [hidden.as_str().to_string()];
        let ids = |which, limit| {
            let (rows, open) = st
                .list_questions_leaving_out(None, which, limit, &out)
                .unwrap();
            (rows.into_iter().map(|q| q.id).collect::<Vec<_>>(), open)
        };
        // The oldest two are hidden: a page of two holds the next two.
        assert_eq!(
            ids(ListStatus::Open, 2),
            (vec![on_shown.clone(), on_none.clone()], 4)
        );
        assert_eq!(ids(ListStatus::All, 1).0, vec![on_none.clone()]);
        st.close_question(DAEMON, &h1, Close::Decline).unwrap();
        st.close_question(DAEMON, &on_shown, Close::Decline)
            .unwrap();
        st.close_question(DAEMON, &h2, Close::Decline).unwrap();
        assert_eq!(ids(ListStatus::Closed, 1).0, vec![on_shown]);
        let (all, _) = st.list_questions(None, ListStatus::All, 10).unwrap();
        assert_eq!(all.len(), 4, "nothing left out without hidden artifacts");
    }

    /// The question events recorded after `seq`: (kind, question ID,
    /// actor type, reason), in order.
    fn question_events(st: &Store, seq: i64) -> Vec<(String, String, String, Value)> {
        st.events_after(seq, 10_000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("question."))
            .map(|e| {
                let body: Value = serde_json::from_str(&e.body).unwrap();
                let actor: Value = serde_json::from_str(&e.actor).unwrap();
                (
                    e.kind,
                    e.ids.question.unwrap(),
                    actor["type"].as_str().unwrap().to_string(),
                    body.get("reason").cloned().unwrap_or(Value::Null),
                )
            })
            .collect()
    }

    fn agent_ctx(st: &Store, sid: &str) -> AuditCtx {
        AuditCtx {
            actor: clax_core::audit::Actor::Agent(st.session_actor(sid).unwrap().unwrap()),
            via: clax_core::audit::Via::Mcp,
            git: clax_core::gitctx::GitField::Absent,
            call: None,
        }
    }

    fn owner_ctx() -> AuditCtx {
        AuditCtx {
            actor: clax_core::audit::Actor::Owner {
                public_id: "u_owner".into(),
            },
            via: clax_core::audit::Via::Shell,
            git: clax_core::gitctx::GitField::Absent,
            call: None,
        }
    }

    #[test]
    fn each_question_transition_records_one_event() {
        use serde_json::json;
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let agent = agent_ctx(&st, &s);
        let owner = owner_ctx();
        let daemon = AuditCtx::DAEMON;
        let seq = st.newest_seq().unwrap();
        let ev = |seq| question_events(&st, seq);
        let one = |kind: &str, qid: &str, actor: &str, reason: Value| {
            vec![(kind.to_string(), qid.to_string(), actor.to_string(), reason)]
        };

        // Ask, then the same tool call again: one event.
        let (q, _) = st
            .create_question(&agent, new(&s, Source::Hook, Some("t1")))
            .unwrap();
        st.create_question(&agent, new(&s, Source::Hook, Some("t1")))
            .unwrap();
        assert_eq!(ev(seq), one("question.ask", &q.id, "agent", Value::Null));
        let ask = st.events_after(seq, 10).unwrap().pop().unwrap();
        let body: Value = serde_json::from_str(&ask.body).unwrap();
        assert_eq!(body["source"], "hook");
        assert_eq!(body["tool_use_id"], "t1");
        assert_eq!(body["questions"], serde_json::to_value(qs()).unwrap());
        assert_eq!(ask.ids.session.as_deref(), Some(s.as_str()));

        // Each close, by its actor, with its reason.
        let cases: Vec<(Close, &AuditCtx, &str, Value)> = vec![
            (
                Close::Answer {
                    answers: a(),
                    via: "shell",
                },
                &owner,
                "question.answer",
                Value::Null,
            ),
            (Close::Decline, &owner, "question.decline", Value::Null),
            (Close::Release, &owner, "question.release", json!("owner")),
            (Close::Expire, &agent, "question.release", json!("timer")),
            (
                Close::Withdraw,
                &agent,
                "question.withdraw",
                json!("explicit"),
            ),
            (
                Close::Unwaited,
                &daemon,
                "question.withdraw",
                json!("unwaited"),
            ),
            (
                Close::Shutdown,
                &daemon,
                "question.withdraw",
                json!("daemon_stop"),
            ),
        ];
        for (c, ctx, kind, reason) in cases {
            let (q, _) = st
                .create_question(&agent, new(&s, Source::Hook, None))
                .unwrap();
            let seq = st.newest_seq().unwrap();
            st.close_question(ctx, &q.id, c).unwrap();
            let actor = match ctx.actor {
                clax_core::audit::Actor::Owner { .. } => "owner",
                clax_core::audit::Actor::Agent(_) => "agent",
                _ => "system",
            };
            assert_eq!(ev(seq), one(kind, &q.id, actor, reason), "{kind}");
            // A refused second change records nothing.
            let seq = st.newest_seq().unwrap();
            assert!(st.close_question(&owner, &q.id, Close::Decline).is_err());
            assert!(ev(seq).is_empty(), "{kind}: a refusal records nothing");
        }
        let (q2, _) = st
            .create_question(&agent, new(&s, Source::Ask, None))
            .unwrap();
        let seq = st.newest_seq().unwrap();
        st.close_question(
            &owner,
            &q2.id,
            Close::Answer {
                answers: vec![Answer {
                    selected: vec![],
                    text: Some("  mine  ".into()),
                }],
                via: "cli",
            },
        )
        .unwrap();
        let e = st.events_after(seq, 10).unwrap().pop().unwrap();
        let body: Value = serde_json::from_str(&e.body).unwrap();
        assert_eq!(
            (body["answers"].clone(), body["answered_via"].clone()),
            (json!([{"selected": [], "text": "mine"}]), json!("cli")),
            "the answers as stored"
        );
        // An ask question cannot be released: refused, nothing recorded.
        assert!(st.close_question(&owner, &q2.id, Close::Release).is_err());
        assert!(ev(e.seq).is_empty());

        // Created released: asked, then released (`created`); then answered
        // in the terminal.
        let mut n = new(&s, Source::Hook, Some("t2"));
        n.released = true;
        let seq = st.newest_seq().unwrap();
        let (r, _) = st.create_question(&agent, n).unwrap();
        let mut want = one("question.ask", &r.id, "agent", Value::Null);
        want.extend(one("question.release", &r.id, "agent", json!("created")));
        assert_eq!(ev(seq), want);
        let seq = st.newest_seq().unwrap();
        st.close_question(&owner, &r.id, Close::Terminal { answers: a() })
            .unwrap();
        assert_eq!(ev(seq), one("question.answer", &r.id, "owner", Value::Null));
        let e = st.events_after(seq, 10).unwrap().pop().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&e.body).unwrap()["answered_via"],
            "terminal"
        );

        // Daemon start withdraws the open hook questions (the first one
        // asked, and `h`) as system:daemon, naming the agent.
        let (h, _) = st
            .create_question(&agent, new(&s, Source::Hook, Some("t3")))
            .unwrap();
        let (k, _) = st
            .create_question(&agent, new(&s, Source::Ask, None))
            .unwrap();
        let seq = st.newest_seq().unwrap();
        st.withdraw_hook_questions_on_start().unwrap();
        let mut want = one("question.withdraw", &q.id, "system", json!("daemon_start"));
        want.extend(one(
            "question.withdraw",
            &h.id,
            "system",
            json!("daemon_start"),
        ));
        assert_eq!(ev(seq), want);
        let e = st.events_after(seq, 10).unwrap().pop().unwrap();
        let body: Value = serde_json::from_str(&e.body).unwrap();
        assert_eq!(body["for_actor"]["session_id"], s.as_str());
        let seq = st.newest_seq().unwrap();
        st.withdraw_hook_questions_on_start().unwrap();
        assert!(ev(seq).is_empty(), "nothing open, nothing recorded");

        // Ending the session withdraws the rest, after its session.end.
        let seq = st.newest_seq().unwrap();
        st.end_session(&agent, &s).unwrap();
        assert_eq!(
            ev(seq),
            one("question.withdraw", &k.id, "agent", json!("session_end"))
        );
        let kinds: Vec<String> = st
            .events_after(seq, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, ["session.end", "question.withdraw"]);

        // The reaper withdraws as system:ttl, naming the agent.
        let t = session(&st, "codex", "h2");
        let (w, _) = st
            .create_question(&agent_ctx(&st, &t), new(&t, Source::Ask, None))
            .unwrap();
        st.with_write(|c| {
            Ok(c.execute(
                "UPDATE sessions SET last_seen_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
                params![t],
            )?)
        })
        .unwrap();
        let seq = st.newest_seq().unwrap();
        st.reap_sessions(std::time::Duration::from_secs(60), &|_| false)
            .unwrap();
        assert_eq!(
            ev(seq),
            one("question.withdraw", &w.id, "system", json!("session_end"))
        );
        let e = st.events_after(seq, 10).unwrap().pop().unwrap();
        let actor: Value = serde_json::from_str(&e.actor).unwrap();
        assert_eq!(actor["reason"], "ttl");
        let body: Value = serde_json::from_str(&e.body).unwrap();
        assert_eq!(body["for_actor"]["session_id"], t.as_str());
    }

    #[test]
    fn the_close_builder_refuses_a_reason_outside_its_kind() {
        let at = "2026-10-06T00:00:00.000Z";
        let build = |kind, why| close_record(kind, at, "q", "s", None, why);
        let refused = |r: Result<AuditRecord>| {
            matches!(
                r,
                Err(CoreError::Invalid {
                    code: "invalid_reason",
                    ..
                })
            )
        };
        for r in Reason::ALL {
            assert_eq!(Reason::parse(r.as_str()), Some(r));
            let rec = build(r.kind(), Some(r)).unwrap();
            assert_eq!(rec.body["reason"], r.as_str());
            let other = match r.kind() {
                AuditKind::QuestionRelease => AuditKind::QuestionWithdraw,
                _ => AuditKind::QuestionRelease,
            };
            assert!(refused(build(other, Some(r))), "{r:?} on {other}");
            assert!(refused(build(AuditKind::QuestionDecline, Some(r))));
            assert!(refused(build(AuditKind::QuestionAnswer, Some(r))));
        }
        // No other phrase is a reason.
        for s in ["", "Owner", "ttl", "free text", "session-end"] {
            assert_eq!(Reason::parse(s), None, "{s}");
        }
        // Unknown reasons (the backfill's) are null; a decline has none.
        assert!(build(AuditKind::QuestionWithdraw, None).unwrap().body["reason"].is_null());
        assert!(
            build(AuditKind::QuestionDecline, None)
                .unwrap()
                .body
                .is_empty()
        );
        assert!(refused(build(AuditKind::QuestionAsk, None)));
    }
}
