//! The provider of a surface a host registered
//! (`docs/superpowers/specs/2026-11-10-claxbot-surfaces-design.md`,
//! "Providers" and "A host's surface on a daemon"): each call of a
//! script goes out to the registration's host through a
//! [`CallBroker`] with the key of its place, and the answer that comes
//! back is the script's. [`OutsideProvider`] serves one binding of one
//! run for one tick; [`Registrations`] is the registry's resolver that
//! makes one for each stored binding that names a registration
//! ([`crate::registry::Resolver`]).
//!
//! # What the provider is, hook by hook
//!
//! | Hook | Does |
//! |---|---|
//! | `definition` | The registration's definition kept under the binding's hash, so a run in flight keeps the definition it was bound with whatever its host has registered since; the registration's own for a binding that keeps no hash. The empty definition for a registration that is gone |
//! | `standing` | For the surface that declared the run's kind: the operations a run of that kind is given (`clax_surface::Definition::run_operations`: the kind's `@allow`, the registration's own `allow`, less those withheld while no content is stored). For any other binding: every operation the registration's `allow` names, less those withheld |
//! | `unavailable` | `unavailable`, for a registration that is gone: never issued, or not held with no definition kept for the binding. A registration whose definition is known takes its calls as far as the broker, so their arguments are checked first |
//! | `prompt` | The prompt file the run was started with: the text the broker keeps under the hash the binding names ([`CallBroker::prompt`]), read when the first turn asks and not before. So a run is sent the prompt file its registration had when the run was started, whatever its host has registered since and whether or not the registration is held (after a restart, or once its lease ended). The definition and the prompt file of a binding were one registration's at one moment, and the prompt file was checked against that definition when it was registered, so the two always fit. No prompt file (the standard prompt) for a binding that names none. A hash whose text is no longer kept is `prompt_gone` ([`CODE_PROMPT_GONE`]), which ends the run `failed`: no other prompt is sent in its place |
//! | `request` | The request as stored. No host is asked, so a run's first turn does not wait for one that is away |
//! | `working` | Nothing |
//! | `begin` | A session with no state: it cannot fail and reads nothing |
//! | `call` | Below |
//! | `finish` | No field and no row |
//! | `ending` | No field: an outside provider has no part in the terminal step |
//! | `end_for` | With a broker whose store is the run's ([`CallBroker::commits_ends`]) and a registration it knows: the end as the host is told it, which the terminal step's commit keeps in its transaction, so a run never ends without its end kept for its host. `None` otherwise |
//! | `ended` | When the end was kept with the commit: wakes the registration, and nothing more, however often it is told. Otherwise (a broker that keeps its calls elsewhere, as the one in memory does) records the run's end with the broker, which changes nothing while the same end is unacknowledged. Either way the hub delivers the end with each `serve` until the host acknowledges it |
//! | `awaiting_ended`, `driver_started` | Not asked: the provider lives for one tick. Nothing is owed: an end that was recorded is delivered by the hub, whoever is running |
//!
//! # A call
//!
//! 1. A call that comes with its entry is answered from the entry
//!    ([`recorded`]); the broker is not touched.
//! 2. An operation the binding does not have is refused, whoever hands
//!    it over.
//! 3. The call's deadline is the policy's for the run and the operation
//!    ([`RunPolicy::delivery`]), no later than the registration's own
//!    settings allow ([`narrowed`]): `wait_limit` from now for an
//!    operation tagged `@waits`, `call_timeout` for any other. With
//!    neither, the broker's own limit.
//! 4. The call is sent ([`CallBroker::send`]) under the step's claim,
//!    for the definition the binding was made with, with the key of
//!    its place.
//! 5. Its outcome is awaited by the broker's clock: until the deadline,
//!    and, for an operation tagged `@waits`, no longer than
//!    `park_after`.
//!
//! | What happens | The answer | Journaled |
//! |---|---|---|
//! | The host answers a result that is the operation's declared result | The value | By the core, with the value ([`Answer::of`]) |
//! | The host answers anything else, or an error | The failure: the host's own code when the operation declares it, `invalid_result` otherwise | By the core, the failure ([`Answer::of`]) |
//! | The deadline comes first | `unavailable` (retryable) when no `serve` was answered with the call, else `expired` for a read and `unknown_outcome` for a write or an effect | By the core, the failure ([`Answer::of`]) |
//! | The call could not be sent: the registration is gone or not connected, its queue is full, the key is another call's | The broker's failure: `unavailable` for the first three | By the core, the failure ([`Answer::refused`]): the step run again is answered it again, and nothing is sent |
//! | An operation tagged `@waits` has no answer after `park_after` | [`Answer::waiting_until`] the call's deadline | By no one. The broker holds the call under its key until its deadline, and the run parks until then |
//!
//! **Who journals an answer that arrives later.** Not this provider:
//! it is gone with its tick. A broker whose store journals answers
//! (the daemon's) writes the row at the call's place when the answer
//! or the deadline's failure arrives, in the form the core writes for
//! [`Answer::of`]; the step run again then makes the call with that
//! entry (1). A broker whose store journals nothing (in memory) keeps
//! the settled call: the step run again sends the call again with its
//! key, which is the call the broker holds, and is answered its
//! outcome, which the core then journals. When both the store and the
//! core write a row for one place (an answer that arrived while the
//! provider waited), the first stands and the two are the same
//! (`Store::record_bot_call` keeps the entry that is there).

