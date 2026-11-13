//! What a provider of a host's surface sends its calls through
//! ([`CallBroker`]), so that the run loop names no transport: a call
//! goes out for a registration under the run's claim, its outcome is
//! awaited until an instant of an injected clock, a registration's
//! definition is looked up, and a run's end is recorded for delivery.
//! [`crate::outside::OutsideProvider`] is its one caller.
//!
//! [`HubBroker`] is the implementation: the hub of the host protocol
//! ([`crate::bot_surfaces::Hub`]) and what keeps its calls
//! ([`BrokerStore`], a [`Pending`]) behind one lock, with a clock
//! ([`BrokerClock`]). The daemon's routes reach the same hub through
//! it ([`HubBroker::with`]), so a host's `serve` and a run's call meet
//! in one place. Over [`MemoryPending`] it keeps everything in memory,
//! for tests; a daemon gives it [`stored::StoredPending`], which keeps
//! it in the store its runs are in.
//!
//! # Who writes what, and when
//!
//! | # | Who | Does |
//! |---|---|---|
//! | 1 | Provider | [`CallBroker::send`], inside [`crate::surface::StepSession::call`], with the driver's claim and the call's place |
//! | 2 | Broker | Opens the call in its store, **before the call is sent**: [`BrokerStore::open_placed`], given the claim's driver and the place ([`Placed`]: the run, the step, the attempt, the place among the step's calls, the hash of the arguments, whether the operation waits). A store that journals answers itself keeps them with the call, since they are what later lets an answer be journaled at the place with no claim held. An answer can therefore arrive at any time after this, also before the run parks |
//! | 3 | Broker | Wakes the registration: its host's next `serve` is answered with the call |
//! | 4 | Provider | [`CallBroker::outcome`]: waits, by the broker's clock, until the call is settled or an instant comes |
//! | 5 | Host | Answers through a `serve`; the hub checks a result against the operation's declared result in the definition the call was made under, and settles the call in the store |
//! | 6 | Broker | Answers the provider the settled outcome, or that there is none yet |
//!
//! Who journals the outcome at the call's place is said where the
//! provider answers ([`crate::outside`]).
//!
//! A call's key is its place's ([`CallPlace::key`]). A call sent again
//! with its key, for the same registration, run, operation and
//! arguments, is the call the store holds: the same ID is answered, the
//! host sees no second call, and [`CallBroker::outcome`] answers what
//! settled it, if anything has.

use crate::bot_surfaces::{
    Answered, CallFailure, CallSummary, Enqueue, EnqueueError, Hub, MemoryPending, Millis, NewCall,
    NewEnd, OpenedCall, Pending, PendingCall, PendingEnd, PendingSummary, RecordedEnd, Retry,
    Tries, withheld,
};
use crate::host::args_hash;
use crate::surface::{CallPlace, Claim};
use clax_bot_sandbox::MaybeSendSync;
use clax_store::bot_runs::DriverId;
use clax_surface::Definition;
use clax_surface::wire::{End, Settings};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A call for [`CallBroker::send`].
#[derive(Clone, Copy, Debug)]
pub struct Sending<'a> {
    /// The ID of the registration that serves the call.
    pub registration: &'a str,
    /// The hash of the definition the call is made under: the one the
    /// run was bound with, which may no longer be the registration's.
    pub definition: &'a str,
    /// The run that makes the call.
    pub run: &'a str,
    /// The operation, by the name its definition gives it and a script
    /// calls it by after `clax.`: `shell.settings.set`. Its host is
    /// sent the name below the surface's.
    pub op: &'a str,
    /// The arguments, checked against the definition: one value for
    /// each parameter.
    pub args: &'a [Value],
    /// The arguments as the script passed them, a JSON array: what the
    /// journal's hash is of.
    pub raw: &'a Value,
    /// The call's place in its step. Its key ([`CallPlace::key`]) is
    /// the call's key, passed to the host as it is.
    pub place: &'a CallPlace,
    /// Whether the operation is tagged `@waits`: its run may park on
    /// the call.
    pub waits: bool,
    /// When the call is settled if nothing has settled it. `None`: the
    /// broker's own limit from now
    /// ([`crate::bot_surfaces::HubLimits::call_ms`]).
    pub deadline: Option<Millis>,
    /// How the call is sent again when an attempt fails
    /// ([`crate::bot_surfaces::Enqueue::retry`]).
    pub retry: Retry,
    /// For a run of another surface's kind that a join bound the
    /// surface into, what the run is on (its binding's scope,
    /// [`crate::joins::joined_scope`]): its host is sent it with the
    /// call ([`clax_surface::wire::Call::joined`]). `None` for a run the
    /// registration started.
    pub joined: Option<&'a Value>,
}

