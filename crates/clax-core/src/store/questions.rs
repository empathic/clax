//! Agent questions (spec 2026-10-06-agent-questions-and-inbox-design §5.1):
//! rows, their one-way transitions (each in one write transaction, so of two
//! racing changes the first wins and the other is `question_closed`), the
//! open limits, and delivery bookkeeping.

use super::Store;
use crate::questions::{Answer, Question};
use crate::{CoreError, Result, new_ulid};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

/// Open questions one session may have at once.
pub const MAX_OPEN_PER_SESSION: u32 = 8;
/// Open questions across all sessions.
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

    fn parse(s: &str) -> Status {
        match s {
            "answered" => Status::Answered,
            "declined" => Status::Declined,
            "released" => Status::Released,
            "withdrawn" => Status::Withdrawn,
            _ => Status::Open,
        }
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
}

/// A transition out of `Open` (or, for `Terminal`, out of `Released`).
pub enum Close {
    /// Answered in Clax; `via` is `shell`, `extension` or `cli`.
    Answer {
        answers: Vec<Answer>,
        via: &'static str,
    },
    Decline,
    /// Handed to the terminal; only for `hook` questions.
    Release,
    Withdraw,
    /// A released question answered in the terminal.
    Terminal {
        answers: Vec<Answer>,
    },
}

/// Which questions [`Store::list_questions`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStatus {
    Open,
    Closed,
    All,
}

const COLS: &str = "id, session_id, artifact_id, source, tool_use_id, questions_json, status,
    answers_json, answered_via, created_at, closed_at, taken_at";

fn corrupt(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn to_json<T: Serialize + ?Sized>(v: &T) -> Result<String> {
    serde_json::to_string(v)
        .map_err(|e| CoreError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))
}

fn row(r: &Row<'_>) -> rusqlite::Result<QuestionRow> {
    let qs: String = r.get("questions_json")?;
    let ans: Option<String> = r.get("answers_json")?;
    Ok(QuestionRow {
        id: r.get("id")?,
        session_id: r.get("session_id")?,
        artifact_id: r.get("artifact_id")?,
        source: if r.get::<_, String>("source")? == "hook" {
            Source::Hook
        } else {
            Source::Ask
        },
        tool_use_id: r.get("tool_use_id")?,
        questions: serde_json::from_str(&qs).map_err(corrupt)?,
        status: Status::parse(&r.get::<_, String>("status")?),
        answers: ans
            .map(|a| serde_json::from_str(&a))
            .transpose()
            .map_err(corrupt)?,
        answered_via: r.get("answered_via")?,
        created_at: r.get("created_at")?,
        closed_at: r.get("closed_at")?,
        taken_at: r.get("taken_at")?,
    })
}

fn fetch(c: &Connection, id: &str) -> Result<Option<QuestionRow>> {
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM questions WHERE id = ?1"),
        params![id],
        row,
    )
    .optional()?)
}

