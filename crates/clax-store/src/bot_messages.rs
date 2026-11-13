//! A run's inbox (migration 65's `bot_messages`; spec
//! 2026-11-10-claxbot-surfaces-design, "A run's inbox"): messages sent
//! to a run after it started, by the registration that started it,
//! each given to the run's model once, between steps, in the order they
//! arrived.
//!
//! - **Order.** Each message of a run has the next `seq` of that run
//!   (1 for its first), taken in the transaction that appends it, so
//!   the order they arrived in is the order they are listed and
//!   delivered in ([`Store::undelivered_messages`]).
//! - **A key** names a message once for its run: the same key with the
//!   same body again is that message ([`Appended::Same`], nothing
//!   changes), with another body it is refused (`message_key_reused`).
//!   A message sent without one is a new message each time.
//! - **Bounds.** A run holds [`MessageBounds::undelivered`] messages
//!   not delivered yet at the most, and a message's body is
//!   [`MessageBounds::bytes`] as JSON at the most. Both are the
//!   caller's to choose; the store holds none.
//! - **Who and when.** Only the registration that started the run
//!   (`bot_runs.started_by`) may append to it, and only while the run
//!   has not ended. A run's kind decides whether it takes messages at
//!   all, and of what type: the store reads neither and checks
//!   neither.
//! - **Delivery is exactly once.** A message is marked delivered, with
//!   the step that gave it to the model, by that step's commit and in
//!   its transaction ([`crate::bot_runs::StepCommit::delivered`]). A
//!   step that is begun again after a crash finds the message still
//!   undelivered and gives it again, so it is given to the model by
//!   exactly one committed step.
//! - **A parked run is not woken.** A message to a run that waits on a
//!   call stays in its inbox, and the run stays parked: the call is
//!   ended only by its answer, its deadline or a cancel, as before
//!   there were messages. The message is given at the run's first turn
//!   after that ([`Appended::waiting`]).
//! - **Never given.** A run whose next step is not a turn (it finishes,
//!   gives up, is stopped or fails) ends with the messages it was not
//!   given still undelivered: [`Store::undelivered_messages`] lists them
//!   for as long as they are kept.
//! - **Kept** while the run's call journal is: [`Store::prune_ended_runs`]
//!   deletes the delivered messages of a run ended before
//!   [`crate::bot_runs::BOT_JOURNAL_KEEP`] with the journal, and those it
//!   was never given when its end is no longer kept for its registration
//!   ([`crate::bot_pending::BOT_END_KEEP`]), so the end can say which
//!   they were. A step that delivered one keeps its body in its own
//!   payload.

use super::Store;
use crate::bot_runs::RunStatus;
use clax_core::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

/// The bounds of a run's inbox: the caller's (the daemon's host
/// protocol limits), never the store's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageBounds {
    /// The messages a run may hold that have not been delivered.
    pub undelivered: usize,
    /// The bytes of one message's body, as compact JSON.
    pub bytes: usize,
}

/// A message to append ([`Store::append_message`]).
#[derive(Clone, Debug)]
pub struct NewMessage<'a> {
    pub run_id: &'a str,
    /// The registration that sends it: the run's `started_by`.
    pub registration_id: &'a str,
    pub body: &'a Value,
    /// Names the message once for its run.
    pub key: Option<&'a str>,
}

/// One message of a run's inbox.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    /// A ULID.
    pub id: String,
    pub run_id: String,
    /// Its place in the run's inbox, from 1, in the order messages
    /// arrived.
    pub seq: i64,
    pub registration_id: String,
    /// Its body, as its row keeps it: `Null` when the row's does not read
    /// as JSON, which [`Message::body`] refuses.
    body: Value,
    /// Why its row's body does not read as JSON, when it does not (a
    /// damaged row).
    #[serde(skip)]
    unreadable: Option<String>,
    pub key: Option<String>,
    pub created_at: String,
    /// When the step that gave it to the model committed; `None` until
    /// then.
    pub delivered_at: Option<String>,
    /// That step's number.
    pub delivered_step: Option<i64>,
}