/// Which call of which attempt is being opened, for a store that keeps
/// more of a call than the hub gives it ([`BrokerStore::open_placed`]).
#[derive(Clone, Debug)]
pub struct Placed<'a> {
    /// The driver that holds the run: the claim the call is opened
    /// under.
    pub driver: &'a DriverId,
    pub run: &'a str,
    /// The step, the attempt, the place among the step's calls, and
    /// the key.
    pub place: &'a CallPlace,
    /// The arguments as the script passed them.
    pub raw: &'a Value,
    /// The hash of `raw` as the journal keeps it
    /// ([`crate::host::args_hash`]).
    pub args_hash: String,
    /// Whether the run may park on the call.
    pub waits: bool,
}

/// What [`CallBroker::outcome`] answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The call is settled: its value, checked against the operation's
    /// declared result, or its failure (its host's, a result that
    /// failed the check, or its deadline's).
    Settled(Result<Value, CallFailure>),
    /// The instant came and nothing has settled the call.
    NotYet,
}

/// A registration as a provider is given it
/// ([`CallBroker::registration`]).
#[derive(Clone, Debug)]
pub struct Served {
    /// The surface's name.
    pub name: String,
    /// The definition asked for: the one kept under the hash given, or
    /// the registration's own.
    pub definition: Arc<Definition>,
    /// That definition's hash.
    pub hash: String,
    /// The operations of that definition no run is given, by the names
    /// a script calls them by after `clax.` (`shell.screenshot`).
    pub withheld: Vec<String>,
    /// The registration's settings, as registered: the hub's for a
    /// registration it holds, else those its store keeps of it
    /// ([`crate::bot_surfaces::Pending::registration_settings`]), else
    /// the defaults. A run reads its own, kept in its binding, and these
    /// only for a binding that keeps none
    /// ([`crate::outside::OutsideProvider`]).
    pub settings: Settings,
    /// Whether the broker holds the registration now. One that is not
    /// held (its lease ended, or the process started since) takes no
    /// call until its host registers as it again.
    pub held: bool,
}