fn ids(c: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<String>> {
    let mut stmt = c.prepare(sql)?;
    Ok(stmt
        .query_map(p, |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Withdraws session `sid`'s open questions inside the caller's
/// transaction; returns their IDs.
pub(crate) fn withdraw_session(c: &Connection, sid: &str, now: &str) -> Result<Vec<String>> {
    let out = ids(
        c,
        "SELECT id FROM questions WHERE session_id = ?1 AND status = 'open' ORDER BY id",
        params![sid],
    )?;
    if !out.is_empty() {
        c.execute(
            "UPDATE questions SET status = 'withdrawn', closed_at = ?2
             WHERE session_id = ?1 AND status = 'open'",
            params![sid, now],
        )?;
    }
    Ok(out)
}

fn closed(q: &QuestionRow) -> CoreError {
    CoreError::invalid(
        "question_closed",
        format!("the question is {}", q.status.as_str()),
    )
}

impl Store {
    /// Records a question for live session `n.session_id`. A second request
    /// with the same `tool_use_id` for the session returns the first row and
    /// `false`.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session; `limit_reached`
    /// past [`MAX_OPEN_PER_SESSION`] or [`MAX_OPEN`] open questions (a
    /// question created released does not count).
    pub fn create_question(&self, n: NewQuestion) -> Result<(QuestionRow, bool)> {
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
            if let Some(t) = &n.tool_use_id {
                let existing = tx
                    .query_row(
                        &format!(
                            "SELECT {COLS} FROM questions WHERE session_id = ?1 AND tool_use_id = ?2"
                        ),
                        params![n.session_id, t],
                        row,
                    )
                    .optional()?;
                if let Some(q) = existing {
                    return Ok((q, false));
                }
            }
            if !n.released {
                let mine: u32 = tx.query_row(
                    "SELECT COUNT(*) FROM questions WHERE session_id = ?1 AND status = 'open'",
                    params![n.session_id],
                    |r| r.get(0),
                )?;
                let all: u32 = tx.query_row(
                    "SELECT COUNT(*) FROM questions WHERE status = 'open'",
                    [],
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
            let now = Store::now();
            let (status, closed_at) = if n.released {
                (Status::Released, Some(now.clone()))
            } else {
                (Status::Open, None)
            };
            tx.execute(
                "INSERT INTO questions (id, session_id, artifact_id, source, tool_use_id,
                    questions_json, status, created_at, closed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    n.session_id,
                    n.artifact_id,
                    n.source.as_str(),
                    n.tool_use_id,
                    json,
                    status.as_str(),
                    now,
                    closed_at
                ],
            )?;
            let q = fetch(tx, &id)?.ok_or(CoreError::NotFound)?;
            Ok((q, true))
        })
    }

    /// Question `id`, if it exists.
    pub fn question(&self, id: &str) -> Result<Option<QuestionRow>> {
        self.with_read(|c| fetch(c, id))
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

    /// The question session `sid` created for tool call `tool_use_id`, if any.
    pub fn question_by_tool_use(
        &self,
        sid: &str,
        tool_use_id: &str,
    ) -> Result<Option<QuestionRow>> {
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM questions WHERE session_id = ?1 AND tool_use_id = ?2"),
                params![sid, tool_use_id],
                row,
            )
            .optional()?)
        })
    }

    /// Applies `c` to question `qid` if its status allows it (see
    /// [`Status`]), recording the time it closed and any answers.
    ///
    /// # Errors
    /// `NotFound`; `not_mirrored` for a release of an `ask` question;
    /// `question_closed` when the status no longer allows `c`.
    pub fn close_question(&self, qid: &str, c: Close) -> Result<QuestionRow> {
        self.with_tx(|tx| {
            let q = fetch(tx, qid)?.ok_or(CoreError::NotFound)?;
            let (status, answers, via) = match (&c, q.status) {
                (Close::Release, _) if q.source == Source::Ask => {
                    return Err(CoreError::invalid(
                        "not_mirrored",
                        "only a mirrored AskUserQuestion moves to the terminal",
                    ));
                }
                (Close::Answer { answers, via }, Status::Open) => {
                    (Status::Answered, Some(answers), Some(*via))
                }
                (Close::Decline, Status::Open) => (Status::Declined, None, None),
                (Close::Release, Status::Open) => (Status::Released, None, None),
                (Close::Withdraw, Status::Open) => (Status::Withdrawn, None, None),
                (Close::Terminal { answers }, Status::Released) => {
                    (Status::Answered, Some(answers), Some("terminal"))
                }
                _ => return Err(closed(&q)),
            };
            let answers = answers.map(to_json).transpose()?;
            tx.execute(
                "UPDATE questions SET status = ?2, answers_json = ?3, answered_via = ?4,
                    closed_at = ?5 WHERE id = ?1",
                params![qid, status.as_str(), answers, via, Store::now()],
            )?;
            fetch(tx, qid)?.ok_or(CoreError::NotFound)
        })
    }

    /// Records that question `qid`'s session received its outcome; a later
    /// call keeps the first time.
    pub fn take_question(&self, qid: &str) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE questions SET taken_at = ?2 WHERE id = ?1 AND taken_at IS NULL",
                params![qid, Store::now()],
            )?;
            Ok(())
        })
    }

    /// Session `sid`'s answered and declined `ask` questions it has not yet
    /// received, in the order they closed, marked received in the same
    /// transaction so each is returned once.
    pub fn take_late_answers(&self, sid: &str) -> Result<Vec<QuestionRow>> {
        self.with_tx(|tx| {
            let mut stmt = tx.prepare(&format!(
                "SELECT {COLS} FROM questions WHERE session_id = ?1 AND source = 'ask'
                    AND status IN ('answered', 'declined') AND taken_at IS NULL
                 ORDER BY closed_at, id"
            ))?;
            let rows = stmt
                .query_map(params![sid], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let now = Store::now();
            for q in &rows {
                tx.execute(
                    "UPDATE questions SET taken_at = ?2 WHERE id = ?1",
                    params![q.id, now],
                )?;
            }
            Ok(rows)
        })
    }

    /// At most `limit` questions: open ones oldest first, closed ones most
    /// recently closed first, or all (open first, then newest first); and
    /// the number open.
    pub fn list_questions(&self, which: ListStatus, limit: u32) -> Result<(Vec<QuestionRow>, u32)> {
        self.with_read(|c| {
            let open: u32 = c.query_row(
                "SELECT COUNT(*) FROM questions WHERE status = 'open'",
                [],
                |r| r.get(0),
            )?;
            let tail = match which {
                ListStatus::Open => "WHERE status = 'open' ORDER BY created_at, id",
                ListStatus::Closed => "WHERE status <> 'open' ORDER BY closed_at DESC, id DESC",
                ListStatus::All => "ORDER BY status <> 'open', created_at DESC, id DESC",
            };
            let mut stmt = c.prepare(&format!("SELECT {COLS} FROM questions {tail} LIMIT ?1"))?;
            let rows = stmt
                .query_map(params![limit], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows, open))
        })
    }

    /// At daemon start: withdraws every open `hook` question, whose hook
    /// was waiting on the previous daemon. Returns their IDs.
    pub fn withdraw_hook_questions_on_start(&self) -> Result<Vec<String>> {
        self.with_tx(|tx| {
            let out = ids(
                tx,
                "SELECT id FROM questions WHERE source = 'hook' AND status = 'open' ORDER BY id",
                [],
            )?;
            tx.execute(
                "UPDATE questions SET status = 'withdrawn', closed_at = ?1
                 WHERE source = 'hook' AND status = 'open'",
                params![Store::now()],
            )?;
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CoreError;
    use crate::questions::{Answer, Question};
    use crate::store::test_util::{session, store};

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
        }
    }

    fn a() -> Vec<Answer> {
        vec![Answer {
            selected: vec!["A".into()],
            text: None,
        }]
    }

    #[test]
    fn creates_once_per_tool_use() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q1, made) = st
            .create_question(new(&s, Source::Hook, Some("toolu_1")))
            .unwrap();
        assert!(made && q1.status == Status::Open);
        assert_eq!(q1.questions, qs());
        let (q2, made) = st
            .create_question(new(&s, Source::Hook, Some("toolu_1")))
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
        st.end_session(&s).unwrap();
        assert!(matches!(
            st.create_question(new(&s, Source::Ask, None)),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        assert!(matches!(
            st.create_question(new("01J00000000000000000000000", Source::Ask, None)),
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
        let (q, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
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
            .create_question(new(&s, Source::Hook, Some("t")))
            .unwrap();
        let st = std::sync::Arc::new(st);
        let (x, y) = (st.clone(), st.clone());
        let (qa, qb) = (q.id.clone(), q.id.clone());
        let h1 = std::thread::spawn(move || {
            x.close_question(
                &qa,
                Close::Answer {
                    answers: a(),
                    via: "shell",
                },
            )
        });
        let h2 = std::thread::spawn(move || y.close_question(&qb, Close::Release));
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
        let (ask, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        assert!(matches!(
            st.close_question(&ask.id, Close::Release),
            Err(CoreError::Invalid {
                code: "not_mirrored",
                ..
            })
        ));
        let (hook, _) = st
            .create_question(new(&s, Source::Hook, Some("t")))
            .unwrap();
        let r = st.close_question(&hook.id, Close::Release).unwrap();
        assert_eq!(r.status, Status::Released);
        assert!(r.closed_at.is_some());
        let r = st
            .close_question(&hook.id, Close::Terminal { answers: a() })
            .unwrap();
        assert_eq!(
            (r.status, r.answered_via.as_deref(), r.answers),
            (Status::Answered, Some("terminal"), Some(a()))
        );
        assert!(matches!(
            st.close_question(&hook.id, Close::Decline),
            Err(CoreError::Invalid {
                code: "question_closed",
                ..
            })
        ));
        assert!(matches!(
            st.close_question("01J00000000000000000000000", Close::Decline),
            Err(CoreError::NotFound)
        ));
    }

    #[test]
    fn a_released_creation_moves_straight_to_the_terminal() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let mut n = new(&s, Source::Hook, Some("t"));
        n.released = true;
        let (q, _) = st.create_question(n).unwrap();
        assert_eq!(q.status, Status::Released);
        assert!(q.closed_at.is_some());
    }

    #[test]
    fn limits_open_questions() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        for _ in 0..MAX_OPEN_PER_SESSION {
            st.create_question(new(&s, Source::Ask, None)).unwrap();
        }
        assert!(matches!(
            st.create_question(new(&s, Source::Ask, None)),
            Err(CoreError::Invalid {
                code: "limit_reached",
                ..
            })
        ));
        // Another session still has room.
        let t = session(&st, "codex", "h2");
        st.create_question(new(&t, Source::Ask, None)).unwrap();
    }

    #[test]
    fn late_answers_are_taken_once() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        let (d, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        let (taken, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        st.close_question(
            &q.id,
            Close::Answer {
                answers: a(),
                via: "shell",
            },
        )
        .unwrap();
        st.close_question(&d.id, Close::Decline).unwrap();
        st.take_question(&taken.id).unwrap();
        st.close_question(&taken.id, Close::Decline).unwrap();
        let late: Vec<String> = st
            .take_late_answers(&s)
            .unwrap()
            .into_iter()
            .map(|q| q.id)
            .collect();
        assert_eq!(late, vec![q.id.clone(), d.id.clone()]);
        assert!(st.take_late_answers(&s).unwrap().is_empty());
        assert!(st.question(&q.id).unwrap().unwrap().taken_at.is_some());
    }

    #[test]
    fn ending_a_session_withdraws_and_start_sweeps_hooks() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        let (h, _) = st
            .create_question(new(&s, Source::Hook, Some("t")))
            .unwrap();
        assert_eq!(
            st.withdraw_hook_questions_on_start().unwrap(),
            vec![h.id.clone()]
        );
        let ended = st.end_session_touched(&s).unwrap();
        assert_eq!(ended.withdrawn_questions, vec![q.id.clone()]);
        assert!(ended.session.ended_at.is_some());
        assert_eq!(
            st.question(&q.id).unwrap().unwrap().status,
            Status::Withdrawn
        );
        // Ending it again withdraws nothing more.
        assert!(
            st.end_session_touched(&s)
                .unwrap()
                .withdrawn_questions
                .is_empty()
        );
    }

    #[test]
    fn reaping_a_session_withdraws_its_questions() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
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
    fn lists_open_oldest_first_and_counts() {
        let (_d, st) = store();
        let s = session(&st, "claude", "h1");
        let (q1, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        let (q2, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        let (q3, _) = st.create_question(new(&s, Source::Ask, None)).unwrap();
        st.close_question(&q1.id, Close::Decline).unwrap();
        let (open, n) = st.list_questions(ListStatus::Open, 50).unwrap();
        let ids: Vec<_> = open.iter().map(|q| q.id.clone()).collect();
        assert_eq!((ids, n), (vec![q2.id.clone(), q3.id.clone()], 2));
        let (closed, _) = st.list_questions(ListStatus::Closed, 50).unwrap();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].id, q1.id);
        let (all, n) = st.list_questions(ListStatus::All, 50).unwrap();
        assert_eq!((all.len(), n), (3, 2));
        assert_eq!(all[2].id, q1.id, "closed after open");
        let (one, n) = st.list_questions(ListStatus::Open, 1).unwrap();
        assert_eq!((one.len(), n), (1, 2));
    }
}
