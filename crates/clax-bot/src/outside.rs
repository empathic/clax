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
//! | `standing` | For the surface that declared the run's kind: the operations a run of that kind is given (`clax_surface::Definition::run_operations`: the kind's `@allow`, the registration's own `allow`, less those the broker withholds while it keeps no content). For any other binding: every operation the registration's `allow` names, less those withheld |
//! | `limits` | The run's settings ("The run's settings", below): `max_effects_per_run` bounds the run's calls of the surface's effects, which the core counts and refuses past it (`effect_limit`, [`crate::surface::perform`]); for the surface that declared the run's kind alone, `runs.max_steps` and `runs.max_run_minutes` cap the run within its policy's caps ([`crate::policy::RunLimits::within`]), and a run that reaches one stops `run_cap` as at the instance's. A surface bound into a run of another's kind (one a join bound into a run of Clax's own) caps no run it did not start |
//! | `unavailable` | `unavailable`, for a registration that is gone: never issued, or not held with no definition kept for the binding. A registration whose definition is known takes its calls as far as the broker, so their arguments are checked first |
//! | `prompt` | The prompt file the run was started with: the text the broker keeps under the hash the binding names ([`CallBroker::prompt`]), read when the first turn asks and not before. So a run is sent the prompt file its registration had when the run was started, whatever its host has registered since and whether or not the registration is held (after a restart, or once its lease ended). The definition and the prompt file of a binding were one registration's at one moment, and the prompt file was checked against that definition when it was registered, so the two always fit. No prompt file (the standard prompt) for a binding that names none. A hash whose text is no longer kept is `prompt_gone` ([`CODE_PROMPT_GONE`]), which ends the run `failed`: no other prompt is sent in its place |
//! | `request` | The request as stored. No host is asked, so a run's first turn does not wait for one that is away |
//! | `working` | Nothing |
//! | `progress` | For the binding of the surface that declared the run's kind: the event, to the registration that started the run, through the broker ([`CallBroker::progress`]). Nothing for any other binding |
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
//!    ([`RunPolicy::delivery`]), no later than the run's settings allow
//!    ([`narrowed`]): `wait_limit` from now for an operation tagged
//!    `@waits`, `call_timeout` for any other. With neither, the broker's
//!    own limit.
//!
//! # The run's settings
//!
//! A run keeps the settings it was started with: the start route writes
//! its registration's settings then into the run's binding
//! ([`kept_with_run`], `StoredBinding::settings`), and every use of a
//! setting for the run reads that copy, never the registration as it is
//! now. A host that registers again with other settings changes them
//! for the runs started afterwards alone, and a daemon that restarted
//! (its hub holds no registration until its host registers again) gives
//! a run in flight the deadlines it was started with. A binding written
//! before settings were kept with the run has none of its own: it is
//! given its registration's as the broker has them
//! ([`crate::broker::Served::settings`]: the hub's while it holds the
//! registration, the row's while it does not, the defaults for a row
//! that keeps none). Settings that this build does not read (a setting a
//! later build keeps) are never the defaults, which would shed what they
//! limit: a run whose binding keeps them ends `failed`,
//! `bindings_invalid`, and a registration whose row keeps them is not
//! served from it.
//! 4. The call is sent ([`CallBroker::send`]) under the step's claim,
//!    for the definition the binding was made with, with the key of
//!    its place and the policy's rule for sending it again
//!    ([`Delivery::retry`]), which the hub follows
//!    ([`crate::bot_surfaces`], "Sending a call again"). A call that
//!    cannot be sent now because the broker does not hold its
//!    registration and did not drop it ([`CallBroker::dropped`]: the
//!    daemon started again, and its host has not registered again), but
//!    that an earlier attempt of the step opened and that still waits in
//!    the run's store, is waited for as one sent now when the rule sends
//!    calls again: its host is sent it when it comes back. One whose
//!    registration's lease ended is answered what is known of it.
//!    A step waits out a host that is away once: after one of its calls
//!    has waited for the registration and it is still not connected,
//!    the step's later calls are refused `unavailable` at once while it
//!    stays away. Likewise a host that is connected and does not answer:
//!    after one of the step's calls sent while it was connected has come
//!    to its deadline unanswered, the step's later calls of the
//!    registration are refused `unavailable` at once, and nothing is
//!    sent; the next step's calls are sent as before.
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