/// What an outside provider sends its calls through. See the module
/// documentation.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CallBroker: MaybeSendSync {
    /// The instant it is, by the broker's clock: what deadlines are
    /// counted from.
    fn now(&self) -> Millis;

    /// The registration with this ID, with the definition kept under
    /// `hash`, or its own when `hash` is `None`. `None` when the ID was
    /// never issued, when no definition is kept under the hash, and,
    /// with no hash, when the registration is not held.
    fn registration(&self, id: &str, hash: Option<&str>) -> Option<Served>;

    /// The text of the prompt file kept under `hash`, the SHA-256 of
    /// its bytes in lowercase hex: the one a run's binding names
    /// ([`crate::surface::BindingAsStored::prompt`]), whatever its
    /// registration has now and whether or not the registration is
    /// held. `Ok(None)` when none is kept under the hash.
    ///
    /// # Errors
    /// Why the broker's store could not be read, in words.
    fn prompt(&self, hash: &str) -> Result<Option<String>, String>;

    /// Whether the registration is connected now: its host is serving
    /// it, or was a moment ago.
    fn connected(&self, id: &str) -> bool;

    /// Whether the broker dropped the registration for good (its lease
    /// ended, or it made room for another), and its host has not gone on
    /// as it since, as its store keeps it, so also before the broker last
    /// started: a call of it waits for a host nothing says will come back
    /// ([`crate::bot_surfaces::Hub::dropped`]). False for one it holds,
    /// and for one it does not hold and did not drop, which may resume
    /// (the broker started again while it was held).
    fn dropped(&self, id: &str) -> bool;

    /// Sends `call` for its registration, under `claim`, the claim of
    /// the driver that holds the call's run: opens the call in the
    /// broker's store (the row a store keeps of a call that is on its
    /// way is written here, with the place and under the claim, before
    /// the call is sent) and wakes the registration. Answers the
    /// call's ID. A call whose key the store holds, for the same
    /// registration, run, operation and arguments, is that call: its
    /// ID is answered and nothing is sent again.
    ///
    /// # Errors
    /// What the call's script is thrown: nothing of the call is kept,
    /// and no host was sent it. `unavailable` for a registration that
    /// is gone or not connected.
    fn send(&self, claim: &Claim<'_>, call: &Sending<'_>) -> Result<String, CallFailure>;

    /// The outcome of the call `call` ([`CallBroker::send`]'s ID):
    /// waits until the call is settled or the clock reads `until`,
    /// whichever is first. A call whose deadline has come by then is
    /// settled with its deadline's failure.
    async fn outcome(&self, call: &str, until: Millis) -> Outcome;

    /// Whether the broker's store is the store its runs are in, so
    /// that a run's end kept by the run's terminal commit
    /// (`Store::commit_bot_step_on`, `ends`) is an end this broker
    /// delivers. A provider then gives the end to the commit
    /// ([`crate::surface::SurfaceProvider::end_for`]) and only wakes the
    /// registration afterwards ([`CallBroker::wake`]); with a broker
    /// that answers false it records the end here
    /// ([`CallBroker::ended`]).
    fn commits_ends(&self) -> bool;

    /// Wakes the registration: something waits for its host that the
    /// broker was not told of when it was written.
    fn wake(&self, registration: &str);

    /// The whole handle of the item of content `id` that `run` holds, as
    /// the script was given it (without `meta`, which is the provider's
    /// words of one answer): what a handle a script passes to a host is
    /// checked against. `None` for an item the run does not hold, and
    /// for every item when the broker's store keeps no content (the
    /// default).
    ///
    /// # Errors
    /// Why the broker's store could not be read, in words.
    fn content(&self, run: &str, id: &str) -> Result<Option<Value>, String> {
        let _ = (run, id);
        Ok(None)
    }

    /// Records that `run` ended as `end` says, for the registration to
    /// be told with each `serve` until it acknowledges. `result` is the
    /// kind's result, for a run that finished. Recording the same end
    /// again changes nothing. Answers the end's ID.
    ///
    /// # Errors
    /// Why nothing was recorded, in words.
    fn ended(
        &self,
        registration: &str,
        run: &str,
        end: End,
        result: Option<Value>,
    ) -> Result<String, String>;

    /// Tells the registration how `run`, a run it started, is going
    /// (`event`, as [`clax_surface::wire::Event`] documents it), with
    /// its next `serve`. Lossy: nothing keeps an event the registration
    /// is not there for. A broker that delivers no events does
    /// nothing, which is what this does unless it is written.
    fn progress(&self, _registration: &str, _run: &str, _event: Value) {}
}

/// The clock of a [`HubBroker`]: the same milliseconds its store keeps
/// deadlines in.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait BrokerClock: MaybeSendSync {
    fn now(&self) -> Millis;

    /// Returns when the clock reads `at` or later. It may return
    /// before: whoever waits reads the clock again.
    async fn until(&self, at: Millis);
}

/// A [`BrokerClock`] for tests that nothing moves but its holder
/// ([`SkipClock::set`], [`SkipClock::advance`]) and a wait: one thread
/// does nothing while it waits, so a wait is over when its instant has
/// come, and the clock then reads that instant.
#[derive(Debug, Default)]
pub struct SkipClock(AtomicU64);

