//! Batch send to agent (spec §10 "Batch send"): several threads sent in one
//! transaction, as one batch with an optional note, delivered together.

use super::Store;
use super::feedback::{SendTarget, live_targets_of, send_in, send_target_value};
use crate::audit::AuditCtx;
use crate::feedback::{FeedbackBatch, Touched};
use crate::working::clean_line;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, params};

/// Most threads one batch holds (as many as one working record names).
pub const MAX_BATCH_THREADS: usize = crate::working::MAX_WORKING_THREADS;
/// Longest batch note, in characters, after whitespace is collapsed.
pub const MAX_BATCH_NOTE_CHARS: usize = 280;

#[derive(Clone, Debug)]
pub struct SendBatch {
    pub thread_ids: Vec<String>,
    pub note: Option<String>,
    /// The sender's display name, as a viewer comment's author.
    pub sent_by: String,
    /// The one agent to send to (a session ID): it becomes each thread's
    /// target. `None` sends to every live owner and watcher and clears each
    /// thread's target.
    pub to: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BatchResult {
    pub batch: FeedbackBatch,
    /// Threads that got new feedback rows (the batch), in request order.
    pub sent: Vec<String>,
    /// Threads already sent with nothing new to send.
    pub unchanged: Vec<String>,
    pub touched: Touched,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ThreadSend {
    pub batch_id: String,
    pub size: u32,
    pub note: Option<String>,
    pub sent_by: String,
    pub sent_at: String,
}

impl Store {
    /// Sends `b.thread_ids` (duplicates dropped, request order kept) as one
    /// batch, all or nothing. Errors, checked in this order before anything is
    /// written: `invalid_args` (none, more than [`MAX_BATCH_THREADS`], or not
    /// ULIDs), `note_too_long`, `unknown_agent` (`to` is not a live owner or
    /// watcher of the artifact), `unknown_thread` (missing or on another
    /// artifact; the message names them), `thread_resolved` (names them), and
    /// `nothing_to_send` when no thread has a viewer comment without a row.
    /// Already-sent threads are accepted: they send only what they have not
    /// sent (as [`Store::send_to_agent`]) and are reported `unchanged` when
    /// that is nothing.
    ///
    /// The batch is recorded as one `thread.send` under `ctx`, in its
    /// transaction: whom it went to, every row it made, the batch, and the
    /// threads it sent.
    pub fn send_batch(
        &self,
        ctx: &AuditCtx,
        aid: &ArtifactId,
        b: SendBatch,
    ) -> Result<BatchResult> {
        let mut ids: Vec<String> = Vec::new();
        for t in b.thread_ids {
            if !ids.contains(&t) {
                ids.push(t);
            }
        }
        if ids.is_empty() || ids.len() > MAX_BATCH_THREADS {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("send 1 to {MAX_BATCH_THREADS} threads"),
            ));
        }
        if let Some(bad) = ids.iter().find(|t| !crate::is_ulid(t)) {
            return Err(CoreError::invalid(
                "invalid_args",
                format!("'{bad}' is not a thread ID"),
            ));
        }
        let note = match b.note.as_deref() {
            Some(n) => match clean_line(n, MAX_BATCH_NOTE_CHARS) {
                (_, true) => {
                    return Err(CoreError::invalid(
                        "note_too_long",
                        format!("a note is at most {MAX_BATCH_NOTE_CHARS} characters"),
                    ));
                }
                (n, false) => n,
            },
            None => None,
        };
        let to = b.to;
        self.with_tx(|tx| {
            if let Some(sid) = to.as_deref()
                && !live_targets_of(tx, aid.as_str())?.iter().any(|s| s == sid)
            {
                return Err(CoreError::invalid("unknown_agent", "no live agent on this artifact has that handle"));
            }
            let mut unknown = Vec::new();
            let mut resolved = Vec::new();
            for t in &ids {
                let row: Option<(String, String)> = tx
                    .query_row("SELECT artifact_id, status FROM threads WHERE id = ?1", params![t], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                match row {
                    Some((a, _)) if a != aid.as_str() => unknown.push(t.clone()),
                    None => unknown.push(t.clone()),
                    Some((_, s)) if s == "resolved" => resolved.push(t.clone()),
                    Some(_) => {}
                }
            }
            if !unknown.is_empty() {
                return Err(CoreError::invalid("unknown_thread", format!("not threads of {aid}: {}", unknown.join(", "))));
            }
            if !resolved.is_empty() {
                return Err(CoreError::invalid(
                    "thread_resolved",
                    format!("resolved threads cannot be sent: {}", resolved.join(", ")),
                ));
            }
            let target = to.as_deref().map_or(SendTarget::Everyone, SendTarget::Agent);
            let batch_id = new_ulid();
            let mut touched = Touched::default();
            let (mut sent, mut unchanged) = (Vec::new(), Vec::new());
            let mut feedback_ids = Vec::new();
            for t in &ids {
                let rows = send_in(tx, t, Some(&batch_id), target, &mut touched)?.feedback_ids;
                if rows.is_empty() {
                    unchanged.push(t.clone());
                } else {
                    sent.push(t.clone());
                    feedback_ids.extend(rows);
                }
            }
            if sent.is_empty() {
                return Err(CoreError::invalid("nothing_to_send", "every thread was already sent with nothing new"));
            }
            tx.execute(
                "INSERT INTO send_batches (id, artifact_id, note, sent_by, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![batch_id, aid.as_str(), note, b.sent_by, sent.len() as u32, Store::now()],
            )?;
            for t in &sent {
                tx.execute("INSERT INTO batch_threads (batch_id, thread_id) VALUES (?1, ?2)", params![batch_id, t])?;
            }
            let rec = super::feedback::send_record(
                &Store::now(),
                aid.as_str(),
                None,
                send_target_value(tx, to.as_deref())?,
                feedback_ids,
                Some(&batch_id),
                sent.clone(),
            );
            self.record_audit(tx, ctx, rec)?;
            let batch = FeedbackBatch { id: batch_id, size: sent.len() as u32, note, sent_by: b.sent_by };
            Ok(BatchResult { batch, sent, unchanged, touched })
        })
    }

    /// The batches that sent `thread_id`, oldest first (its send history).
    pub fn thread_sends(&self, thread_id: &str) -> Result<Vec<ThreadSend>> {
        self.with_read(|c| {
            let mut stmt = c.prepare(
                "SELECT b.id, b.size, b.note, b.sent_by, b.created_at FROM batch_threads t JOIN send_batches b ON b.id = t.batch_id
                 WHERE t.thread_id = ?1 ORDER BY b.created_at, b.id",
            )?;
            Ok(stmt
                .query_map(params![thread_id], |r| {
                    Ok(ThreadSend { batch_id: r.get(0)?, size: r.get(1)?, note: r.get(2)?, sent_by: r.get(3)?, sent_at: r.get(4)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::DAEMON;
    use crate::store::test_util::{anchor, artifact, session, store};
    use crate::{NewThread, Store, TakeFeedback, Tier};

    fn thread(st: &Store, id: &ArtifactId, body: &str) -> String {
        st.create_thread(
            DAEMON,
            id,
            NewThread {
                version_n: 1,
                anchor: anchor(),
                author_name: "Alex".into(),
                author_public_id: None,
                body: body.into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }
    fn batch(ids: &[&String], note: Option<&str>) -> SendBatch {
        SendBatch {
            thread_ids: ids.iter().map(|s| s.to_string()).collect(),
            note: note.map(String::from),
            sent_by: "Alex".into(),
            to: None,
        }
    }
    fn code(e: crate::CoreError) -> &'static str {
        match e {
            crate::CoreError::Invalid { code, .. } => code,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_batch_sends_every_thread_in_one_transaction_and_one_delivery() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "b1");
        let id = artifact(&st, Some(&sid));
        st.ensure_watch(DAEMON, &sid, &id).unwrap();
        let ts: Vec<String> = (0..3).map(|i| thread(&st, &id, &format!("c{i}"))).collect();
        let r = st
            .send_batch(
                DAEMON,
                &id,
                batch(&ts.iter().collect::<Vec<_>>(), Some("  Before the demo ")),
            )
            .unwrap();
        assert_eq!(r.sent, ts);
        assert_eq!(
            (r.batch.size, r.batch.note.as_deref()),
            (3, Some("Before the demo"))
        );
        for t in &ts {
            assert!(st.get_thread(t).unwrap().unwrap().sent_to_agent);
            assert_eq!(st.thread_sends(t).unwrap()[0].size, 3);
        }
        let (items, _) = st
            .take_feedback(
                DAEMON,
                &TakeFeedback {
                    session_id: sid.clone(),
                    tier: Tier::Piggyback,
                    artifact_id: None,
                    include_resends: true,
                },
                "http://h",
            )
            .unwrap();
        assert_eq!(items.len(), 3);
        assert!(
            items
                .iter()
                .all(|i| i.batch.as_ref().map(|b| b.id.as_str()) == Some(r.batch.id.as_str()))
        );
    }

    #[test]
    fn any_bad_thread_fails_the_whole_batch_and_writes_nothing() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let other = artifact(&st, None);
        let (a, b) = (thread(&st, &id, "a"), thread(&st, &id, "b"));
        let foreign = thread(&st, &other, "x");
        assert_eq!(
            code(
                st.send_batch(DAEMON, &id, batch(&[&a, &foreign], None))
                    .unwrap_err()
            ),
            "unknown_thread"
        );
        let gone = crate::new_ulid();
        assert_eq!(
            code(
                st.send_batch(DAEMON, &id, batch(&[&a, &gone], None))
                    .unwrap_err()
            ),
            "unknown_thread"
        );
        st.resolve_thread(DAEMON, &b, "viewer:anonymous").unwrap();
        assert_eq!(
            code(
                st.send_batch(DAEMON, &id, batch(&[&a, &b], None))
                    .unwrap_err()
            ),
            "thread_resolved"
        );
        assert!(
            !st.get_thread(&a).unwrap().unwrap().sent_to_agent,
            "nothing was written"
        );
        assert!(st.thread_sends(&a).unwrap().is_empty());
        assert_eq!(
            code(st.send_batch(DAEMON, &id, batch(&[], None)).unwrap_err()),
            "invalid_args"
        );
        let many: Vec<String> = (0..21).map(|_| crate::new_ulid()).collect();
        assert_eq!(
            code(
                st.send_batch(DAEMON, &id, batch(&many.iter().collect::<Vec<_>>(), None))
                    .unwrap_err()
            ),
            "invalid_args"
        );
        assert_eq!(
            code(
                st.send_batch(DAEMON, &id, batch(&[&a], Some(&"n".repeat(281))))
                    .unwrap_err()
            ),
            "note_too_long"
        );
    }

    #[test]
    fn already_sent_threads_are_accepted_and_reported_unchanged() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (a, b) = (thread(&st, &id, "a"), thread(&st, &id, "b"));
        st.send_to_agent(DAEMON, &a).unwrap();
        let r = st
            .send_batch(DAEMON, &id, batch(&[&a, &b, &b], None))
            .unwrap();
        assert_eq!((r.sent, r.unchanged), (vec![b.clone()], vec![a.clone()]));
        assert_eq!(r.batch.size, 1);
        assert_eq!(
            code(
                st.send_batch(DAEMON, &id, batch(&[&a, &b], None))
                    .unwrap_err()
            ),
            "nothing_to_send"
        );
    }

    #[test]
    fn deleting_a_thread_drops_it_from_its_batches() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let a = thread(&st, &id, "a");
        st.send_batch(DAEMON, &id, batch(&[&a], None)).unwrap();
        st.delete_thread(DAEMON, &a).unwrap();
        assert!(st.thread_sends(&a).unwrap().is_empty());
    }
}