use crate::bot_surfaces::{CODE_INVALID_ARGS, CallFailure, EnqueueError, Millis};
use crate::broker::{CallBroker, Outcome, Sending, Served};
use crate::policy::{Delivery, RunPolicy};
use crate::registry::{CODE_PROMPT_GONE, CODE_UNAVAILABLE, NOTHING, Resolver};
use crate::surface::{
    Answer, BindingAsStored, Call, End, Fields, Progress, RunInfo, StepCx, StepEnd, StepParts,
    StepSession, SurfaceLimits, SurfaceProvider, no_such_operation, not_declared, recorded,
};
use clax_bot_sandbox::HostError;
use clax_store::bot_pending::EndFor;
use clax_store::bot_runs::Run;
use clax_surface::wire::{self, Bound as Limit, Settings};
use clax_surface::{Definition, Pattern, RunScope};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// What a run's binding keeps of its registration's `settings` when the
/// run is started ([`StoredBinding::with_settings`]): every setting but
/// `allow`, which the binding's operations already hold, as the host
/// protocol writes them. An object, `{}` for a registration that set
/// none, so that a run started with none is told from one whose
/// binding was written before settings were kept.
///
/// [`StoredBinding::with_settings`]: clax_store::bot_runs::StoredBinding::with_settings
#[must_use]
pub fn kept_with_run(settings: &Settings) -> Value {
    let kept = Settings {
        allow: None,
        ..settings.clone()
    };
    serde_json::to_value(kept).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

/// The settings `binding` keeps ([`kept_with_run`]), read; `None` for a
/// binding that keeps none.
///
/// # Errors
/// Kept settings this build does not read (a setting a later build
/// keeps, or what is no settings at all): never the defaults, which
/// would shed every limit the run was started with. A run whose binding
/// keeps such settings ends `failed`, `bindings_invalid`
/// ([`crate::registry::Providers::bind`]).
pub(crate) fn kept_settings(
    binding: &BindingAsStored,
) -> Result<Option<Settings>, serde_json::Error> {
    binding
        .settings
        .as_ref()
        .map(|kept| serde_json::from_value(kept.clone()))
        .transpose()
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
    /// The run's settings: those its binding keeps, else the
    /// registration's as the broker gave them (module documentation,
    /// "The run's settings").
    settings: Settings,
    /// The operations every run of this kind bound to the registration
    /// has.
    standing: Vec<String>,
    /// Whether the surface declared the run's kind: only then do its
    /// `runs.*` settings cap the run.
    declares_kind: bool,
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
    /// registration, and for one whose kept settings this build does not
    /// read ([`kept_settings`]): its calls are then answered as a binding
    /// no provider takes is, and none is sent.
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
        let settings = kept_settings(binding)
            .ok()?
            .or_else(|| served.as_ref().map(|served| served.settings.clone()))
            .unwrap_or_default();
        let declares_kind = kind
            .split_once('.')
            .is_some_and(|(surface, _)| surface == binding.surface);
        Some(OutsideProvider {
            broker,
            policy,
            declares_kind,
            surface: binding.surface.clone(),
            registration,
            served,
            settings,
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

    /// The run's settings (module documentation, "The run's settings").
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
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

/// `end` as a host is told it: whole for a run of its surface's own
/// kind (`declares_kind`), and for a run it was joined into with no
/// text a person or the model wrote: a give-up's `reason` is empty
/// ([`wire::Ended`]). A stop's reason and a failure's code are Clax's
/// own codes.
fn as_told(end: &End, declares_kind: bool) -> wire::End {
    match end.clone() {
        End::Finished => wire::End::Finished {},
        End::GaveUp { code, reason } => wire::End::GaveUp {
            code,
            reason: if declares_kind { reason } else { String::new() },
        },
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
                    content: true,
                },
            )
        });
    // What the broker withholds for content ([`Served::withheld`]) is
    // left out of both.
    let held_back = |name: &str| served.withheld.iter().any(|w| w == name);
    match own_kind {
        Some(run) => run
            .granted
            .into_iter()
            .filter(|name| !held_back(name))
            .map(str::to_string)
            .collect(),
        None => definition
            .operations()
            .map(|o| o.name)
            .filter(|name| {
                operator
                    .as_ref()
                    .is_none_or(|allow| allow.iter().any(|p| p.matches(name)))
                    && !held_back(name)
            })
            .map(str::to_string)
            .collect(),
    }
}

/// Refuses a call whose arguments hold a handle that is not one of the
/// run's own as the run's store holds it: each `Content` and `Image` the
/// script passes names an item of `run` and agrees with its row in
/// every field but `meta` (the provider's words of one answer). So a
/// host is sent no other run's content, and none a script made up.
/// Refused `invalid_args`, which the core journals: nothing is sent.
fn held_content(
    broker: &dyn CallBroker,
    run: &str,
    operation: &clax_surface::Operation<'_>,
    args: &[Value],
) -> Result<(), HostError> {
    let op = operation.name;
    let bad = |m: String| HostError::new(CODE_INVALID_ARGS, format!("clax.{op}: {m}"));
    let places = operation
        .content_in_args(args)
        .map_err(|e| bad(e.message))?;
    let args = Value::Array(args.to_vec());
    for place in places {
        let at = format!("the arguments at {}", place.at);
        let passed = args.pointer(&place.at).unwrap_or(&Value::Null);
        let id = passed["id"].as_str().unwrap_or_default();
        let held = broker
            .content(run, id)
            .map_err(|why| {
                HostError::new(
                    CODE_UNAVAILABLE,
                    format!("clax.{op}: the run's content could not be read: {why}"),
                )
                .retryable()
            })?
            .ok_or_else(|| bad(format!("{at} names no content of this run ({id:?})")))?;
        let agrees = held.as_object().is_some_and(|fields| {
            fields.iter().all(|(k, v)| passed.get(k) == Some(v))
                && passed
                    .as_object()
                    .is_some_and(|p| p.keys().all(|k| k == "meta" || fields.contains_key(k)))
        });
        if !agrees {
            return Err(bad(format!(
                "{at} is not the handle of the content {id:?} it names: pass the handle as it was given"
            )));
        }
    }
    Ok(())
}

/// An [`OutsideProvider`]'s session: the run and the step's claim, and
/// whether a call of the step has waited out its host while it was away
/// or while it was connected and did not answer.
struct OutsideSession<'s> {
    provider: &'s OutsideProvider,
    cx: StepCx<'s>,
    /// Set when a call of the step was made while its registration was
    /// not connected and its wait ended with the registration still not
    /// connected; cleared by a call that finds it connected. While it is
    /// set and the registration is not connected, a call is refused at
    /// once rather than held for the host again, so that a step whose
    /// script catches `unavailable` and calls again waits out an away
    /// host once, not once for each call.
    waited_out: AtomicBool,
    /// Set when a call of the step was sent while its registration was
    /// connected and came to its deadline with no answer from its host
    /// ([`unanswered_by_deadline`]). While it is set every later call of
    /// the step is refused at once, nothing sent, so that a host that
    /// takes calls and never answers them holds a step for one call's
    /// deadline, not one for each call. Calls of a step never overlap,
    /// so nothing is answered in the step after it is set: it holds for
    /// the rest of the step, and the next step's session starts without
    /// it.
    unanswered: AtomicBool,
}