impl SkipClock {
    /// A clock that reads `ms`.
    #[must_use]
    pub fn at(ms: u64) -> SkipClock {
        SkipClock(AtomicU64::new(ms))
    }

    /// Moves the clock to `ms`, when that is later than it reads.
    pub fn set(&self, ms: u64) {
        self.0.fetch_max(ms, Ordering::SeqCst);
    }

    /// Moves the clock on by `ms`.
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl BrokerClock for SkipClock {
    fn now(&self) -> Millis {
        Millis(self.0.load(Ordering::SeqCst))
    }

    async fn until(&self, at: Millis) {
        self.set(at.0);
    }
}

/// What a [`HubBroker`] needs of the store its hub keeps calls in,
/// beside what the hub does.
pub trait BrokerStore: Pending {
    /// [`Pending::open_call`] for the call `at` describes: the one
    /// call of one attempt that a run's driver is making now. A store
    /// whose row of a call holds more than the hub gives it (the step
    /// and the place, the attempt, the arguments' hash, whether the
    /// run may park on it) and is written under the driver's claim
    /// takes them from `at`; one that keeps nothing of them opens the
    /// call as [`Pending::open_call`] does.
    ///
    /// # Errors
    /// The store's. A claim that is lost is one.
    fn open_placed(
        &mut self,
        at: &Placed<'_>,
        call: NewCall,
        limit: usize,
        now: Millis,
    ) -> Result<OpenedCall, Self::Error>;

    /// Whether this store is the one the runs are in, and keeps a run's
    /// end written with the run's terminal commit
    /// ([`CallBroker::commits_ends`]). False, unless the store says
    /// otherwise.
    fn commits_ends(&self) -> bool {
        false
    }

    /// The outcome of the call `id` once it is settled, read and left
    /// where it is: `None` while it is open, and for a call the store
    /// does not hold. A store that journals a call's answer at its
    /// place answers what that row holds.
    ///
    /// # Errors
    /// The store's.
    fn settled(&mut self, id: &str) -> Result<Option<Result<Value, CallFailure>>, Self::Error>;

    /// The whole handle of the item of content `id` that the run `run`
    /// holds ([`CallBroker::content`]): `None` for one it does not, and
    /// for every item in a store that keeps no content (the default).
    ///
    /// # Errors
    /// The store's.
    fn content_handle(&mut self, run: &str, id: &str) -> Result<Option<Value>, Self::Error> {
        let _ = (run, id);
        Ok(None)
    }
}

impl BrokerStore for MemoryPending {
    fn open_placed(
        &mut self,
        _at: &Placed<'_>,
        call: NewCall,
        limit: usize,
        now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        self.open_call(call, limit, now)
    }

    fn settled(&mut self, id: &str) -> Result<Option<Result<Value, CallFailure>>, Self::Error> {
        Ok(self.outcome_of(id).cloned())
    }
}

/// Has `hub` make `call` for `registration` over `store`, opened at the
/// place `at` describes ([`BrokerStore::open_placed`]), as
/// [`CallBroker::send`] does: for whoever keeps a hub and its store apart
/// from a [`HubBroker`] (a test that plays a hub over the store the runs
/// are in).
///
/// # Errors
/// As [`Hub::enqueue`].
pub fn enqueue_placed<S: BrokerStore>(
    hub: &mut Hub,
    store: &mut S,
    at: &Placed<'_>,
    registration: &str,
    call: Enqueue,
    now: Millis,
) -> Result<String, EnqueueError> {
    hub.enqueue(&mut Placing { store, at }, registration, call, now)
}

/// A [`BrokerStore`] as the hub sees it while one call is opened: every
/// method is the store's, but [`Pending::open_call`], which is
/// [`BrokerStore::open_placed`] with the call's place.
struct Placing<'a, S> {
    store: &'a mut S,
    at: &'a Placed<'a>,
}

