//! The broker's store over Clax's own ([`clax_store::Store`]):
//! [`StoredPending`] keeps the hub's calls, ended runs, registration
//! IDs and definitions in the tables the runs are in
//! (`crates/clax-store/src/bot_pending.rs`, `bot_surfaces.rs`), so they
//! outlast the process, an answer is journaled at its call's place by
//! whoever brings it, and a run's end is kept by the run's terminal
//! commit.
//!
//! Each method of [`Pending`] is one call of the store's:
//!
//! | The hub's | The store's |
//! |---|---|
//! | `issue_registration` | `issue_registration`, as [`StoredPending::registered_by`], under [`StoredPending::ctx`], with the prompt file and settings [`StoredPending::act_with`] was given (below) |
//! | `registration_name` | `registration`, its name |
//! | `keep_definition`, `definition_text` | `put_surface_def`, `surface_def` |
//! | `keep_prompt`, `prompt_text` | `put_surface_prompt`, `surface_prompt` |
//! | `open_call` through a broker ([`BrokerStore::open_placed`]) | `open_call`, under the claim of [`Placed::driver`], at the place |
//! | `pending_for`, `calls_for` | `pending_summary`, `calls_named` |
//! | `sent_calls`, `accept_call` | `mark_sent`, `accept_call` |
//! | `answer_call` | `answer_call`: the answer is journaled at the call's place with no claim held, and a parked run is made due |
//! | `expire_calls`, `next_deadline` | `expire_calls`, `next_call_deadline` |
//! | `record_end`, `ends_for`, `acknowledge_end`, `expire_ends` | `record_end`, `ends_named`, `ack_end`, `expire_ends` |
//! | [`BrokerStore::settled`] | `pending_call`, then the journal row at its place |
//!
//! What does not map one to one:
//!
//! - **Time.** The hub counts milliseconds since the epoch and the
//!   store writes stamps to the millisecond; [`at`] and [`millis`] are
//!   the two directions, and nothing else converts. A call with no
//!   deadline in the store reads as one that never comes.
//! - **`sent_calls` is given no instant.** The store wants when the
//!   calls were handed over; they are marked at the last instant the
//!   adapter was given, which is that `serve`'s opening.
//! - **A call's arguments.** The store's row keeps the arguments the
//!   host is sent (one value for each parameter, as the hub checked
//!   them) and the hash of the arguments as the script passed them,
//!   which is what the journal row of the place holds.
//! - **`open_call` without a place** cannot be made: the store's row
//!   is of one place of one attempt, opened under a claim. Only a
//!   broker opens calls ([`BrokerStore::open_placed`]).
//! - **A registration's prompt and settings.**
//!   [`Pending::issue_registration`] is given a name and a hash alone,
//!   so whoever registers gives the adapter the prompt file and the
//!   settings for that request ([`StoredPending::act_with`]), and the
//!   row is written with them, and brought up to date when the
//!   registration is gone on with ([`Pending::change_registration`]).
//!   The row names its prompt file by hash, and the store keeps the
//!   text under it in the same transaction. A run is not sent its
//!   registration's prompt file: it is sent the one its binding names
//!   by hash ([`Pending::prompt_text`]), which is the one its
//!   registration had when the run was started. The settings are read
//!   from the hub, which holds the ones a host last registered with
//!   for as long as it holds the registration.
//! - **Limits.** `key_reused` and `registration_full` are the store's
//!   errors and the hub's [`OpenedCall::KeyReused`] and
//!   [`OpenedCall::Full`].

use super::{BrokerStore, Placed};
use crate::bot_surfaces::{
    CallFailure, CallSummary, EndSummary, Millis, NewCall, NewEnd, OpenedCall, Pending,
    PendingCall, PendingEnd, PendingSummary, RecordedEnd,
};
use chrono::{DateTime, TimeZone, Utc};
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_store::Store;
use clax_store::bot_pending::{
    self as kept, AcceptOutcome, AnswerOutcome, CallAccess, EndFor, KeptEnd, Opened,
};
use clax_store::bot_surfaces::NewRegistration;
use clax_surface::wire::{CallKind, End};
use serde_json::Value;
use std::sync::Arc;