impl Message {
    /// Its body.
    ///
    /// # Errors
    /// A conversion failure, naming the message, when its row's body does
    /// not read as JSON (a damaged row). Such a message is not given to a
    /// model: no step's commit marks it delivered ([`deliver_in`]), and it
    /// is listed undelivered ([`Store::undelivered_messages`]) for as long
    /// as it is kept.
    pub fn body(&self) -> Result<&Value> {
        match &self.unreadable {
            None => Ok(&self.body),
            Some(why) => Err(unreadable(&self.run_id, self.seq, why)),
        }
    }
}

/// The refusal of message `seq` of run `run`, whose body does not read
/// as JSON for `why`.
fn unreadable(run: &str, seq: i64, why: &str) -> CoreError {
    CoreError::Db(rusqlite::Error::FromSqlConversionFailure(
        4,
        rusqlite::types::Type::Text,
        format!("bot_messages.body_json of message {seq} of run {run} is not JSON: {why}").into(),
    ))
}

/// What [`Store::append_message`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct Appended {
    pub message: Message,
    /// The message is new; false when the run held one with its key and
    /// the same body, which is this, unchanged.
    pub new: bool,
    /// The run was parked, waiting on a call, when the message arrived:
    /// it is given at the run's first turn after the call is answered or
    /// its deadline comes, and cannot end the wait. A message sent again
    /// under its key is answered what it was first.
    pub waiting: bool,
}

/// The codes [`Store::append_message`] refuses with, as
/// [`CoreError::Invalid`]'s `code`.
pub const CODE_RUN_ENDED: &str = "run_ended";
pub const CODE_NOT_THE_STARTER: &str = "not_the_starter";
pub const CODE_KEY_REUSED: &str = "message_key_reused";
pub const CODE_TOO_MANY_MESSAGES: &str = "too_many_messages";
pub const CODE_MESSAGE_TOO_LARGE: &str = "message_too_large";

const COLS: &str = "id, run_id, seq, registration_id, body_json, key, created_at, delivered_at,
    delivered_step";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let body: String = r.get(4)?;
    let (body, unreadable) = match serde_json::from_str(&body) {
        Ok(body) => (body, None),
        Err(e) => (Value::Null, Some(e.to_string())),
    };
    Ok(Message {
        id: r.get(0)?,
        run_id: r.get(1)?,
        seq: r.get(2)?,
        registration_id: r.get(3)?,
        body,
        unreadable,
        key: r.get(5)?,
        created_at: r.get(6)?,
        delivered_at: r.get(7)?,
        delivered_step: r.get(8)?,
    })
}

fn by_key(c: &Connection, run: &str, key: &str) -> Result<Option<Message>> {
    Ok(c.prepare_cached(&format!(
        "SELECT {COLS} FROM bot_messages WHERE run_id = ?1 AND key = ?2"
    ))?
    .query_row(params![run, key], row)
    .optional()?)
}

fn invalid(code: &'static str, message: impl Into<String>) -> CoreError {
    CoreError::Invalid {
        code,
        message: message.into(),
    }
}

