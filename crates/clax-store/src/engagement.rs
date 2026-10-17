//! Where an agent session's person last engaged (spec
//! 2026-10-16-ask-where-attention-is-design §1): their last prompt to it in
//! the terminal, typed or inferred from a turn, and its last delivery from
//! Clax. Every time is the store's clock's, so neither a hook's machine's
//! clock nor a browser's ever enters a comparison.

use super::Store;
use super::questions::Close;
use clax_core::audit::AuditCtx;
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// What the store knows of where a session's person last engaged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSignals {
    /// The person's last prompt to the session in the terminal.
    pub terminal_at: Option<String>,
    /// `terminal_at` stands for a turn with no Clax delivery behind it,
    /// not a prompt the harness reported.
    pub terminal_inferred: bool,
    /// The session's last delivery from Clax: a thread, an answer or an
    /// "Ask me here" sent to it.
    pub delivered_at: Option<String>,
    pub delivered_artifact_id: Option<String>,
    pub delivered_thread_id: Option<String>,
}

fn unknown_session() -> CoreError {
    CoreError::invalid("unknown_session", "no live session has this ID")
}

impl Store {
    /// Records that the person typed a prompt to live session `sid` in the
    /// terminal now, and withdraws its open hook in-terminal questions under
    /// `ctx` (`question.withdraw`, reason `lapsed`): a terminal dialog that
    /// took no answer (Esc) is gone once the person types again. Returns the
    /// IDs withdrawn, for the caller to announce.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session.
    pub fn note_terminal(&self, ctx: &AuditCtx, sid: &str) -> Result<Vec<String>> {
        let now = self.now();
        let n = self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE sessions SET terminal_at = ?2, terminal_inferred = 0
                 WHERE id = ?1 AND ended_at IS NULL",
                params![sid, now],
            )?)
        })?;
        if n == 0 {
            return Err(unknown_session());
        }
        let open: Vec<String> = self.with_read(|c| {
            let mut s = c.prepare_cached(
                "SELECT id FROM questions WHERE session_id = ?1 AND status = 'open'
                 AND in_terminal = 1 AND source = 'hook' ORDER BY id",
            )?;
            Ok(s.query_map(params![sid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        let mut lapsed = Vec::with_capacity(open.len());
        for id in open {
            match self.close_question(ctx, &id, Close::Lapse) {
                Ok(_) => lapsed.push(id),
                // Answered or withdrawn in the meantime.
                Err(CoreError::Invalid {
                    code: "question_closed",
                    ..
                }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(lapsed)
    }

    /// A turn of live session `sid` ended now, in a harness that reports no
    /// prompts (Codex, Grok Build). When no Clax delivery reached the
    /// session since its previous turn ended (for its first turn: at all),
    /// the turn stands for a terminal prompt, recorded as inferred. Returns
    /// whether it did. A delivery sets a flag that the next turn clears
    /// ([`Store::note_delivery`]), so a delivery stamped in the same
    /// millisecond as the turn before it still counts.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session.
    pub fn note_turn(&self, sid: &str) -> Result<bool> {
        let now = self.now();
        self.with_tx(|tx| {
            let delivered: Option<bool> = tx
                .query_row(
                    "SELECT delivered_since_turn FROM sessions WHERE id = ?1 AND ended_at IS NULL",
                    params![sid],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(delivered) = delivered else {
                return Err(unknown_session());
            };
            let from_terminal = !delivered;
            if from_terminal {
                tx.execute(
                    "UPDATE sessions SET terminal_at = ?2, terminal_inferred = 1, turn_at = ?2
                     WHERE id = ?1",
                    params![sid, now],
                )?;
            } else {
                tx.execute(
                    "UPDATE sessions SET turn_at = ?2, delivered_since_turn = 0 WHERE id = ?1",
                    params![sid, now],
                )?;
            }
            Ok(from_terminal)
        })
    }

    /// Records a delivery from Clax to session `sid` now, about artifact
    /// `aid` and, when given, its thread `tid`. An ended or unknown session
    /// records nothing.
    pub fn note_delivery(&self, sid: &str, aid: &str, tid: Option<&str>) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE sessions SET delivered_at = ?2, delivered_artifact_id = ?3,
                    delivered_thread_id = ?4, delivered_since_turn = 1
                 WHERE id = ?1 AND ended_at IS NULL",
                params![sid, now, aid, tid],
            )?;
            Ok(())
        })
    }

    /// Session `sid`'s signals; `None` when it does not exist.
    pub fn session_signals(&self, sid: &str) -> Result<Option<SessionSignals>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT terminal_at, terminal_inferred, delivered_at, delivered_artifact_id,
                    delivered_thread_id FROM sessions WHERE id = ?1",
                params![sid],
                |r| {
                    Ok(SessionSignals {
                        terminal_at: r.get(0)?,
                        terminal_inferred: r.get(1)?,
                        delivered_at: r.get(2)?,
                        delivered_artifact_id: r.get(3)?,
                        delivered_thread_id: r.get(4)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// The owner's public ID when the owner exists; never makes the owner
    /// (unlike [`Store::owner_viewer`]).
    pub fn owner_public_id(&self) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(
                c.query_row("SELECT public_id FROM viewers WHERE owner = 1", [], |r| {
                    r.get(0)
                })
                .optional()?,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::{NewQuestion, Source, Status};
    use crate::test_util::{DAEMON, session, store_with_clock};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn clocked() -> (Arc<ManualClock>, tempfile::TempDir, Store) {
        let c = Arc::new(ManualClock::at("2026-10-16T10:00:00Z"));
        let (d, st) = store_with_clock(c.clone());
        (c, d, st)
    }

    fn new_question(
        st: &Store,
        sid: &str,
        source: Source,
        t: Option<&str>,
        in_terminal: bool,
    ) -> String {
        let qs =
            serde_json::from_value(serde_json::json!([{"question": "Which?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]))
            .unwrap();
        st.create_question(
            DAEMON,
            NewQuestion {
                session_id: sid.into(),
                artifact_id: None,
                source,
                tool_use_id: t.map(Into::into),
                questions: qs,
                thread_id: None,
                in_terminal,
                released: false,
            },
        )
        .unwrap()
        .0
        .id
    }

    fn hook_in_terminal(st: &Store, sid: &str, t: &str) -> String {
        new_question(st, sid, Source::Hook, Some(t), true)
    }

    #[test]
    fn a_prompt_and_a_delivery_are_stamped_by_the_store_clock() {
        let (c, _d, st) = clocked();
        let sid = session(&st, "claude", "h1");
        assert_eq!(
            st.session_signals(&sid).unwrap().unwrap(),
            SessionSignals::default()
        );
        assert!(st.note_terminal(DAEMON, &sid).unwrap().is_empty());
        c.advance(30);
        st.note_delivery(&sid, "7q3k9mzx2b4t", Some("t1")).unwrap();
        assert_eq!(
            st.session_signals(&sid).unwrap().unwrap(),
            SessionSignals {
                terminal_at: Some("2026-10-16T10:00:00.000Z".into()),
                terminal_inferred: false,
                delivered_at: Some("2026-10-16T10:00:30.000Z".into()),
                delivered_artifact_id: Some("7q3k9mzx2b4t".into()),
                delivered_thread_id: Some("t1".into()),
            }
        );
        assert_eq!(st.session_signals("nope").unwrap(), None);
    }

    #[test]
    fn a_prompt_lapses_the_sessions_open_hook_in_terminal_questions_only() {
        let (_c, _d, st) = clocked();
        let sid = session(&st, "claude", "h1");
        let other = session(&st, "claude", "h2");
        let mine = hook_in_terminal(&st, &sid, "t1");
        let theirs = hook_in_terminal(&st, &other, "t1");
        let plain = new_question(&st, &sid, Source::Hook, Some("t2"), false);
        let asked = new_question(&st, &sid, Source::Ask, None, true);
        assert_eq!(st.note_terminal(DAEMON, &sid).unwrap(), vec![mine.clone()]);
        assert_eq!(
            st.question(&mine).unwrap().unwrap().status,
            Status::Withdrawn
        );
        assert_eq!(st.question(&theirs).unwrap().unwrap().status, Status::Open);
        for q in [&plain, &asked] {
            assert_eq!(st.question(q).unwrap().unwrap().status, Status::Open);
        }
    }

    #[test]
    fn a_turn_with_no_delivery_since_the_last_stands_for_a_prompt() {
        let (c, _d, st) = clocked();
        let sid = session(&st, "codex", "h1");
        assert!(st.note_turn(&sid).unwrap(), "a first turn with no delivery");
        let s = st.session_signals(&sid).unwrap().unwrap();
        assert_eq!(
            (s.terminal_at.as_deref(), s.terminal_inferred),
            (Some("2026-10-16T10:00:00.000Z"), true)
        );
        c.advance(10);
        st.note_delivery(&sid, "7q3k9mzx2b4t", None).unwrap();
        c.advance(10);
        assert!(!st.note_turn(&sid).unwrap(), "that turn came from Clax");
        c.advance(10);
        assert!(st.note_turn(&sid).unwrap(), "nothing came from Clax since");
        assert_eq!(
            st.session_signals(&sid)
                .unwrap()
                .unwrap()
                .terminal_at
                .as_deref(),
            Some("2026-10-16T10:00:30.000Z")
        );
        // A typed prompt clears the inference.
        st.note_terminal(DAEMON, &sid).unwrap();
        assert!(!st.session_signals(&sid).unwrap().unwrap().terminal_inferred);
    }

    #[test]
    fn a_turn_after_a_delivery_in_the_same_millisecond_came_from_clax() {
        let (_c, _d, st) = clocked();
        let sid = session(&st, "codex", "h1");
        assert!(st.note_turn(&sid).unwrap());
        // One feedback poll: the turn, then the items it hands over.
        st.note_delivery(&sid, "7q3k9mzx2b4t", Some("t1")).unwrap();
        assert!(!st.note_turn(&sid).unwrap(), "that turn came from Clax");
        assert!(
            st.note_turn(&sid).unwrap(),
            "no delivery since the last turn"
        );
    }

    #[test]
    fn an_ended_session_records_nothing() {
        let (_c, _d, st) = clocked();
        let sid = session(&st, "claude", "h1");
        st.end_session(DAEMON, &sid).unwrap();
        assert!(matches!(
            st.note_terminal(DAEMON, &sid),
            Err(CoreError::Invalid {
                code: "unknown_session",
                ..
            })
        ));
        assert!(st.note_turn(&sid).is_err());
        st.note_delivery(&sid, "7q3k9mzx2b4t", None).unwrap();
        assert_eq!(
            st.session_signals(&sid).unwrap().unwrap().delivered_at,
            None
        );
    }

    #[test]
    fn the_owners_public_id_is_read_and_never_made() {
        let (_c, _d, st) = clocked();
        assert_eq!(st.owner_public_id().unwrap(), None);
        let v = st.owner_viewer(false).unwrap();
        assert_eq!(st.owner_public_id().unwrap(), Some(v.public_id));
    }
}