impl<S: BrokerStore> Pending for Placing<'_, S> {
    type Error = S::Error;

    fn issue_registration(
        &mut self,
        id: &str,
        name: &str,
        hash: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        self.store.issue_registration(id, name, hash, now)
    }
    fn registration_name(&mut self, id: &str) -> Result<Option<String>, Self::Error> {
        self.store.registration_name(id)
    }
    fn registration_settings(&mut self, id: &str) -> Result<Option<Value>, Self::Error> {
        self.store.registration_settings(id)
    }
    fn keep_definition(
        &mut self,
        hash: &str,
        canonical: &str,
        now: Millis,
    ) -> Result<(), Self::Error> {
        self.store.keep_definition(hash, canonical, now)
    }
    fn definition_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        self.store.definition_text(hash)
    }
    fn keep_prompt(&mut self, hash: &str, text: &str, now: Millis) -> Result<(), Self::Error> {
        self.store.keep_prompt(hash, text, now)
    }
    fn prompt_text(&mut self, hash: &str) -> Result<Option<String>, Self::Error> {
        self.store.prompt_text(hash)
    }
    fn open_call(
        &mut self,
        call: NewCall,
        limit: usize,
        now: Millis,
    ) -> Result<OpenedCall, Self::Error> {
        self.store.open_placed(self.at, call, limit, now)
    }
    fn pending_for(&mut self, registration: &str) -> Result<PendingSummary, Self::Error> {
        self.store.pending_for(registration)
    }
    fn calls_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingCall>, Self::Error> {
        self.store.calls_for(registration, ids)
    }
    fn sent_calls(&mut self, ids: &[String]) -> Result<(), Self::Error> {
        self.store.sent_calls(ids)
    }
    fn accept_call(
        &mut self,
        registration: &str,
        id: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        self.store.accept_call(registration, id, now)
    }
    fn answer_call(
        &mut self,
        registration: &str,
        id: &str,
        outcome: Result<Value, CallFailure>,
        now: Millis,
    ) -> Result<Answered, Self::Error> {
        self.store.answer_call(registration, id, outcome, now)
    }
    fn keep_tries(
        &mut self,
        registration: &str,
        id: &str,
        tries: &Tries,
    ) -> Result<bool, Self::Error> {
        self.store.keep_tries(registration, id, tries)
    }
    fn resends(&mut self, run: &str, registration: &str) -> Result<u32, Self::Error> {
        self.store.resends(run, registration)
    }
    fn next_resend(&mut self, after: Millis) -> Result<Option<Millis>, Self::Error> {
        self.store.next_resend(after)
    }
    fn set_dropped(&mut self, id: &str, at: Option<Millis>) -> Result<(), Self::Error> {
        self.store.set_dropped(id, at)
    }
    fn is_dropped(&mut self, id: &str) -> Result<bool, Self::Error> {
        self.store.is_dropped(id)
    }
    fn served(&mut self, id: &str, now: Millis) -> Result<(), Self::Error> {
        self.store.served(id, now)
    }
    fn expire_calls(
        &mut self,
        now: Millis,
        failure: &dyn Fn(&CallSummary) -> CallFailure,
    ) -> Result<Vec<String>, Self::Error> {
        self.store.expire_calls(now, failure)
    }
    fn next_deadline(&mut self) -> Result<Option<Millis>, Self::Error> {
        self.store.next_deadline()
    }
    fn record_end(
        &mut self,
        end: NewEnd,
        limit: usize,
        now: Millis,
    ) -> Result<RecordedEnd, Self::Error> {
        self.store.record_end(end, limit, now)
    }
    fn ends_for(
        &mut self,
        registration: &str,
        ids: &[String],
    ) -> Result<Vec<PendingEnd>, Self::Error> {
        self.store.ends_for(registration, ids)
    }
    fn acknowledge_end(
        &mut self,
        registration: &str,
        id: &str,
        now: Millis,
    ) -> Result<bool, Self::Error> {
        self.store.acknowledge_end(registration, id, now)
    }
    fn expire_ends(&mut self, before: Millis) -> Result<usize, Self::Error> {
        self.store.expire_ends(before)
    }
}