use crate::bot_surfaces::{CallFailure, Millis};
use crate::broker::{CallBroker, Outcome, Sending, Served};
use crate::policy::{Delivery, RunPolicy};
use crate::registry::{CODE_PROMPT_GONE, CODE_UNAVAILABLE, NOTHING, Resolver};
use crate::surface::{
    Answer, BindingAsStored, Call, End, Fields, RunInfo, StepCx, StepEnd, StepParts, StepSession,
    SurfaceProvider, no_such_operation, not_declared, recorded,
};
use clax_bot_sandbox::HostError;
use clax_store::bot_pending::EndFor;
use clax_store::bot_runs::Run;
use clax_surface::wire::{self, Bound as Limit, Settings};
use clax_surface::{Definition, Pattern, RunScope};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// `delivery`, with each time no longer than `settings`, a
/// registration's own, allows: a registration narrows what the policy
/// gives and never widens it, so a setting of `"none"` leaves the
/// policy's.
#[must_use]
pub fn narrowed(delivery: Delivery, settings: &Settings) -> Delivery {
    let least = |policy: Option<Duration>, own: Option<Limit>| match (policy, own) {
        (policy, None | Some(Limit::None)) => policy,
        (None, Some(Limit::At(s))) => Some(Duration::from_secs(s)),
        (Some(policy), Some(Limit::At(s))) => Some(policy.min(Duration::from_secs(s))),
    };
    Delivery {
        call_timeout: least(delivery.call_timeout, settings.call_timeout_s),
        wait_limit: least(delivery.wait_limit, settings.wait_limit_s),
        ..delivery
    }
}

