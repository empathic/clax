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
//!   registration had when the run was started. A run keeps the
//!   settings it was started with in its binding, likewise; for a
//!   binding that keeps none (one written before they were kept), the
//!   settings are the hub's, which holds the ones a host last
//!   registered with for as long as it holds the registration, and the
//!   row's ([`Pending::registration_settings`]) while it does not.
//! - **Limits.** `key_reused` and `registration_full` are the store's
//!   errors and the hub's [`OpenedCall::KeyReused`] and
//!   [`OpenedCall::Full`].

use super::{BrokerStore, Placed};
use crate::bot_surfaces::{
    Answered, CallFailure, CallSummary, EndSummary, Millis, NewCall, NewEnd, OpenedCall, Pending,
    PendingCall, PendingEnd, PendingSummary, RecordedEnd, Resend, Retry, Tries,
};
use chrono::{DateTime, TimeZone, Utc};
use clax_core::CoreError;
use clax_core::audit::AuditCtx;
use clax_store::Store;
use clax_store::bot_content::ContentRow;
use clax_store::bot_pending::{
    self as kept, AcceptOutcome, AnswerOutcome, CallAccess, EndFor, KeptEnd, Opened,
};
use clax_store::bot_surfaces::NewRegistration;
use clax_surface::wire::{CallKind, End};
use serde_json::Value;
use std::sync::Arc;

