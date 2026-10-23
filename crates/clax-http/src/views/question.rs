//! The owner view of an agent question, its announcement to its recipient
//! and its pollers, and the grace that withdraws a hook question no poll
//! waits on.

use crate::state::AppState;
use clax_core::{ArtifactId, CoreError, Event};
use clax_realtime::questions::OnLast;
use clax_store::Store;
use clax_store::questions::{Close, QuestionRow, Source, Status};
use serde_json::{Value, json};

/// The owner view of `q` (spec §5.4): never a session ID, PID or working
/// directory; `project` is the last component of the session's directory;
/// `artifact` is null for none or a deleted one; `thread`, `{id, open}`, is
/// the thread of `artifact` it was asked in, null for none or a deleted
/// one; `in_terminal` says the agent asks it in the terminal or in chat;
/// `pulled` says the person chose Ask me here on it; `linked`, on a local
/// daemon, says where its copy on a remote stands
/// ([`clax_store::linked::LinkedQuestionView`]), null for none.
pub fn view(st: &Store, q: &QuestionRow) -> clax_core::Result<Value> {
    let agent = match st.get_session(&q.session_id)? {
        Some(s) => {
            let mut a = json!({
                "handle": s.agent_handle,
                "name": s.name,
                "harness": s.harness,
                "project": std::path::Path::new(&s.cwd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(""),
            });
            // Another Clax's agent: whose Clax it asks from.
            if let Some(via) = st.session_via(s.grant_id.as_deref(), s.person.as_deref())? {
                a["via"] = json!(via);
            }
            Some(a)
        }
        None => None,
    };
    let (artifact, thread) = match q.artifact_id.as_deref().map(ArtifactId::parse) {
        Some(Ok(id)) => match st.get_artifact(&id)? {
            Some(a) => {
                // Its thread while it is one of its artifact's; null once deleted.
                let thread = match &q.thread_id {
                    Some(t) => st
                        .thread_statuses(&id, std::slice::from_ref(t))?
                        .remove(t)
                        .map(|status| json!({"id": t, "open": status == "open"})),
                    None => None,
                };
                (
                    Some(json!({"id": a.id, "title": a.title, "kind": a.kind})),
                    thread,
                )
            }
            None => (None, None),
        },
        _ => (None, None),
    };
    Ok(json!({
        "id": q.id,
        "agent": agent,
        "artifact": artifact,
        "source": q.source,
        "status": q.status.as_str(),
        "questions": q.questions,
        "answers": q.answers,
        "answered_via": q.answered_via,
        "created_at": q.created_at,
        "closed_at": q.closed_at,
        "thread": thread,
        "in_terminal": q.in_terminal,
        "pulled": q.pulled_at.is_some(),
        "linked": st.linked_question_view(&q.id)?,
    }))
}

/// Publishes `view` (the view of `q`) as a `question` event to the
/// `questions` topic of the person whose agent asked it (read from `st`)
/// and wakes the polls of `q`; an answered or declined `ask` question, or
/// a pulled in-terminal question, also wakes its session's feedback polls,
/// which carry late answers (spec §6.4) and pulls.
pub fn announce(s: &AppState, st: &Store, q: &QuestionRow, view: Value) {
    match st.question_recipient(q).and_then(|r| {
        let theirs = in_reach(s, st, r.as_deref(), q)?;
        Ok((r, theirs))
    }) {
        Ok((recipient, true)) => s.events.publish(Event::Question {
            question: view,
            recipient,
        }),
        Ok((_, false)) => {}
        Err(e) => tracing::warn!(error = %e, "a question's change not announced"),
    }
    s.questions.wake(&q.id);
    if (q.source == Source::Ask && matches!(q.status, Status::Answered | Status::Declined))
        || q.pulled_at.is_some()
    {
        s.feedback_waiters.wake(std::iter::once(&q.session_id));
    }
}

/// Whether question `q` is `recipient`'s to see on this daemon: on a hosted
/// one, not about an artifact out of their reach
/// ([`crate::views::inbox::recipient_lists`]).
///
/// # Errors
/// The store's.
pub fn in_reach(
    s: &AppState,
    st: &Store,
    recipient: Option<&str>,
    q: &QuestionRow,
) -> clax_core::Result<bool> {
    match &q.artifact_id {
        Some(aid) => crate::views::inbox::recipient_lists(st, recipient, s.hosted.is_some(), aid),
        None => Ok(true),
    }
}

/// Announces each of `ids` as it now is (the questions an ended session
/// withdrew). Runs on a store thread.
pub fn announce_ids(s: &AppState, st: &Store, ids: &[String]) {
    for id in ids {
        let r = st
            .question(id)
            .and_then(|q| q.map(|q| view(st, &q).map(|v| (q, v))).transpose());
        match r {
            Ok(Some((q, v))) => announce(s, st, &q, v),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "could not announce a withdrawn question"),
        }
    }
}