/// How long, in milliseconds of the broker's clock, a wait for a call's
/// outcome waits before it settles deadlines again when settling them
/// failed ([`CallBroker::outcome`]): the store may be locked, full or
/// holding a row it cannot read, and asking again at once would spin.
pub const EXPIRE_AGAIN_MS: u64 = 1_000;

/// A [`CallBroker`] over a [`Hub`] and the store that keeps its calls,
/// behind one lock, by a clock. The lock is never held across a wait.
pub struct HubBroker<S> {
    inner: Mutex<(Hub, S)>,
    clock: Arc<dyn BrokerClock>,
    /// Told each time a use of the hub settled a call or recorded an
    /// end ([`Hub::settles`]): whoever waits for an outcome looks
    /// again.
    changed: tokio::sync::Notify,
}

impl<S> std::fmt::Debug for HubBroker<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubBroker").finish_non_exhaustive()
    }
}

impl<S: BrokerStore> HubBroker<S> {
    /// A broker over `hub` and `store`, by `clock`. The hub's wake hook
    /// ([`crate::bot_surfaces::Wake`]) is whatever `hub` was made with:
    /// it is called under the broker's lock, and does not call the
    /// broker back.
    #[must_use]
    pub fn new(hub: Hub, store: S, clock: Arc<dyn BrokerClock>) -> HubBroker<S> {
        HubBroker {
            inner: Mutex::new((hub, store)),
            clock,
            changed: tokio::sync::Notify::new(),
        }
    }