/// `ms` as the store writes an instant: no later than the latest it
/// keeps ([`kept::latest_stamp`], the end of the year 9999), whose stamps
/// sort as instants do.
#[must_use]
pub fn at(ms: Millis) -> DateTime<Utc> {
    let latest = kept::latest_stamp();
    i64::try_from(ms.0)
        .ok()
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
        .map_or(latest, |t| t.min(latest))
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
    /// What run `run` is on, as the host of `registration` is told it
    /// with each call ([`clax_surface::wire::Call::joined`]): the scope of
    /// the run's binding that `registration` serves, when a join bound
    /// that surface into a run of another surface's kind; `None` for a
    /// run of the surface's own kind, one whose bindings do not read, and
    /// one the store no longer has. Read from the run's bindings, which
    /// keep it for as long as the run is, so a call the store holds over
    /// a restart is sent it too.
    fn joined_of(&self, run: &str, registration: &str) -> Result<Option<Value>, CoreError> {
        let Some((_, made)) = self.store.run_as_made(run)? else {
            return Ok(None);
        };
        let Ok(bindings) = made.bindings() else {
            return Ok(None);
        };
        let Some(binding) = bindings
            .all()
            .iter()
            .find(|b| b.registration() == Some(registration))
        else {
            return Ok(None);
        };
        let declares = made
            .kind
            .split_once('.')
            .is_some_and(|(surface, _)| surface == binding.surface());
        Ok((!declares && !binding.scope().is_null()).then(|| binding.scope().clone()))
    }

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

    /// Where the call `id` is with its attempts, as the hub reads them
    /// from the store: `None` for a call that is settled, or not held.
    ///
    /// # Errors
    /// The store's.
    pub fn tries(&self, id: &str) -> Result<Option<Tries>, CoreError> {
        Ok(self
            .store
            .pending_call(id)?
            .filter(|call| call.answered_at.is_none())
            .map(|call| tries_of(&call.tries)))
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

fn rule_of(rule: kept::RetryRule) -> Retry {
    Retry {
        attempts: rule.attempts,
        backoff_ms: rule.backoff_ms,
        backoff_max_ms: rule.backoff_max_ms,
    }
}

fn kept_rule(rule: Retry) -> kept::RetryRule {
    kept::RetryRule {
        attempts: rule.attempts,
        backoff_ms: rule.backoff_ms,
        backoff_max_ms: rule.backoff_max_ms,
    }
}

/// A call's attempts as the hub counts them: `not_before` is the epoch
/// for none.
fn tries_of(tries: &kept::CallTries) -> Tries {
    Tries {
        rule: tries.rule.map(rule_of),
        failures: tries.failures,
        not_before: tries.not_before.as_deref().map_or(Millis(0), millis),
        resend: match tries.resend {
            kept::Resend::None => Resend::None,
            kept::Resend::Planned => Resend::Planned,
            kept::Resend::Due => Resend::Due,
        },
        later: tries.later,
        last: tries.last.clone().map(|f| CallFailure {
            code: f.code,
            message: f.message,
            retryable: f.retryable,
        }),
    }
}

/// A call's attempts as the store keeps them: no `not_before` for the
/// epoch.
fn kept_tries(tries: &Tries) -> kept::CallTries {
    kept::CallTries {
        rule: tries.rule.map(kept_rule),
        failures: tries.failures,
        not_before: (tries.not_before > Millis(0))
            .then(|| clax_core::time::stamp(at(tries.not_before))),
        resend: match tries.resend {
            Resend::None => kept::Resend::None,
            Resend::Planned => kept::Resend::Planned,
            Resend::Due => kept::Resend::Due,
        },
        later: tries.later,
        last: tries.last.clone().map(failure_of),
    }
}

fn summary(call: &kept::CallSummary) -> CallSummary {
    CallSummary {
        id: call.id.clone(),
        run: call.run_id.clone(),
        definition: call.def_hash.clone(),
        op: call.op.clone(),
        kind: kind_of(call.access),
        deadline: deadline(call.expires_at.as_deref()),
        args_bytes: call.args_bytes,
        sent: call.sent,
        accepted: call.accepted,
        tries: tries_of(&call.tries),
    }
}

fn summary_of(call: &kept::PendingCall) -> CallSummary {
    CallSummary {
        id: call.id.clone(),
        run: call.run_id.clone(),
        definition: call.def_hash.clone(),
        op: call.op.clone(),
        kind: kind_of(call.access),
        deadline: deadline(call.expires_at.as_deref()),
        args_bytes: call.args.to_string().len(),
        sent: call.sent_at.is_some(),
        accepted: call.accepted_at.is_some(),
        tries: tries_of(&call.tries),
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

    /// The settings registration `id`'s row keeps; an error for a row
    /// whose settings do not read ([`clax_store::bot_surfaces::Registration::settings_unread`]),
    /// so the registration is not served.
    fn registration_settings(&mut self, id: &str) -> Result<Option<Value>, Self::Error> {
        match self.store.registration(id)? {
            Some(r) => match r.settings_unread {
                Some(why) => Err(CoreError::Invalid {
                    code: "registration_settings_unread",
                    message: format!("the settings registration {id} keeps do not read: {why}"),
                }),
                None => Ok(Some(r.settings)),
            },
            None => Ok(None),
        }
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

    /// The calls named that read. Those that do not (their arguments,
    /// as kept, are not a JSON array) are settled `internal_error` here
    /// ([`Store::settle_unreadable_calls`]): no host is sent a call
    /// other than the one its script made.
    fn calls_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingCall>, Self::Error> {
        let calls = self.store.calls_named(registration, ids)?;
        if calls.len() < ids.len() {
            let missing: Vec<String> = ids
                .iter()
                .filter(|id| !calls.iter().any(|c| &c.id == *id))
                .cloned()
                .collect();
            for ended in
                self.store
                    .settle_unreadable_calls(registration, &missing, at(self.seen))?
            {
                tracing::warn!(
                    "clax-bot: call {} of run {} does not read as the store keeps it: settled \
                     internal_error, and sent to no host",
                    ended.id,
                    ended.run_id
                );
            }
        }
        let mut joined: std::collections::BTreeMap<String, Option<Value>> =
            std::collections::BTreeMap::new();
        let mut out = Vec::with_capacity(calls.len());
        for call in calls {
            if !joined.contains_key(&call.run_id) {
                let of = self.joined_of(&call.run_id, registration)?;
                joined.insert(call.run_id.clone(), of);
            }
            let Value::Array(args) = call.args.clone() else {
                return Err(CoreError::invalid(
                    "storage_unread",
                    format!("the arguments of call {} are not a JSON array", call.id),
                ));
            };
            out.push(PendingCall {
                summary: summary_of(&call),
                joined: joined.get(&call.run_id).cloned().flatten(),
                run: call.run_id,
                args,
                key: call.key,
            });
        }
        Ok(out)
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
    ) -> Result<Answered, Self::Error> {
        let now = self.saw(now);
        let outcome = outcome.map_err(failure_of);
        Ok(
            match self.store.answer_call(id, registration, outcome, now)? {
                AnswerOutcome::Journaled { .. } => Answered::Settled,
                AnswerOutcome::AlreadyJournaled => Answered::AlreadySettled,
                AnswerOutcome::Ignored(_) => Answered::Ignored,
            },
        )
    }

    fn keep_tries(
        &mut self,
        registration: &str,
        id: &str,
        tries: &Tries,
    ) -> Result<bool, Self::Error> {
        self.store
            .keep_call_tries(registration, id, &kept_tries(tries))
    }

    fn resends(&mut self, run: &str, registration: &str) -> Result<u32, Self::Error> {
        self.store.call_resends(run, registration)
    }

    fn next_resend(&mut self, after: Millis) -> Result<Option<Millis>, Self::Error> {
        Ok(self
            .store
            .next_call_resend(at(after))?
            .as_deref()
            .map(millis))
    }

    fn set_dropped(&mut self, id: &str, when: Option<Millis>) -> Result<(), Self::Error> {
        let when = when.map(|t| self.saw(t));
        self.store.set_registration_dropped(id, when).map(|_| ())
    }

    fn is_dropped(&mut self, id: &str) -> Result<bool, Self::Error> {
        self.store.registration_dropped(id)
    }

    /// When the registration was last served, written as its `serve` is
    /// taken: a host whose client gives up on a held `serve` is served
    /// all the same ([`Store::touch_registration`]).
    fn served(&mut self, id: &str, now: Millis) -> Result<(), Self::Error> {
        let now = self.saw(now);
        self.store.touch_registration(id, now).map(|_| ())
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
        let written = serde_json::to_value(&end.end)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| {
                CoreError::invalid(
                    "invalid_request",
                    format!("the end of run {} is not one the protocol writes", end.run),
                )
            })?;
        let kept = EndFor {
            registration_id: end.registration,
            end: written,
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

    /// The ends named that read. Those that do not (as kept, the end is
    /// not a JSON object, or its result not JSON) are forgotten here
    /// ([`Store::forget_unreadable_ends`]): there is nothing to tell a
    /// host of them. An end this build does not know is one a later Clax
    /// wrote: it is sent as it is kept ([`End::Other`]).
    fn ends_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingEnd>, Self::Error> {
        let ends = self.store.ends_named(registration, ids)?;
        if ends.len() < ids.len() {
            let missing: Vec<String> = ids
                .iter()
                .filter(|id| !ends.iter().any(|e| &e.id == *id))
                .cloned()
                .collect();
            let forgotten = self.store.forget_unreadable_ends(registration, &missing)?;
            if forgotten > 0 {
                tracing::warn!(
                    "clax-bot: {forgotten} ended runs of registration {registration} do not read \
                     as the store keeps them: forgotten, and sent to no host"
                );
            }
        }
        ends.into_iter()
            .map(|end| {
                let undelivered = self
                    .store
                    .undelivered_messages(&end.run_id)?
                    .iter()
                    .map(|m| {
                        u64::try_from(m.seq).map_err(|_| {
                            CoreError::invalid(
                                "storage_unread",
                                format!("a message of run {} has seq {}", end.run_id, m.seq),
                            )
                        })
                    })
                    .collect::<Result<_, _>>()?;
                let Value::Object(members) = end.end else {
                    return Err(CoreError::invalid(
                        "storage_unread",
                        format!("the end of run {} is not a JSON object", end.run_id),
                    ));
                };
                let read = serde_json::from_value::<End>(Value::Object(members.clone()))
                    .unwrap_or(End::Other(members));
                Ok(PendingEnd {
                    id: end.id,
                    run: end.run_id,
                    end: read,
                    result: end.result,
                    undelivered,
                })
            })
            .collect()
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
        let rule = kept_rule(call.retry);
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
        match self.store.open_call_with(
            placed.run,
            placed.driver,
            new,
            Some(rule),
            Some(limit),
            now,
        ) {
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

    fn content_handle(&mut self, run: &str, id: &str) -> Result<Option<Value>, Self::Error> {
        Ok(self
            .store
            .bot_content_of(run, id)?
            .map(|row| ContentRow { meta: None, ..row }.handle()))
    }

    /// `None` too for a call, or a journal, whose row does not read: no
    /// answer stands for it, and the serve that asks goes on.
    fn settled(&mut self, id: &str) -> Result<Option<Result<Value, CallFailure>>, Self::Error> {
        let unreadable = |e: CoreError| {
            if clax_store::bot_runs::is_unreadable(&e) {
                tracing::warn!("clax-bot: call {id} as the store keeps it does not read: {e}");
                Ok(None)
            } else {
                Err(e)
            }
        };
        let call = match self.store.pending_call(id) {
            Ok(Some(call)) => call,
            Ok(None) => return Ok(None),
            Err(e) => return unreadable(e),
        };
        let journal = match self.store.bot_calls(&call.run_id, call.step_n) {
            Ok(journal) => journal,
            Err(e) => return unreadable(e),
        };
        let row = journal.into_iter().find(|row| row.seq() == call.seq);
        Ok(row.map(|row| {
            crate::surface::recorded(&row).map_err(|e| CallFailure {
                code: e.code,
                message: e.message,
                retryable: e.retryable,
            })
        }))
    }
}