/// A failure as a script is thrown it.
fn host_error(failure: CallFailure) -> HostError {
    let e = HostError::new(failure.code, failure.message);
    if failure.retryable { e.retryable() } else { e }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The registry's resolver for the surfaces hosts registered: the
/// provider of each stored binding that names a registration
/// ([`BindingAsStored::registration`]) is an [`OutsideProvider`] over
/// one broker. A binding that names none is not its to serve.
#[derive(Clone)]
pub struct Registrations {
    broker: Arc<dyn CallBroker>,
    policy: Arc<dyn RunPolicy>,
}

impl Registrations {
    /// Over `broker`, with `policy` saying how long each call may take
    /// ([`RunPolicy::delivery`]): the policy the run loop's deps have.
    #[must_use]
    pub fn new(broker: Arc<dyn CallBroker>, policy: Arc<dyn RunPolicy>) -> Registrations {
        Registrations { broker, policy }
    }
}

impl Resolver for Registrations {
    fn resolve(
        &self,
        _row: &Run,
        kind: &str,
        binding: &BindingAsStored,
    ) -> Option<Arc<dyn SurfaceProvider>> {
        binding.registration.as_ref()?;
        Some(Arc::new(OutsideProvider::new(
            self.broker.clone(),
            self.policy.clone(),
            binding,
            kind,
        )?))
    }
}

/// The provider of one binding of a surface a host registered, for one
/// tick of one run. See the module documentation.
pub struct OutsideProvider {
    broker: Arc<dyn CallBroker>,
    policy: Arc<dyn RunPolicy>,
    surface: String,
    registration: String,
    /// The registration as the broker gave it when the provider was
    /// made; `None` for one that is gone.
    served: Option<Served>,
    /// The operations every run of this kind bound to the registration
    /// has.
    standing: Vec<String>,
    /// The hash of the prompt file the run was started with, as the
    /// binding names it; `None` for a run started with none.
    prompt_hash: Option<String>,
    /// That prompt file's text, or why it cannot be given, once it has
    /// been asked for.
    prompt: OnceLock<Result<String, HostError>>,
}

impl std::fmt::Debug for OutsideProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutsideProvider")
            .field("surface", &self.surface)
            .field("registration", &self.registration)
            .field("gone", &self.served.is_none())
            .finish_non_exhaustive()
    }
}

impl OutsideProvider {
    /// The provider of `binding`, a stored binding that names a
    /// registration, for a run of kind `kind` (`<surface>.<kind>`):
    /// over `broker`, which is asked now for the registration and the
    /// definition kept under the binding's hash, with `policy` saying
    /// how long each call may take. `None` for a binding that names no
    /// registration.
    #[must_use]
    pub fn new(
        broker: Arc<dyn CallBroker>,
        policy: Arc<dyn RunPolicy>,
        binding: &BindingAsStored,
        kind: &str,
    ) -> Option<OutsideProvider> {
        let registration = binding.registration.clone()?;
        let served = broker
            .registration(&registration, binding.hash.as_deref())
            .filter(|served| served.name == binding.surface);
        let standing = served
            .as_ref()
            .map(|served| standing(served, kind))
            .unwrap_or_default();
        Some(OutsideProvider {
            broker,
            policy,
            surface: binding.surface.clone(),
            registration,
            served,
            standing,
            prompt_hash: binding.prompt.clone(),
            prompt: OnceLock::new(),
        })
    }

    /// The text kept under `hash`, the prompt file the run was started
    /// with.
    fn kept_prompt(&self, hash: &str) -> Result<String, HostError> {
        match self.broker.prompt(hash) {
            Ok(Some(text)) => Ok(text),
            Ok(None) => Err(HostError::new(
                CODE_PROMPT_GONE,
                format!(
                    "the prompt file the run was started with (SHA-256 {hash}) of the surface \
                     {} is no longer kept, and the run is sent no other in its place",
                    self.surface
                ),
            )),
            Err(why) => Err(HostError::new(
                CODE_UNAVAILABLE,
                format!(
                    "the prompt file the run was started with (SHA-256 {hash}) could not be \
                     read: {why}"
                ),
            )
            .retryable()),
        }
    }

    /// The ID of the registration that serves the binding.
    #[must_use]
    pub fn registration(&self) -> &str {
        &self.registration
    }

    /// Whether the run's end is kept by its terminal commit: the
    /// broker's store is the run's, and the registration is one it
    /// knows.
    fn commits_end(&self) -> bool {
        self.served.is_some() && self.broker.commits_ends()
    }

    fn gone(&self) -> HostError {
        HostError::new(
            CODE_UNAVAILABLE,
            format!(
                "the surface {}'s host is gone: its registration has ended. Nothing happened",
                self.surface
            ),
        )
    }
}