    /// Lends the hub and its store, with the instant it is: how a
    /// host's side (a `serve`, a registration) reaches them. When the
    /// use settled a call or recorded an end ([`Hub::settles`]),
    /// whoever waits for a call's outcome looks again afterwards, so an
    /// answer a `serve` brought reaches its run without a clock moving;
    /// a use that settled nothing wakes no one. `f` does not call the
    /// broker.
    pub fn with<R>(&self, f: impl FnOnce(&mut Hub, &mut S, Millis) -> R) -> R {
        let (out, settled) = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let (hub, store) = &mut *inner;
            let before = hub.settles();
            let out = f(hub, store, self.clock.now());
            (out, hub.settles() != before)
        };
        if settled {
            self.changed.notify_waiters();
        }
        out
    }

    /// The clock the broker was made with.
    #[must_use]
    pub fn clock(&self) -> &Arc<dyn BrokerClock> {
        &self.clock
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<S: BrokerStore + Send + 'static> CallBroker for HubBroker<S> {
    fn now(&self) -> Millis {
        self.clock.now()
    }

    fn registration(&self, id: &str, hash: Option<&str>) -> Option<Served> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (hub, store) = &mut *inner;
        let held = hub
            .registration(id)
            .map(|r| (r.name.to_string(), r.hash.to_string(), r.settings.clone()));
        let (name, own, settings) = match held.clone() {
            Some((name, own, settings)) => (name, Some(own), settings),
            None => {
                let name = store.registration_name(id).ok()??;
                // The settings its row keeps, or none: a registration's
                // row holds what the hub checked when it was registered.
                // Settings that cannot be read (a setting a later build
                // keeps) are not the defaults, which would widen what
                // they narrow: the registration is not served.
                let settings = match store.registration_settings(id) {
                    Ok(None | Some(Value::Null)) => Settings::default(),
                    Ok(Some(kept)) => match serde_json::from_value(kept) {
                        Ok(settings) => settings,
                        Err(e) => {
                            tracing::warn!(
                                "clax-bot: the settings registration {id} keeps are not settings \
                                 this build reads ({e}); it is not served"
                            );
                            return None;
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            "clax-bot: the settings of registration {id} could not be read: {e}"
                        );
                        return None;
                    }
                };
                (name, None, settings)
            }
        };
        let hash = hash.map(str::to_string).or(own)?;
        let definition = hub.kept_definition(store, &hash).ok()??;
        if definition.name() != name {
            return None;
        }
        let prefix = format!("{name}.");
        Some(Served {
            withheld: withheld(&definition, hub.limits().content)
                .into_iter()
                .map(|w| format!("{prefix}{}", w.op))
                .collect(),
            name,
            definition,
            hash,
            settings,
            held: held.is_some(),
        })
    }

    fn prompt(&self, hash: &str) -> Result<Option<String>, String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.1.prompt_text(hash).map_err(|e| e.to_string())
    }

    fn connected(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.0.connected(id, self.clock.now())
    }

    fn dropped(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (hub, store) = &mut *inner;
        match hub.dropped(store, id, self.clock.now()) {
            Ok(dropped) => dropped,
            Err(e) => {
                // Not known to be dropped: its calls wait for their host
                // until their deadlines, as those of a registration that
                // may resume do.
                tracing::warn!("clax-bot: whether registration {id} was dropped: {e}");
                false
            }
        }
    }

    fn send(&self, claim: &Claim<'_>, call: &Sending<'_>) -> Result<String, CallFailure> {
        let at = Placed {
            driver: claim.driver(),
            run: call.run,
            place: call.place,
            raw: call.raw,
            args_hash: args_hash(call.raw),
            waits: call.waits,
        };
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (hub, store) = &mut *inner;
        let enqueue = Enqueue {
            run: call.run.to_string(),
            op: call.op.to_string(),
            args: call.args.to_vec(),
            key: call.place.key.clone(),
            deadline: call.deadline,
            retry: call.retry,
            joined: call.joined.cloned(),
        };
        hub.enqueue_under(
            &mut Placing { store, at: &at },
            call.registration,
            call.definition,
            enqueue,
            self.clock.now(),
        )
        .map_err(|e| e.failure())
    }

    async fn outcome(&self, call: &str, until: Millis) -> Outcome {
        loop {
            // Listening before looking: an answer that arrives between
            // the look and the wait is not missed.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let now = self.clock.now();
            let wake = {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                let (hub, store) = &mut *inner;
                // Looking writes nothing, but when a deadline has come
                // and nothing has settled it yet (a daemon's timer
                // does; a broker without one has only this).
                let due = matches!(store.next_deadline(), Ok(Some(deadline)) if deadline <= now);
                let mut unsettled = false;
                if due && let Err(e) = hub.expire(store, now) {
                    tracing::warn!("clax-bot: settling the calls whose deadline has come: {e}");
                    unsettled = true;
                }
                match store.settled(call) {
                    Ok(Some(outcome)) => return Outcome::Settled(outcome),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!("clax-bot: reading the outcome of call {call}: {e}");
                    }
                }
                if now >= until {
                    return Outcome::NotYet;
                }
                // A deadline that comes before the instant asked for
                // settles its call: looked at then. One that has come and
                // could not be settled is tried again after a back-off by
                // the clock, not at once.
                if unsettled {
                    now.after(EXPIRE_AGAIN_MS).min(until)
                } else {
                    store
                        .next_deadline()
                        .ok()
                        .flatten()
                        .map_or(until, |d| d.min(until))
                }
            };
            tokio::select! {
                () = &mut changed => {}
                () = self.clock.until(wake) => {}
            }
        }
    }

    fn commits_ends(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.1.commits_ends()
    }

    fn wake(&self, registration: &str) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.0.notify(registration);
    }

    fn content(&self, run: &str, id: &str) -> Result<Option<Value>, String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.1.content_handle(run, id).map_err(|e| e.to_string())
    }

    fn ended(
        &self,
        registration: &str,
        run: &str,
        end: End,
        result: Option<Value>,
    ) -> Result<String, String> {
        self.with(|hub, store, now| {
            hub.ended(store, registration, run, end, result, now)
                .map_err(|e| e.to_string())
        })
    }

    fn progress(&self, registration: &str, run: &str, event: Value) {
        self.with(|hub, _, now| hub.event(registration, run, event, now));
    }
}

pub mod stored;
#[cfg(test)]
mod tests;