/// `ms` as the store writes an instant.
#[must_use]
pub fn at(ms: Millis) -> DateTime<Utc> {
    i64::try_from(ms.0)
        .ok()
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// A stamp of the store's as the hub counts instants; the epoch for
/// text that is no stamp.
#[must_use]
pub fn millis(stamp: &str) -> Millis {
    let ms = DateTime::parse_from_rfc3339(stamp).map_or(0, |t| t.timestamp_millis());
    Millis(u64::try_from(ms).unwrap_or(0))
}

/// A [`BrokerStore`] over [`Store`]. See the module documentation.
#[derive(Clone)]
pub struct StoredPending {
    store: Arc<Store>,
    /// Under whom a registration is recorded as issued
    /// (`bot.surface.registered`).
    pub ctx: AuditCtx,
    /// The person key of who registers, for a registration issued
    /// through this adapter.
    pub registered_by: String,
    /// The prompt file a registration issued through this adapter is
    /// recorded with ([`StoredPending::act_with`]).
    pub prompt: Option<String>,
    /// The settings a registration issued through this adapter is
    /// recorded with, as its registrant gave them; `null` for none.
    pub settings: Value,
    /// The last instant a method was given.
    seen: Millis,
}

impl std::fmt::Debug for StoredPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredPending")
            .field("registered_by", &self.registered_by)
            .finish_non_exhaustive()
    }
}

impl StoredPending {
    /// Over `store`, issuing registrations as the daemon itself
    /// ([`AuditCtx::DAEMON`], for the owner): what the run loop's side
    /// needs, which issues none. Whoever registers a surface for a
    /// person sets [`StoredPending::ctx`] and
    /// [`StoredPending::registered_by`] for that request
    /// ([`StoredPending::acting`]).
    #[must_use]
    pub fn new(store: Arc<Store>) -> StoredPending {
        StoredPending {
            store,
            ctx: AuditCtx::DAEMON,
            registered_by: clax_store::roles::OWNER.to_string(),
            prompt: None,
            settings: Value::Null,
            seen: Millis(0),
        }
    }

    /// This adapter, issuing registrations under `ctx` for the person
    /// `registered_by`.
    #[must_use]
    pub fn acting(mut self, ctx: AuditCtx, registered_by: impl Into<String>) -> StoredPending {
        self.ctx = ctx;
        self.registered_by = registered_by.into();
        self
    }

    /// Sets who registrations are issued for, in place
    /// ([`StoredPending::acting`]).
    pub fn act(&mut self, ctx: AuditCtx, registered_by: impl Into<String>) {
        self.ctx = ctx;
        self.registered_by = registered_by.into();
    }

    /// Sets who registrations are issued for ([`StoredPending::act`]),
    /// and the prompt file and settings the next one's row keeps: what
    /// its registrant sent with it. Whoever registers a surface sets
    /// them for that request and clears them after.
    pub fn act_with(
        &mut self,
        ctx: AuditCtx,
        registered_by: impl Into<String>,
        prompt: Option<String>,
        settings: Value,
    ) {
        self.act(ctx, registered_by);
        self.prompt = prompt;
        self.settings = settings;
    }

    /// The store.
    #[must_use]
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    fn saw(&mut self, now: Millis) -> DateTime<Utc> {
        self.seen = self.seen.max(now);
        at(now)
    }
}

fn kind_of(access: CallAccess) -> CallKind {
    match access {
        CallAccess::Read => CallKind::Read,
        CallAccess::Write => CallKind::Write,
        CallAccess::Effect => CallKind::Effect,
    }
}

fn access_of(kind: CallKind) -> CallAccess {
    match kind {
        CallKind::Read => CallAccess::Read,
        CallKind::Write => CallAccess::Write,
        CallKind::Effect => CallAccess::Effect,
    }
}

fn deadline(expires_at: Option<&str>) -> Millis {
    expires_at.map_or(Millis(u64::MAX), millis)
}

fn summary(call: &kept::CallSummary) -> CallSummary {
    CallSummary {
        id: call.id.clone(),
        definition: call.def_hash.clone(),
        op: call.op.clone(),
        kind: kind_of(call.access),
        deadline: deadline(call.expires_at.as_deref()),
        args_bytes: call.args_bytes,
        sent: call.sent,
        accepted: call.accepted,
    }
}