/// `end` as a host is told it.
fn as_told(end: &End) -> wire::End {
    match end.clone() {
        End::Finished => wire::End::Finished {},
        End::GaveUp { code, reason } => wire::End::GaveUp { code, reason },
        End::Stopped { reason, .. } => wire::End::Stopped { reason },
        End::Failed { code } => wire::End::Failed { code },
    }
}

/// The operations a run of kind `kind` bound to `served` has of it.
fn standing(served: &Served, kind: &str) -> Vec<String> {
    let definition = &served.definition;
    let operator: Option<Vec<Pattern>> = served.settings.allow.as_ref().map(|allow| {
        allow
            .iter()
            .filter_map(|p| Pattern::parse(p).ok())
            .collect()
    });
    let own_kind = kind
        .split_once('.')
        .filter(|(surface, _)| *surface == served.name)
        .and_then(|(_, kind)| {
            definition.run_operations(
                kind,
                &RunScope {
                    operator: operator.as_deref(),
                    starter: None,
                    content: false,
                },
            )
        });
    match own_kind {
        Some(run) => run.granted.into_iter().map(str::to_string).collect(),
        None => definition
            .operations()
            .map(|o| o.name)
            .filter(|name| {
                operator
                    .as_ref()
                    .is_none_or(|allow| allow.iter().any(|p| p.matches(name)))
                    && !served.withheld.iter().any(|w| w == name)
            })
            .map(str::to_string)
            .collect(),
    }
}

/// An [`OutsideProvider`]'s session: the run and the step's claim, and
/// no state.
struct OutsideSession<'s> {
    provider: &'s OutsideProvider,
    cx: StepCx<'s>,
}