impl Store {
    /// Appends `new` to its run's inbox, with the run's next `seq`, and
    /// answers it; a key the run holds already answers that message as
    /// it was first answered, its [`Appended::waiting`] too (not
    /// [`Appended::new`]), when the body is the same, and changes
    /// nothing. A key is looked at before anything else but the run, so
    /// a message sent again is answered the same after its run has
    /// ended or its inbox filled.
    ///
    /// A message to a parked run leaves it parked: nothing of its run
    /// changes but its inbox ([`Appended::waiting`]). The message is
    /// given to the model at the run's next turn.
    ///
    /// # Errors
    /// `NotFound` for no such run; `not_the_starter` when
    /// `new.registration_id` did not start the run; `message_key_reused`
    /// for a key the run holds with another body; a conversion failure for
    /// a key the run holds with a body that does not read as JSON
    /// ([`Message::body`]); `run_ended` for a run
    /// that has ended; `message_too_large` for a body over
    /// `bounds.bytes`; `too_many_messages` when the run holds
    /// `bounds.undelivered` undelivered messages already; the store's.
    /// Each changes nothing.
    pub fn append_message(&self, new: NewMessage<'_>, bounds: MessageBounds) -> Result<Appended> {
        let body = new.body.to_string();
        let now = self.now();
        self.with_tx(|tx| {
            let run: Option<(Option<String>, String)> = tx
                .prepare_cached("SELECT started_by, status FROM bot_runs WHERE id = ?1")?
                .query_row(params![new.run_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let (started_by, status) = run.ok_or(CoreError::NotFound)?;
            if started_by.as_deref() != Some(new.registration_id) {
                return Err(invalid(
                    CODE_NOT_THE_STARTER,
                    format!(
                        "run {} was not started by registration {}",
                        new.run_id, new.registration_id
                    ),
                ));
            }
            if let Some(key) = new.key
                && let Some(held) = by_key(tx, new.run_id, key)?
            {
                if held.body()? == new.body {
                    // Answered as it was first: its `waiting` is what
                    // the run was when it arrived, kept with it.
                    let waiting: bool = tx
                        .prepare_cached("SELECT waiting FROM bot_messages WHERE id = ?1")?
                        .query_row(params![held.id], |r| r.get(0))?;
                    return Ok(Appended {
                        message: held,
                        new: false,
                        waiting,
                    });
                }
                return Err(invalid(
                    CODE_KEY_REUSED,
                    format!(
                        "run {} holds message {} under this key, with another body",
                        new.run_id, held.seq
                    ),
                ));
            }
            if RunStatus::parse(&status).is_none_or(RunStatus::is_terminal) {
                return Err(invalid(
                    CODE_RUN_ENDED,
                    format!("run {} has ended ({status})", new.run_id),
                ));
            }
            if body.len() > bounds.bytes {
                return Err(invalid(
                    CODE_MESSAGE_TOO_LARGE,
                    format!(
                        "a message is {} bytes as JSON at the most; this one is {}",
                        bounds.bytes,
                        body.len()
                    ),
                ));
            }
            let (waiting, last): (i64, i64) = tx.query_row(
                "SELECT COALESCE(sum(delivered_at IS NULL), 0), COALESCE(max(seq), 0)
                   FROM bot_messages WHERE run_id = ?1",
                params![new.run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if usize::try_from(waiting).unwrap_or(usize::MAX) >= bounds.undelivered {
                return Err(invalid(
                    CODE_TOO_MANY_MESSAGES,
                    format!(
                        "run {} holds {waiting} messages it has not been given yet, which is \
                         the limit: send this one when it has taken some",
                        new.run_id
                    ),
                ));
            }
            let id = clax_core::new_ulid();
            let parked = status == RunStatus::Parked.as_str();
            tx.execute(
                "INSERT INTO bot_messages (id, run_id, seq, registration_id, body_json, key,
                    created_at, waiting)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    new.run_id,
                    last + 1,
                    new.registration_id,
                    body,
                    new.key,
                    now,
                    parked
                ],
            )?;
            Ok(Appended {
                message: Message {
                    id,
                    run_id: new.run_id.to_string(),
                    seq: last + 1,
                    registration_id: new.registration_id.to_string(),
                    body: new.body.clone(),
                    unreadable: None,
                    key: new.key.map(str::to_string),
                    created_at: now.clone(),
                    delivered_at: None,
                    delivered_step: None,
                },
                new: true,
                waiting: parked,
            })
        })
    }

    /// The messages of run `id` not delivered yet, in the order they
    /// arrived, a message whose body does not read among them: its
    /// [`Message::body`] refuses, and whoever gives a run its messages
    /// leaves it undelivered and goes on with the rest.
    ///
    /// # Errors
    /// The store's.
    pub fn undelivered_messages(&self, id: &str) -> Result<Vec<Message>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLS} FROM bot_messages WHERE run_id = ?1 AND delivered_at IS NULL
                  ORDER BY seq"
            ))?;
            let rows = stmt.query_map(params![id], row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The end of run `id` kept for `registration` to be told
    /// (`bot_run_ends`, [`Store::commit_bot_step_on`]'s `ends`), whether
    /// or not it has acknowledged it, while it is kept
    /// ([`crate::bot_pending::BOT_END_KEEP`]): what a registration that
    /// asks where its run is reads of how it ended.
    ///
    /// # Errors
    /// A conversion failure when the kept end or result does not read as
    /// JSON (a damaged row), never an end read as none; the store's.
    pub fn kept_end(
        &self,
        id: &str,
        registration: &str,
    ) -> Result<Option<crate::bot_pending::RunEnd>> {
        self.with_read(|c| {
            Ok(c.prepare_cached(
                "SELECT id, run_id, registration_id, end_json, result_json, created_at,
                        delivered_at
                   FROM bot_run_ends WHERE run_id = ?1 AND registration_id = ?2
                  ORDER BY created_at DESC LIMIT 1",
            )?
            .query_row(params![id, registration], |r| {
                Ok(crate::bot_pending::RunEnd {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    registration_id: r.get(2)?,
                    end: crate::bot_runs::stored_json(r, 3, "bot_run_ends.end_json")?,
                    result: crate::bot_runs::stored_json_or_null(r, 4, "bot_run_ends.result_json")?,
                    created_at: r.get(5)?,
                    delivered_at: r.get(6)?,
                })
            })
            .optional()?)
        })
    }

    /// Every message of run `id`, delivered or not, in the order they
    /// arrived.
    ///
    /// # Errors
    /// The store's.
    pub fn messages_of(&self, id: &str) -> Result<Vec<Message>> {
        self.with_read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLS} FROM bot_messages WHERE run_id = ?1 ORDER BY seq"
            ))?;
            let rows = stmt.query_map(params![id], row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

/// Marks the messages `seqs` of run `run` delivered by step `step` at
/// `at`, in a step's commit ([`Store::commit_bot_step_on`]). A message
/// that is not undelivered (another commit took it, or it is not the
/// run's) is refused, and so the commit with it: a message is delivered
/// by one step. So is one whose body does not read as JSON
/// ([`Message::body`]): it is never marked delivered.
pub(crate) fn deliver_in(
    c: &Connection,
    run: &str,
    step: i64,
    seqs: &[i64],
    at: &str,
) -> Result<()> {
    let mut stmt = c.prepare_cached(
        "UPDATE bot_messages SET delivered_at = ?4, delivered_step = ?3
          WHERE run_id = ?1 AND seq = ?2 AND delivered_at IS NULL AND json_valid(body_json)",
    )?;
    for seq in seqs {
        if stmt.execute(params![run, seq, step, at])? != 1 {
            return Err(invalid(
                "run_invalid",
                format!(
                    "message {seq} of run {run} is not one waiting to be delivered with a body \
                     that reads"
                ),
            ));
        }
    }
    Ok(())
}

/// Deletes up to `limit` messages of run `run`, on `c` (a prune's
/// transaction, [`Store::prune_ended_runs`]): those delivered, and with
/// `all` those never delivered too. How many.
pub(crate) fn prune_run_in(c: &Connection, run: &str, all: bool, limit: usize) -> Result<usize> {
    Ok(c.prepare_cached(
        "DELETE FROM bot_messages WHERE rowid IN
           (SELECT rowid FROM bot_messages
             WHERE run_id = ?1 AND (?2 OR delivered_at IS NOT NULL) LIMIT ?3)",
    )?
    .execute(params![run, all, i64::try_from(limit).unwrap_or(i64::MAX)])?)
}

#[cfg(test)]
mod tests;
