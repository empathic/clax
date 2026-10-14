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
/// `artifact` is null for none or a deleted one.
pub fn view(st: &Store, q: &QuestionRow) -> clax_core::Result<Value> {
    let agent = st.get_session(&q.session_id)?.map(|s| {
        json!({
            "handle": s.agent_handle,
            "name": s.name,
            "harness": s.harness,
            "project": std::path::Path::new(&s.cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(""),
        })
    });
    let artifact = match q.artifact_id.as_deref().map(ArtifactId::parse) {
        Some(Ok(id)) => st
            .get_artifact(&id)?
            .map(|a| json!({"id": a.id, "title": a.title, "kind": a.kind})),
        _ => None,
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
    }))
}

/// Publishes `view` (the view of `q`) as a `question` event to the
/// `questions` topic of the person whose agent asked it (read from `st`)
/// and wakes the polls of `q`; an answered or declined `ask` question also
/// wakes its session's feedback polls, which carry late answers (spec
/// §6.4).
pub fn announce(s: &AppState, st: &Store, q: &QuestionRow, view: Value) {
    match st.question_recipient(q) {
        Ok(recipient) => s.events.publish(Event::Question {
            question: view,
            recipient,
        }),
        Err(e) => tracing::warn!(error = %e, "a question's change not announced"),
    }
    s.questions.wake(&q.id);
    if q.source == Source::Ask && matches!(q.status, Status::Answered | Status::Declined) {
        s.feedback_waiters.wake(std::iter::once(&q.session_id));
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