fn summary_of(call: &kept::PendingCall) -> CallSummary {
    CallSummary {
        id: call.id.clone(),
        definition: call.def_hash.clone(),
        op: call.op.clone(),
        kind: kind_of(call.access),
        deadline: deadline(call.expires_at.as_deref()),
        args_bytes: call.args.to_string().len(),
        sent: call.sent_at.is_some(),
        accepted: call.accepted_at.is_some(),
    }
}

/// What a pending call of the store's that was not answered by its
/// deadline is journaled as: the hub's rule
/// ([`crate::bot_surfaces::unanswered`]), for whoever ends such a wait
/// without a hub (a tick of a parked run whose deadline has come).
#[must_use]
pub fn deadline_failure(call: &kept::PendingCall) -> kept::CallFailure {
    failure_of(crate::bot_surfaces::unanswered(&summary_of(call)))
}

fn failure_of(failure: CallFailure) -> kept::CallFailure {
    kept::CallFailure {
        code: failure.code,
        message: failure.message,
        retryable: failure.retryable,
    }
}

impl Pending for StoredPending {
    type Error = CoreError;

    fn issue_registration(
        &mut self,
        id: &str,
        name: &str,
        hash: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        let now = self.saw(now);
        let new = NewRegistration {
            name: name.to_string(),
            def_hash: hash.to_string(),
            prompt: self.prompt.clone(),
            settings: self.settings.clone(),
            registered_by: self.registered_by.clone(),
            replaces: None,
        };
        Ok(self
            .store
            .issue_registration(&self.ctx, id, new, now)?
            .is_some())
    }

    fn registration_name(&mut self, id: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.store.registration(id)?.map(|r| r.name))
    }

    fn change_registration(
        &mut self,
        id: &str,
        hash: &str,
        now: Millis,
    ) -> Result<(), Self::Error> {
        let now = self.saw(now);
        self.store
            .change_registration(
                &self.ctx,
                id,
                hash,
                self.prompt.as_deref(),
                &self.settings,
                now,
            )
            .map(|_| ())
    }

    fn keep_definition(
        &mut self,
        hash: &str,
        canonical: &str,
        now: Millis,
    ) -> Result<(), Self::Error> {
        let now = self.saw(now);
        self.store.put_surface_def(hash, canonical, now).map(|_| ())
    }

    fn definition_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.store.surface_def(hash)?.map(|d| d.manifest))
    }

    fn keep_prompt(&mut self, hash: &str, text: &str, now: Millis) -> Result<(), Self::Error> {
        let now = self.saw(now);
        self.store.put_surface_prompt(hash, text, now).map(|_| ())
    }

    fn prompt_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        self.store.surface_prompt(hash)
    }

    fn open_call(
        &mut self,
        call: NewCall,
        _limit: usize,
        _now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        Err(CoreError::invalid(
            "call_unplaced",
            format!(
                "the call of {} for run {} has no place in a step: a call is opened by a broker, \
                 under its run's claim",
                call.op, call.run
            ),
        ))
    }

    fn pending_for(&mut self, registration: &str) -> Result<PendingSummary, Self::Error> {
        let pending = self.store.pending_summary(registration)?;
        Ok(PendingSummary {
            calls: pending.calls.iter().map(summary).collect(),
            ends: pending
                .ends
                .into_iter()
                .map(|e| EndSummary {
                    id: e.id,
                    result_bytes: e.result_bytes,
                })
                .collect(),
        })
    }

    fn calls_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingCall>, Self::Error> {
        Ok(self
            .store
            .calls_named(registration, ids)?
            .into_iter()
            .map(|call| PendingCall {
                summary: summary_of(&call),
                run: call.run_id,
                args: call.args.as_array().cloned().unwrap_or_default(),
                key: call.key,
            })
            .collect())
    }

    fn sent_calls(&mut self, ids: &[String]) -> Result<(), Self::Error> {
        self.store.mark_sent(ids, at(self.seen)).map(|_| ())
    }

    fn accept_call(
        &mut self,
        registration: &str,
        id: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        let now = self.saw(now);
        Ok(matches!(
            self.store.accept_call(id, registration, now)?,
            AcceptOutcome::Accepted | AcceptOutcome::AlreadyAccepted
        ))
    }

    fn answer_call(
        &mut self,
        registration: &str,
        id: &str,
        outcome: Result<Value, CallFailure>,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        let now = self.saw(now);
        let outcome = outcome.map_err(failure_of);
        Ok(matches!(
            self.store.answer_call(id, registration, outcome, now)?,
            AnswerOutcome::Journaled { .. }
        ))
    }

    fn expire_calls(
        &mut self,
        now: Millis,
        failure: &dyn Fn(&CallSummary) -> CallFailure,
    ) -> Result<Vec<String>, Self::Error> {
        let now = self.saw(now);
        let ended = self
            .store
            .expire_calls(now, &|call| failure_of(failure(&summary_of(call))))?;
        Ok(ended.into_iter().map(|call| call.id).collect())
    }

    fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error> {
        Ok(self.store.next_call_deadline()?.as_deref().map(millis))
    }

    fn record_end(
        &mut self,
        end: NewEnd,
        limit: usize,
        now: Millis,
    ) -> Result<RecordedEnd, Self::Error> {
        let now = self.saw(now);
        let kept = EndFor {
            registration_id: end.registration,
            end: serde_json::to_value(&end.end).unwrap_or(Value::Null),
            result: end.result,
        };
        Ok(
            match self.store.record_end(&end.run, &kept, Some(limit), now)? {
                KeptEnd::New(id) => RecordedEnd::New(id),
                KeptEnd::Same(id) => RecordedEnd::Same(id),
                KeptEnd::Full => RecordedEnd::Full,
            },
        )
    }

    fn ends_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingEnd>, Self::Error> {
        Ok(self
            .store
            .ends_named(registration, ids)?
            .into_iter()
            .map(|end| PendingEnd {
                id: end.id,
                run: end.run_id,
                end: serde_json::from_value::<End>(end.end.clone()).unwrap_or_else(|_| {
                    End::Other(end.end.as_object().cloned().unwrap_or_default())
                }),
                result: end.result,
            })
            .collect())
    }

    fn acknowledge_end(
        &mut self,
        registration: &str,
        id: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        let now = self.saw(now);
        self.store.ack_end(registration, id, now)
    }

    fn expire_ends(&mut self, before: Millis) -> Result<usize, Self::Error> {
        self.store.expire_ends(at(before))
    }
}