/// Publishes, as a `question` event to its recipient, each open question
/// asked in thread `tid` as it now is: the thread was resolved, reopened
/// or deleted, which its view's `thread` shows. Wakes no poll: none of
/// them changed. Runs on a store thread.
pub fn announce_thread(s: &AppState, st: &Store, tid: &str) {
    let ids = match st.open_questions_in_thread(tid) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = %e, "a thread's questions not announced");
            return;
        }
    };
    for id in ids {
        let r = st.question(&id).and_then(|q| {
            q.map(|q| {
                let v = view(st, &q)?;
                let recipient = st.question_recipient(&q)?;
                let theirs = in_reach(s, st, recipient.as_deref(), &q)?;
                Ok(theirs.then_some((recipient, v)))
            })
            .transpose()
            .map(Option::flatten)
        });
        match r {
            Ok(Some((recipient, question))) => s.events.publish(Event::Question {
                question,
                recipient,
            }),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "a thread's question not announced"),
        }
    }
}

/// Starts the grace for hook question `qid`: after `s.question_grace`, if
/// no poll has held it and no later grace has started meanwhile, an open
/// `qid` is withdrawn and announced, recording `question.withdraw` (reason
/// `unwaited`) as `system:daemon`. The grace thus runs from the last time a
/// poll let go (or from creation, when none has held it).
///
/// With a sleeper that leaves graces to the sweep
/// ([`Sleeper::times_graces`]), no timer runs: the grace is due at the
/// question clock's now plus the grace, and [`withdraw_due`] withdraws it
/// then.
pub fn start_grace(s: AppState, qid: String) {
    if !s.question_sleeper.times_graces() {
        let at = s.question_clock.now()
            + chrono::Duration::from_std(s.question_grace).unwrap_or_default();
        s.questions.arm_until(&qid, at);
        return;
    }
    let generation = s.questions.arm(&qid);
    clax_rt::spawn(async move {
        s.question_sleeper.sleep(s.question_grace).await;
        if !s.questions.due(&qid, generation) {
            return;
        }
        withdraw_unwaited(&s, qid).await;
    });
}

/// Withdraws every question whose swept grace is due now
/// ([`QuestionWaiters::take_due`]).
pub async fn withdraw_due(s: &AppState) {
    for qid in s.questions.take_due(s.question_clock.now()) {
        withdraw_unwaited(s, qid).await;
    }
}

/// Starts a grace for every open hook question no poll holds: what a host
/// whose polls did not outlive it (a Durable Object rebuilt after
/// eviction) does at start, so a question whose hook does not poll again
/// is withdrawn after the grace, as one let go is. A poll that comes back
/// cancels it.
///
/// # Errors
/// The store's.
pub async fn arm_open_hooks(s: &AppState) -> Result<(), crate::error::ApiError> {
    let ids = s.store_call(|st| st.open_hook_question_ids()).await?;
    for qid in ids {
        if s.questions.count(&qid) == 0 {
            start_grace(s.clone(), qid);
        }
    }
    Ok(())
}

/// Withdraws open hook question `qid` as unwaited and announces it.
async fn withdraw_unwaited(s: &AppState, qid: String) {
    {
        let st = s.clone();
        let r = s
            .store
            .call(move |db| {
                let q =
                    db.close_question(&clax_core::audit::AuditCtx::DAEMON, &qid, Close::Unwaited)?;
                let v = view(db, &q)?;
                announce(&st, db, &q, v);
                Ok(())
            })
            .await;
        match r {
            Ok(())
            | Err(CoreError::Invalid {
                code: "question_closed",
                ..
            }) => {}
            Err(e) => tracing::warn!(error = %e, "could not withdraw an unwaited question"),
        }
    }
}

/// [`start_grace`] for `qid`, to run when its last poll lets go.
pub fn arm_grace(s: AppState, qid: String) -> OnLast {
    Box::new(move || start_grace(s, qid))
}