impl OutsideSession<'_> {
    /// What a call that could not be sent now is answered: `failure`,
    /// unless an earlier attempt opened the call and it still waits in
    /// the run's store. Its host may have been sent it then, so
    /// "nothing happened" would be a claim no one can make: it is
    /// answered what a call left unanswered is by what is known of it
    /// ([`crate::broker::stored::deadline_failure`]: `unavailable` when
    /// it was never sent, `expired` for a read that was, `unknown_outcome`
    /// for a write or an effect that was). The core journals the answer
    /// at the call's place, which ends the call's wait in the store.
    fn unsent(&self, call: &Call<'_>, failure: HostError) -> Answer {
        let kept = self
            .cx
            .claim
            .pending_call(call.place)
            .filter(|kept| kept.answered_at.is_none());
        Answer::refused(match kept {
            Some(kept) => {
                let known = crate::broker::stored::deadline_failure(&kept);
                host_error(CallFailure {
                    code: known.code,
                    message: known.message,
                    retryable: known.retryable,
                })
            }
            None => failure,
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl StepSession for OutsideSession<'_> {
    async fn call(&self, call: Call<'_>) -> Answer {
        // A replayed call: the entry is the answer, and nothing is
        // sent.
        if let Some(entry) = call.replay {
            return Answer::of(recorded(entry));
        }
        if !self.cx.binding.has(call.op) {
            return Answer::refused(not_declared(call.op));
        }
        let provider = self.provider;
        let Some(served) = &provider.served else {
            return self.unsent(&call, provider.gone());
        };
        let Some(operation) = served.definition.operation(call.op) else {
            return Answer::refused(no_such_operation(call.op));
        };
        let waits = operation.op.waits;
        let delivery = narrowed(
            provider.policy.delivery(self.cx.run, call.op),
            &served.settings,
        );
        let broker = &*provider.broker;
        let now = broker.now();
        let deadline = delivery.limit(waits).map(|d| now.after(millis(d)));
        let sent = broker.send(
            &self.cx.claim,
            &Sending {
                registration: &provider.registration,
                definition: &served.hash,
                run: &self.cx.run.id,
                op: call.op,
                args: call.args,
                raw: call.raw,
                place: call.place,
                waits,
                deadline,
            },
        );
        let id = match sent {
            Ok(id) => id,
            Err(failure) => return self.unsent(&call, host_error(failure)),
        };
        // How long this attempt waits: to the deadline, and for a call
        // that may wait, no longer than the policy lets a script sit.
        let park = delivery
            .park_after
            .filter(|_| waits)
            .map(|d| now.after(millis(d)));
        let until = match (park, deadline) {
            (Some(park), Some(deadline)) => park.min(deadline),
            (Some(at), None) | (None, Some(at)) => at,
            (None, None) => Millis(u64::MAX),
        };
        match broker.outcome(&id, until).await {
            Outcome::Settled(outcome) => Answer::of(outcome.map_err(host_error)),
            // The call's deadline is the one it was first opened with,
            // which the store's row of it keeps: an attempt that finds
            // the call still waiting does not move it.
            Outcome::NotYet if waits => Answer::waiting_until(
                self.cx
                    .claim
                    .pending_call(call.place)
                    .and_then(|kept| kept.expires_at)
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
                    .map(|at| at.to_utc())
                    .or(deadline.map(crate::broker::stored::at)),
            ),
            // The broker settles a call at its deadline, so this is a
            // broker that could not read the outcome: nothing is known
            // of the call, and nothing is journaled.
            Outcome::NotYet => Answer::unwritten(
                HostError::new(
                    CODE_UNAVAILABLE,
                    format!("clax.{}: its answer could not be read", call.op),
                )
                .retryable(),
            ),
        }
    }

    fn finish(self: Box<Self>, _end: StepEnd) -> StepParts {
        StepParts::default()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SurfaceProvider for OutsideProvider {
    fn definition(&self) -> &Definition {
        match &self.served {
            Some(served) => &served.definition,
            None => &NOTHING,
        }
    }

    fn standing(&self, op: &str) -> bool {
        self.standing.iter().any(|s| s == op)
    }

    fn unavailable(&self) -> Option<HostError> {
        self.served.is_none().then(|| self.gone())
    }

    /// The prompt file the run was started with: the text kept under
    /// the hash its binding names, and none for a binding that names
    /// none. Never the registration's current one.
    fn prompt(&self) -> Result<Option<&str>, HostError> {
        let Some(hash) = &self.prompt_hash else {
            return Ok(None);
        };
        match self.prompt.get_or_init(|| self.kept_prompt(hash)) {
            Ok(text) => Ok(Some(text)),
            Err(e) => Err(e.clone()),
        }
    }

    /// The request as stored: no host is asked, so a host that is away
    /// does not keep a run from its first turn.
    async fn request(&self, run: &RunInfo) -> Result<Value, HostError> {
        Ok(run.request.clone())
    }

    async fn begin<'s>(&'s self, cx: StepCx<'s>) -> clax_core::Result<Box<dyn StepSession + 's>> {
        Ok(Box::new(OutsideSession { provider: self, cx }))
    }

    async fn ending(
        &self,
        _cx: StepCx<'_>,
        _end: &End,
        _result: Option<&Value>,
    ) -> clax_core::Result<Fields> {
        Ok(Fields::new())
    }

    /// The end, for the terminal commit to keep, when the broker's
    /// store is the run's and knows the registration.
    fn end_for(&self, _run: &RunInfo, end: &End, result: Option<&Value>) -> Option<EndFor> {
        self.commits_end().then(|| EndFor {
            registration_id: self.registration.clone(),
            end: serde_json::to_value(as_told(end)).unwrap_or(Value::Null),
            result: result.cloned(),
        })
    }

    async fn ended(
        &self,
        run: &RunInfo,
        end: &End,
        result: Option<&Value>,
    ) -> Result<(), HostError> {
        // Kept with the run's terminal commit: its host is woken for
        // it. Told again, that is all that happens again.
        if self.commits_end() {
            self.broker.wake(&self.registration);
            return Ok(());
        }
        self.broker
            .ended(&self.registration, &run.id, as_told(end), result.cloned())
            .map(|_| ())
            .map_err(|why| HostError::new(CODE_UNAVAILABLE, why))
    }
}