impl BrokerStore for StoredPending {
    fn open_placed(
        &mut self,
        placed: &Placed<'_>,
        call: NewCall,
        limit: usize,
        now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        let now = self.saw(now);
        let new = kept::NewCall {
            step_n: placed.place.step,
            seq: placed.place.seq,
            attempt: placed.place.attempt,
            registration_id: call.registration,
            def_hash: call.definition,
            op: call.op,
            args: Value::Array(call.args),
            args_hash: placed.args_hash.clone(),
            key: call.key,
            access: access_of(call.kind),
            waits: placed.waits,
            expires_at: Some(at(call.deadline)),
        };
        match self
            .store
            .open_call(placed.run, placed.driver, new, Some(limit), now)
        {
            Ok(Opened::New(call)) => Ok(OpenedCall::New(call.id)),
            Ok(Opened::Same(call)) => Ok(OpenedCall::Same(call.id)),
            Err(CoreError::Invalid {
                code: "key_reused", ..
            }) => Ok(OpenedCall::KeyReused),
            Err(CoreError::Invalid {
                code: "registration_full",
                ..
            }) => Ok(OpenedCall::Full),
            Err(e) => Err(e),
        }
    }

    fn commits_ends(&self) -> bool {
        true
    }

    fn settled(&mut self, id: &str) -> Result<Option<Result<Value, CallFailure>>, Self::Error> {
        let Some(call) = self.store.pending_call(id)? else {
            return Ok(None);
        };
        let row = self
            .store
            .bot_calls(&call.run_id, call.step_n)?
            .into_iter()
            .find(|row| row.seq() == call.seq);
        Ok(row.map(|row| {
            crate::surface::recorded(&row).map_err(|e| CallFailure {
                code: e.code,
                message: e.message,
                retryable: e.retryable,
            })
        }))
    }
}