/// What a call of `op` is answered when what the run's store keeps of it
/// cannot be read (`e`): `unavailable`, journaled by no one, so the step
/// run again reads it again; never "nothing happened", which an effect
/// sent by an earlier attempt may not be.
fn unread(op: &str, e: &clax_core::CoreError) -> Answer {
    Answer::unwritten(
        HostError::new(
            CODE_UNAVAILABLE,
            format!("clax.{op}: what the run's store keeps of the call could not be read: {e}"),
        )
        .retryable(),
    )
}

/// Whether `outcome`, of a call whose deadline was `deadline`, read at
/// `now`, is its deadline's and not its host's answer: the deadline has
/// come, and the call is settled with one of the failures a call left
/// unanswered is settled with ([`crate::bot_surfaces::unanswered`]).
fn unanswered_by_deadline(outcome: &Outcome, deadline: Option<Millis>, now: Millis) -> bool {
    let Outcome::Settled(Err(failure)) = outcome else {
        return false;
    };
    deadline.is_some_and(|d| now >= d)
        && [
            wire::CODE_UNAVAILABLE,
            wire::CODE_EXPIRED,
            wire::CODE_UNKNOWN_OUTCOME,
        ]
        .contains(&failure.code.as_str())
}

impl OutsideSession<'_> {
    /// The call at `call`'s place that an earlier attempt opened and
    /// that still waits in the run's store, when there is one.
    ///
    /// # Errors
    /// The store's, when it cannot be read: such a call may have been
    /// sent, so the caller answers it [`unread`], never as one never
    /// opened, which would leave an effect its host performed out of the
    /// run's count.
    fn kept(
        &self,
        call: &Call<'_>,
    ) -> clax_core::Result<Option<clax_store::bot_pending::PendingCall>> {
        Ok(self
            .cx
            .claim
            .pending_call(call.place)?
            .filter(|kept| kept.answered_at.is_none()))
    }

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
        let kept = match self.kept(call) {
            Ok(kept) => kept,
            Err(e) => return unread(call.op, &e),
        };
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
        if let Err(refused) =
            held_content(&*provider.broker, &self.cx.run.id, &operation, call.args)
        {
            return Answer::refused(refused);
        }
        let waits = operation.op.waits;
        let delivery = narrowed(
            provider.policy.delivery(self.cx.run, call.op),
            &provider.settings,
        );
        let broker = &*provider.broker;
        // A host this step has waited out already, and that is still
        // away, is not waited for again.
        let away = !broker.connected(&provider.registration);
        if !away {
            self.waited_out.store(false, Ordering::SeqCst);
        } else if self.waited_out.load(Ordering::SeqCst) {
            return self.unsent(&call, host_error(EnqueueError::NotConnected.failure()));
        }
        // A host that left a call of this step unanswered to its
        // deadline is sent no more of the step's calls.
        if self.unanswered.load(Ordering::SeqCst) {
            return self.unsent(
                &call,
                HostError::new(
                    CODE_UNAVAILABLE,
                    "the surface's host left a call of this step unanswered to its deadline. \
                     Nothing happened",
                )
                .retryable(),
            );
        }
        let now = broker.now();
        let deadline = delivery.limit(waits).map(|d| now.after(millis(d)));
        // A run the surface was joined into: its host is told what the
        // run is on.
        let scope = &self.cx.binding.scope;
        let joined = (!provider.declares_kind && !scope.is_null()).then_some(scope);
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
                retry: delivery.retry,
                joined,
            },
        );
        let id = match sent {
            Ok(id) => id,
            // A call an earlier attempt of the step opened, which still
            // waits in the run's store, and could not be sent now: under
            // a rule that sends calls again, and when the broker did not
            // drop its registration for good (the daemon started again
            // since, and its host has not registered again yet), it is
            // sent when its host comes back, and is waited for as if it
            // was sent now. A registration whose lease ended is not
            // waited for: the call is answered what is known of it.
            Err(failure) => match self.kept(&call) {
                Ok(Some(kept))
                    if delivery.retry.waits_for_host()
                        && !broker.dropped(&provider.registration) =>
                {
                    kept.id
                }
                Err(e) => return unread(call.op, &e),
                Ok(_) => return self.unsent(&call, host_error(failure)),
            },
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
        let outcome = broker.outcome(&id, until).await;
        if away && !broker.connected(&provider.registration) {
            self.waited_out.store(true, Ordering::SeqCst);
        }
        if !away && unanswered_by_deadline(&outcome, deadline, broker.now()) {
            self.unanswered.store(true, Ordering::SeqCst);
        }
        match outcome {
            Outcome::Settled(outcome) => Answer::of(outcome.map_err(host_error)),
            // The call's deadline is the one it was first opened with,
            // which the store's row of it keeps: an attempt that finds
            // the call still waiting does not move it.
            Outcome::NotYet if waits => Answer::waiting_until(
                self.cx
                    .claim
                    .pending_call(call.place)
                    .ok()
                    .flatten()
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

    /// The run's settings' limits: `max_effects_per_run` as the
    /// surface's bound on effects, and, when the surface declared the
    /// run's kind, `runs.max_steps` and `runs.max_run_minutes` as caps
    /// the run loop holds the run to within its policy's; `"none"` or no
    /// setting sets none.
    fn limits(&self) -> SurfaceLimits {
        let at = |limit: Option<Limit>| match limit {
            Some(Limit::At(n)) => Some(n),
            None | Some(Limit::None) => None,
        };
        let runs = if self.declares_kind {
            self.settings.runs.clone().unwrap_or_default()
        } else {
            wire::RunSettings::default()
        };
        let cap = |limit: Option<Limit>| at(limit).map(|n| u32::try_from(n).unwrap_or(u32::MAX));
        SurfaceLimits {
            max_effects: at(self.settings.max_effects_per_run),
            max_steps: cap(runs.max_steps),
            max_run_minutes: cap(runs.max_run_minutes),
        }
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
        Ok(Box::new(OutsideSession {
            provider: self,
            cx,
            waited_out: AtomicBool::new(false),
            unanswered: AtomicBool::new(false),
        }))
    }

    /// Passes `progress` on to the registration, as an event of the
    /// run ([`clax_surface::wire::Event`]), when this binding is the
    /// kind's: the registration that started the run. What the run
    /// waits on names its surface, and its operation and deadline only
    /// when the surface is this one: another surface's calls are not
    /// this registration's to see.
    fn progress(&self, run: &RunInfo, progress: &Progress<'_>) {
        if self.served.is_none() || run.kind_surface() != self.surface {
            return;
        }
        let event = match *progress {
            Progress::Step {
                step,
                kind,
                delivered,
            } => {
                let mut v = json!({"type": "step", "step": step, "kind": kind.as_str()});
                if !delivered.is_empty() {
                    v["messages"] = json!(delivered);
                }
                v
            }
            Progress::Parked { step, waiting_on } => {
                let surface = waiting_on["surface"].as_str().unwrap_or_default();
                let mut on = json!({"surface": surface});
                if surface == self.surface {
                    let op = waiting_on["op"].as_str().unwrap_or_default();
                    let prefix = format!("{}.", self.surface);
                    on["op"] = json!(op.strip_prefix(&prefix).unwrap_or(op));
                    on["deadline"] = waiting_on["deadline"].clone();
                }
                json!({"type": "parked", "step": step, "waiting_on": on})
            }
            Progress::Resumed { step } => json!({"type": "resumed", "step": step}),
        };
        self.broker.progress(&self.registration, &run.id, event);
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
            end: serde_json::to_value(as_told(end, self.declares_kind)).unwrap_or(Value::Null),
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
            .ended(
                &self.registration,
                &run.id,
                as_told(end, self.declares_kind),
                result.cloned(),
            )
            .map(|_| ())
            .map_err(|why| HostError::new(CODE_UNAVAILABLE, why))
    }
}
