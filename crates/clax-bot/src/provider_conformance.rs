//! The cases every [`SurfaceProvider`] passes, played by the cases as
//! the run loop plays a step ([`crate::surface`], "One script step, in
//! order"): a real claim, steps begun and begun again in the store, each
//! call through [`dispatch`], which routes it, checks its arguments and
//! journals what the provider leaves to the core.
//!
//! What they hold:
//!
//! - a read's answer is the provider's each time it is performed, and
//!   the journal's when the call comes with its entry, whatever the
//!   provider holds by then;
//! - a write made again under its key takes effect once and answers the
//!   same, with its entry and without it (a crash between a write and
//!   the core's journal entry), and its place has a row once it is
//!   answered, whoever wrote it;
//! - an answer says truly whether its place has a row: one made as
//!   written has one, and a failure made as unwritten
//!   ([`Answer::unwritten`]) has none, changes nothing, and is made
//!   again, not answered again, when its step is run again;
//! - a call the provider answers that it waits on ([`Answer::waiting`])
//!   leaves no journal row and is applied once for its key, however
//!   often it is made again before its answer; made again with the
//!   entry its answer was journaled as, it is answered from the entry;
//!   and such an answer from an operation that is not tagged `@waits`
//!   is the provider's error, refused and journaled by the core;
//! - a refusal a session answers is journaled once, changes nothing,
//!   and is answered the same when the step is run again, with its
//!   entry and without it;
//! - a session begun and dropped leaves nothing a later attempt cannot
//!   repeat: the later attempt's calls answer what the first answered,
//!   and the write stands once;
//! - a `begin` that fails leaves the step to be begun again, and
//!   nothing behind;
//! - `finish` answers the same parts on an attempt that replays a step
//!   as on the attempt that first ran it, and the parts commit, under
//!   names that are none of the core's ([`merge_fields`]);
//! - a step that ends [`StepEnd::Cancelled`] keeps what its calls wrote
//!   under their keys, answers no rows for the commit, and commits as
//!   the run's `stopped` step; a write that waits for the step's commit
//!   is not kept;
//! - an operation outside the binding's set does not reach the session
//!   through [`dispatch`], and a session handed one anyway refuses it
//!   and changes nothing;
//! - `ending` called for a second attempt of the terminal step answers
//!   the same fields and writes nothing more, also when the first
//!   attempt's `ending` failed after it wrote;
//! - `ended` after the terminal commit succeeds, and told a second time
//!   succeeds again and changes nothing.
//!
//! # A session checks its own binding
//!
//! One case hands the session an operation its binding does not have,
//! past the core, and requires a refusal that changes nothing. The core
//! refuses such a call before any session sees it, so a session that
//! trusted the core would pass every case played through [`dispatch`].
//! The check is asked of the session all the same, for two reasons. A
//! provider outside the process is reached by more callers than one
//! run loop, and what a run may do must not rest on each caller
//! checking. And for `clax` it is what its web operations always did:
//! the session itself refuses an operation the run did not declare
//! (check 2 of `crates/clax-bot/src/clax/web.rs`), and keeping that
//! check in the session is what keeps a run's web grant from being
//! decided in one place only.
//!
//! # Cases that do not apply
//!
//! Some cases need something of the provider that not every provider
//! has: a write that waits for its step's commit
//! ([`ProviderFixture::staged`]), hooks a fixture can make fail
//! ([`ProviderFixture::fail_once`]), a call it can make fail with
//! nothing written ([`ProviderFixture::unwritten`]), an operation that
//! waits ([`ProviderFixture::waiting`]) and a way to make one that does
//! not answer so ([`ProviderFixture::untagged`]), a read that answers otherwise the
//! second time it is performed ([`ProviderFixture::disturb`]), a record
//! of the hooks it was told ([`ProviderFixture::lifecycle`]). A fixture
//! that has none says why ([`Maybe::Lacks`], with a reason that says
//! something: an empty one fails), and the case is then not played. A
//! case that is
//! not played fails, with the fixture's reason, unless the caller of
//! [`check_provider`] named it among the cases it accepts as not
//! applicable; and a case so named that was played fails too. So what
//! a provider is not held to is written down twice, by the fixture and
//! by the test, and neither can leave a case out without the other.
//!
//! The cases' writes take effect when they are made
//! ([`ProviderFixture::write`]): for `clax` that is a host write
//! (`threads.reply`), not a draft write, which changes nothing outside
//! its session before the step commits and is what
//! [`ProviderFixture::staged`] is for.
//!
//! An implementation's tests build a [`ProviderFixture`] and call
//! [`check_provider`] with the cases it accepts as not applicable (none,
//! for a provider that can be played through all of them):
//!
//! ```ignore
//! for (name, result) in clax_bot::provider_conformance::check_provider(&fixture, &[]).await {
//!     result.unwrap_or_else(|e| panic!("{name}: {e}"));
//! }
//! ```

use crate::surface::{
    Answer, Binding, Bound, Call, CallPlace, Claim, End, Fields, Hook, Journal, Merged,
    PAYLOAD_KEYS, RunInfo, StepCx, StepEnd, StepParts, StepSession, SurfaceProvider, dispatch,
    merge_fields, no_place, perform, recorded, route,
};
use clax_bot_sandbox::{CancelToken, HostError};
use clax_store::Store;
use clax_store::bot_runs::{CallEntry, DriverId, Step, StepCommit, StepKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One call a case makes: the operation as a script names it after
/// `clax.`, and its arguments, one for each parameter, as the surface's
/// file checks them.
#[derive(Clone, Debug, PartialEq)]
pub struct OpCall {
    pub op: String,
    pub args: Vec<Value>,
}

impl OpCall {
    #[must_use]
    pub fn new(op: &str, args: Value) -> OpCall {
        OpCall {
            op: op.to_string(),
            args: args.as_array().cloned().unwrap_or_default(),
        }
    }
}

/// Reads something of the provider's from outside the seam.
pub type Probe = Box<dyn Fn() -> Result<Value, String> + Send + Sync>;

/// Checks the hooks a provider received for the run of an ID.
pub type Lifecycle = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Makes the next call of a hook of the provider's fail, once: `Ok`
/// when it will, or why this hook of this provider cannot be made to
/// fail (the case that needs it is then not played, as for a fixture
/// that lacks the part: [`Maybe::Lacks`]).
pub type FailOnce = Box<dyn Fn(Hook) -> Result<(), &'static str> + Send + Sync>;

/// A reason a fixture gives for a case that cannot be played, which
/// must say something.
fn said(why: &'static str) -> Case {
    if why.trim().is_empty() {
        Err("the fixture gives no reason for a hook it cannot make fail".into())
    } else {
        Ok(Played::No(why))
    }
}

/// A call a fixture can make fail once with nothing written.
pub struct Unwritten {
    /// The call: one whose first performance after [`Unwritten::arm`]
    /// fails and leaves nothing (the provider answers
    /// [`Answer::unwritten`]), and which succeeds when it is made again.
    /// It adds nothing to its session's parts.
    pub call: OpCall,
    /// Makes the provider's next performance of the call fail so.
    pub arm: Box<dyn Fn() + Send + Sync>,
}

/// Counts how many times the call of a key was applied.
pub type Applied = Box<dyn Fn(&str) -> Result<u64, String> + Send + Sync>;

/// A call the provider holds and has no answer for.
pub struct Waiting {
    /// A call of an operation tagged `@waits` (and, where the surface
    /// has one, `@once`) that the provider answers
    /// [`Answer::waiting`] each time it is made with no entry. It adds
    /// nothing to its session's parts.
    pub call: OpCall,
    /// What the call is answered later: a value of the operation's
    /// declared result. The cases journal it at the call's place, as
    /// whoever brings a waiting call's answer does.
    pub answer: Value,
    /// How many times the call of a key (`<run>:<step>:<seq>`) was
    /// applied by whatever performs it: the provider, or what it sends
    /// its calls to.
    pub applied: Applied,
}

/// A call of an operation that is not tagged `@waits`, which a fixture
/// can make its provider answer [`Answer::waiting`] all the same.
pub struct Untagged {
    /// The call. It adds nothing to its session's parts.
    pub call: OpCall,
    /// Makes the provider's next performance of the call answer that
    /// it waits.
    pub arm: Box<dyn Fn() + Send + Sync>,
}

/// A write that takes effect only when its step commits.
pub struct Staged {
    pub write: OpCall,
    /// What the write changes once its step has committed, and not
    /// before.
    pub state: Probe,
}

/// Something a case needs of a provider, or why the provider has none.
pub enum Maybe<T> {
    Has(T),
    /// The provider has none, and why: the reason a case that needs it
    /// is not played ([`check_provider`]). A reason that says nothing
    /// (empty, or blank) fails the suite.
    Lacks(&'static str),
}

/// How a case came out, when nothing it holds was broken.
enum Played {
    Yes,
    /// The case could not be played against this provider, and why.
    No(&'static str),
}

/// The reason a fixture gives for lacking `part`, when it lacks it.
fn reason<T>(part: &Maybe<T>) -> Option<&'static str> {
    match part {
        Maybe::Has(_) => None,
        Maybe::Lacks(why) => Some(why),
    }
}

/// A case's result, before the caller's list of the cases it accepts
/// as not applicable is applied.
type Case = Result<Played, String>;

/// A case's result: what the provider did otherwise, in words.
pub type CaseResult = Result<(), String>;

/// What the cases run against.
pub struct ProviderFixture {
    pub provider: Arc<dyn SurfaceProvider>,
    /// The store the runs are in, where the cases claim them, begin
    /// their steps and journal.
    pub store: Arc<Store>,
    /// A run of the store that has not ended and that no driver holds,
    /// with a binding of the provider's surface. The cases run a script
    /// step of it and commit that step.
    pub run: RunInfo,
    /// A read of the surface.
    pub read: OpCall,
    /// Changes what [`ProviderFixture::read`] answers, from outside the
    /// run; or why the fixture has none: the read moves by itself, or
    /// the cases' own write moves it. The case of the replayed read is
    /// played only when the read, performed again, answers otherwise
    /// than it first did: a read that nothing moves cannot tell an
    /// answer from the journal from one made afresh.
    pub disturb: Maybe<Box<dyn Fn() + Send + Sync>>,
    /// A write of the surface that takes effect when it is made, and
    /// changes what [`ProviderFixture::state`] reads each time it is
    /// made under a new key.
    pub write: OpCall,
    /// A second write that changes it again.
    pub second_write: OpCall,
    /// What the writes change.
    pub state: Probe,
    /// A call of an operation the binding has that the session refuses,
    /// changing nothing: a rule of the run's, say.
    pub refused: OpCall,
    /// An operation of the surface's file that [`ProviderFixture::run`]'s
    /// binding does not have.
    pub outside: OpCall,
    /// Another such run, whose script step the cases cancel.
    pub cancelled_run: RunInfo,
    /// A write that waits for its step's commit, or why the provider
    /// has none.
    pub staged: Maybe<Staged>,
    /// Another such run, which the cases end.
    pub ending_run: RunInfo,
    /// How they end it, and the kind's result when it finishes.
    pub end: End,
    pub result: Option<Value>,
    /// What `ending` writes (the notices it posts).
    pub noticed: Probe,
    /// A call the fixture can make fail with nothing written, or why
    /// it cannot.
    pub unwritten: Maybe<Unwritten>,
    /// A call the provider answers that it waits on, or why it has
    /// none.
    pub waiting: Maybe<Waiting>,
    /// A call of an operation not tagged `@waits` that the fixture can
    /// make the provider answer so, or why it cannot.
    pub untagged: Maybe<Untagged>,
    /// Makes the provider's next `begin` ([`Hook::Begin`]) or `ending`
    /// ([`Hook::Ending`]) fail, once; or why the fixture cannot make
    /// either fail.
    pub fail_once: Maybe<FailOnce>,
    /// Checks the hooks the provider received for a run, for a provider
    /// that records them
    /// ([`crate::surface::FakeSurfaceProvider::lifecycle`]); or why it
    /// records none.
    pub lifecycle: Maybe<Lifecycle>,
}

/// How long the cases' claim lasts.
const LEASE_SECS: i64 = 60;

/// One run as the cases drive it: claimed, at its next step.
struct Driven<'a> {
    f: &'a ProviderFixture,
    run: &'a RunInfo,
    binding: &'a Binding,
    driver: DriverId,
    step: i64,
    history: Vec<Step>,
    cancel: CancelToken,
}

impl<'a> Driven<'a> {
    fn new(f: &'a ProviderFixture, run: &'a RunInfo, surface_of: &OpCall) -> Result<Self, String> {
        let binding = route(&run.bindings, &surface_of.op)
            .ok_or("the fixture's run has no binding for its calls")?;
        let driver = DriverId::mint();
        f.store
            .claim_run(&run.id, &driver, LEASE_SECS)
            .map_err(|e| e.to_string())?
            .ok_or("the fixture's run cannot be claimed")?;
        let (_, steps) = f
            .store
            .run_with_steps(&run.id)
            .map_err(|e| e.to_string())?
            .ok_or("the fixture's run is gone")?;
        Ok(Driven {
            f,
            run,
            binding,
            step: f.store.next_step_n(&run.id).map_err(|e| e.to_string())?,
            history: steps
                .into_iter()
                .filter(|s| s.committed_at.is_some())
                .collect(),
            driver,
            cancel: CancelToken::new(),
        })
    }

    fn claim(&self) -> Claim<'_> {
        Claim::new(&self.f.store, &self.driver, &self.run.id)
    }

    /// Begins the step, or begins it again: the attempt's context.
    fn begin(&self, kind: StepKind) -> Result<StepCx<'_>, String> {
        let step = self
            .f
            .store
            .begin_step(&self.run.id, &self.driver, self.step, kind)
            .map_err(|e| format!("the step could not be begun: {e}"))?;
        Ok(StepCx::new(
            self.run,
            self.binding,
            self.step,
            step.attempt,
            &self.history,
            self.claim(),
            &self.cancel,
        ))
    }

    /// The journal row at place `seq` of the step, when there is one.
    fn row(&self, seq: i64) -> Result<Option<CallEntry>, String> {
        Ok(self
            .f
            .store
            .bot_calls(&self.run.id, self.step)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| c.seq() == seq))
    }

    /// A new attempt of the script step, and its session.
    async fn attempt(&self) -> Result<Attempt<'_>, String> {
        self.f.provider.working(self.run).await;
        let cx = self.begin(StepKind::Script)?;
        let session = self
            .f
            .provider
            .begin(cx)
            .await
            .map_err(|e| format!("the session could not be opened: {e}"))?;
        let journal = self
            .f
            .store
            .bot_calls(&self.run.id, self.step)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| (c.seq(), c))
            .collect();
        Ok(Attempt {
            d: self,
            cx,
            session,
            journal,
        })
    }
}

/// One attempt of the script step.
struct Attempt<'a> {
    d: &'a Driven<'a>,
    cx: StepCx<'a>,
    session: Box<dyn StepSession + 'a>,
    /// The step's journal as the attempt found it.
    journal: BTreeMap<i64, CallEntry>,
}

/// How a case makes a call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    /// As the run loop does: with the place's entry when the attempt
    /// found one.
    Loop,
    /// Without an entry though one may be there: the write happened and
    /// the core's entry was lost.
    EntryLost,
    /// Straight to the session, past [`perform`].
    Bypass,
}

impl Attempt<'_> {
    async fn call(&self, seq: i64, c: &OpCall, how: How) -> Result<Answer, String> {
        let binding = route(&self.d.run.bindings, &c.op)
            .ok_or_else(|| format!("{} names no surface of the run", c.op))?;
        let Some(place): Option<CallPlace> = self.cx.place(seq) else {
            return Ok(Answer::refused(no_place(seq)));
        };
        let raw = Value::Array(c.args.clone());
        let call = Call {
            place: &place,
            op: &c.op,
            args: &c.args,
            raw: &raw,
            replay: None,
        };
        let provider = &*self.d.f.provider;
        Ok(match how {
            How::Loop => {
                let bound = [Bound {
                    binding,
                    provider,
                    session: &*self.session,
                }];
                let claim = &self.cx.claim;
                let replay = self.journal.get(&seq);
                let answer = dispatch(&bound, claim, &place, &c.op, &raw, replay).await;
                if replay.is_none() {
                    self.says_truly(seq, &c.op, &answer)?;
                }
                answer
            }
            How::EntryLost => {
                perform(provider, &*self.session, binding, &self.cx.claim, call).await
            }
            How::Bypass => self.session.call(call).await,
        })
    }

    /// Whether `answer`, to a call at `seq` that came with no entry,
    /// says truly whether the place has a row now: one made as written
    /// has a row, one made as unwritten has none.
    fn says_truly(&self, seq: i64, op: &str, answer: &Answer) -> Result<(), String> {
        let row = self.d.row(seq)?;
        match (answer.journal(), row) {
            (Journal::Written, None) => Err(format!(
                "{op} was answered as written and its place has no journal row"
            )),
            (Journal::Unwritten, Some(_)) => Err(format!(
                "{op} was answered as unwritten and its place has a journal row"
            )),
            (Journal::Unwritten, None) if answer.result().is_ok() => Err(format!(
                "{op} was answered a value as unwritten: only a failure leaves nothing"
            )),
            (Journal::Waiting, Some(_)) => Err(format!(
                "{op} was answered as waiting and its place has a journal row"
            )),
            _ => Ok(()),
        }
    }

    async fn made(&self, seq: i64, c: &OpCall) -> Result<Result<Value, HostError>, String> {
        Ok(self.call(seq, c, How::Loop).await?.into_result())
    }

    fn finish(self, end: StepEnd) -> StepParts {
        self.session.finish(end)
    }
}

/// `parts`' payload fields as the step's payload has them, merged in
/// for the binding of `surface`.
fn payload_of(
    surface: &str,
    parts: &StepParts,
    mut payload: Fields,
    merged: &mut Merged,
) -> Result<Fields, String> {
    let fields = parts.payload.clone();
    merge_fields(&mut payload, merged, &PAYLOAD_KEYS, surface, fields)
        .map_err(|e| e.to_string())?;
    Ok(payload)
}

fn same(
    what: &str,
    first: &Result<Value, HostError>,
    again: &Result<Value, HostError>,
) -> Result<(), String> {
    if first == again {
        Ok(())
    } else {
        Err(format!("{what} answered {again:?}, the first {first:?}"))
    }
}

/// The places the cases' calls take in the script step.
const READ: i64 = 0;
const WRITE: i64 = 1;
const OUTSIDE: i64 = 2;
const REFUSED: i64 = 3;
const SECOND_WRITE: i64 = 4;
const UNWRITTEN: i64 = 5;
const WAITING: i64 = 6;
const UNTAGGED: i64 = 7;

/// The results of the cases, each by its name.
type Results = Vec<(&'static str, Case)>;

fn held(r: Result<(), String>) -> Case {
    r.map(|()| Played::Yes)
}

/// The script step of the fixture's run: every case but the last, in
/// the order a step's attempts come.
async fn the_step(f: &ProviderFixture, out: &mut Results) {
    let d = match Driven::new(f, &f.run, &f.read) {
        Ok(d) => d,
        Err(e) => return out.push(("the fixture's run can be driven", Err(e))),
    };
    // What the first attempt answered, for the attempts after it.
    let mut firsts: BTreeMap<i64, Result<Value, HostError>> = BTreeMap::new();
    let mut state_after_write = Value::Null;

    out.push((
        "a_write_repeated_with_its_key_applies_once_and_answers_the_same",
        async {
            let before = (f.state)()?;
            let first = d.attempt().await?;
            firsts.insert(READ, first.made(READ, &f.read).await?);
            let answer = first.call(WRITE, &f.write, How::Loop).await?;
            let wrote = answer.result().clone();
            if let Err(e) = &wrote {
                return Err(format!("the write failed: {e:?}"));
            }
            // Whoever journals it, the place has its row once the call
            // is answered: a replay is told from it, and the call is
            // audited from it when no attempt commits.
            let by = match answer.journal() {
                Journal::Written => {
                    "the provider answered `written`, so the row is its own to write"
                }
                Journal::Answer | Journal::Outcome => "the core writes it",
                Journal::Unwritten => {
                    return Err("the write succeeded and was answered as unwritten".into());
                }
                Journal::Waiting => {
                    return Err("the fixture's write was answered as waiting".into());
                }
            };
            let row = d
                .row(WRITE)?
                .ok_or_else(|| format!("the write's place has no journal row: {by}"))?;
            if !row.ok() || row.op() != f.write.op {
                return Err(format!(
                    "the row at the write's place is of {} and says ok: {} ({by})",
                    row.op(),
                    row.ok()
                ));
            }
            state_after_write = (f.state)()?;
            if state_after_write == before {
                return Err("the write changed nothing".into());
            }
            // The first attempt is dropped here: a crash.
            drop(first);
            let second = d.attempt().await?;
            let lost = second
                .call(WRITE, &f.write, How::EntryLost)
                .await?
                .into_result();
            same("the write made again without its entry", &wrote, &lost)?;
            if (f.state)()? != state_after_write {
                return Err("the write made again without its entry wrote again".into());
            }
            let replayed = second.made(WRITE, &f.write).await?;
            same("the write made again with its entry", &wrote, &replayed)?;
            if (f.state)()? != state_after_write {
                return Err("the write made again with its entry wrote again".into());
            }
            firsts.insert(WRITE, wrote);
            Ok(())
        }
        .await
        .map(|()| Played::Yes),
    ));

    out.push((
        "an_unwritten_failure_leaves_no_row_and_is_made_again",
        async {
            let u = match &f.unwritten {
                Maybe::Has(u) => u,
                Maybe::Lacks(why) => return Ok(Played::No(why)),
            };
            let before = (f.state)()?;
            (u.arm)();
            let first = d.attempt().await?;
            let answer = first.call(UNWRITTEN, &u.call, How::Loop).await?;
            if answer.result().is_ok() {
                return Err("the fixture's `unwritten` did not make the call fail".into());
            }
            if answer.journal() != Journal::Unwritten {
                return Err(format!(
                    "the call failed as {:?}: the fixture's `unwritten` is a call the provider \
                     answers `Answer::unwritten`",
                    answer.journal()
                ));
            }
            if d.row(UNWRITTEN)?.is_some() {
                return Err("an unwritten failure has a journal row".into());
            }
            if (f.state)()? != before {
                return Err("an unwritten failure changed the provider's state".into());
            }
            // The attempt is dropped. The next finds no entry at the
            // place, makes the call again, and is answered afresh.
            drop(first);
            let second = d.attempt().await?;
            if second.journal.contains_key(&UNWRITTEN) {
                return Err("the next attempt found an entry at the unwritten place".into());
            }
            let again = second.call(UNWRITTEN, &u.call, How::Loop).await?;
            match again.result() {
                Ok(_) => Ok(Played::Yes),
                Err(e) => Err(format!(
                    "the call made again answered {e:?}: it was answered again, not made again"
                )),
            }
        }
        .await,
    ));

    out.push((
        "a_waiting_call_leaves_no_row_is_applied_once_and_is_answered_from_its_entry",
        async {
            let w = match &f.waiting {
                Maybe::Has(w) => w,
                Maybe::Lacks(why) => return Ok(Played::No(why)),
            };
            let op = &w.call.op;
            if !f
                .provider
                .definition()
                .operation(op)
                .is_some_and(|o| o.op.waits)
            {
                return Err(format!(
                    "the fixture's `waiting` call is of {op}, which the definition does not tag \
                     @waits"
                ));
            }
            let waits = |answer: &Answer, when: &str| {
                if answer.is_waiting() && answer.journal() == Journal::Waiting {
                    Ok(())
                } else {
                    Err(format!(
                        "{op}, {when}, answered {:?}: the fixture's `waiting` is a call the \
                         provider holds unanswered",
                        answer.result()
                    ))
                }
            };
            let first = d.attempt().await?;
            let place = first
                .cx
                .place(WAITING)
                .ok_or("the step has no place for the waiting call")?;
            waits(
                &first.call(WAITING, &w.call, How::Loop).await?,
                "first made",
            )?;
            if d.row(WAITING)?.is_some() {
                return Err("a call that waits has a journal row".into());
            }
            let applied = (w.applied)(&place.key)?;
            if applied != 1 {
                return Err(format!(
                    "the call that waits was applied {applied} times under its key"
                ));
            }
            // The attempt is dropped, as a run that parks drops it. The
            // step run again before the answer is in finds no entry and
            // makes the call again under its key: the same call.
            drop(first);
            let second = d.attempt().await?;
            if second.journal.contains_key(&WAITING) {
                return Err("the next attempt found an entry at the waiting place".into());
            }
            waits(
                &second.call(WAITING, &w.call, How::Loop).await?,
                "made again before its answer",
            )?;
            let applied = (w.applied)(&place.key)?;
            if applied != 1 {
                return Err(format!(
                    "the call made again under its key, still waiting, was applied {applied} \
                     times"
                ));
            }
            if d.row(WAITING)?.is_some() {
                return Err("a call that still waits has a journal row".into());
            }
            drop(second);
            // The answer arrives: whoever brings it journals it at the
            // call's place, in the row the core writes for a value.
            let raw = Value::Array(w.call.args.clone());
            d.claim()
                .record(&place, op, &raw, &Answer::of(Ok(w.answer.clone())));
            let third = d.attempt().await?;
            if !third.journal.contains_key(&WAITING) {
                return Err("the answer could not be journaled at the call's place".into());
            }
            let replayed = third.call(WAITING, &w.call, How::Loop).await?;
            if replayed.is_waiting() {
                return Err("the call made again with its entry still waits".into());
            }
            if replayed.result() != &Ok(w.answer.clone()) {
                return Err(format!(
                    "the call made again with its entry answered {:?}, and the entry holds {}",
                    replayed.result(),
                    w.answer
                ));
            }
            let applied = (w.applied)(&place.key)?;
            if applied != 1 {
                return Err(format!(
                    "the call answered from its entry was applied again: {applied} times"
                ));
            }
            Ok(Played::Yes)
        }
        .await,
    ));

    out.push((
        "a_waiting_answer_from_an_operation_not_tagged_waits_is_the_providers_error",
        async {
            let u = match &f.untagged {
                Maybe::Has(u) => u,
                Maybe::Lacks(why) => return Ok(Played::No(why)),
            };
            let op = &u.call.op;
            if f.provider
                .definition()
                .operation(op)
                .is_none_or(|o| o.op.waits)
            {
                return Err(format!(
                    "the fixture's `untagged` call is of {op}, which is to be an operation the \
                     definition does not tag @waits"
                ));
            }
            (u.arm)();
            let attempt = d.attempt().await?;
            // Straight to the session it waits, which shows the fixture
            // armed it; through the core it is refused.
            let answer = attempt.call(UNTAGGED, &u.call, How::Loop).await?;
            if answer.is_waiting() {
                return Err(format!(
                    "{op} is not tagged @waits and its waiting answer came through the core"
                ));
            }
            match answer.result() {
                Err(e) if e.code == crate::surface::CODE_PROVIDER_ERROR => {}
                other => {
                    return Err(format!(
                        "{op} answered {other:?}: the fixture's `untagged` did not make it \
                         answer that it waits, or the core did not refuse that"
                    ));
                }
            }
            let row = d
                .row(UNTAGGED)?
                .ok_or("the refusal of a waiting answer was not journaled")?;
            if row.ok() {
                return Err("the refusal of a waiting answer was journaled as a success".into());
            }
            Ok(Played::Yes)
        }
        .await,
    ));

    out.push((
        "a_reads_answer_is_the_providers_and_its_replay_the_journals",
        async {
            let first = firsts.get(&READ).ok_or("the read was not made")?.clone();
            if let Maybe::Has(disturb) = &f.disturb {
                disturb();
            }
            let attempt = d.attempt().await?;
            if !attempt.journal.contains_key(&READ) {
                return Err("the read's answer was not journaled".into());
            }
            // Performed again it is the provider's answer now; with its
            // entry it is the first's.
            let fresh = attempt
                .call(READ, &f.read, How::Bypass)
                .await?
                .into_result();
            let replayed = attempt.made(READ, &f.read).await?;
            same("the read made again with its entry", &first, &replayed)?;
            // The two can be told apart only when the read has moved.
            match (&f.disturb, fresh != first) {
                (_, true) => Ok(Played::Yes),
                (Maybe::Has(_), false) => Err(format!(
                    "the fixture's `disturb` did not change what {} answers",
                    f.read.op
                )),
                (Maybe::Lacks(why), false) => Ok(Played::No(why)),
            }
        }
        .await,
    ));

    out.push((
        "an_operation_outside_the_binding_never_reaches_the_provider",
        async {
            let attempt = d.attempt().await?;
            let before = (f.state)()?;
            let refused = attempt.made(OUTSIDE, &f.outside).await?;
            if let Ok(v) = &refused {
                return Err(format!("{} answered {v} through the core", f.outside.op));
            }
            // Past the core: the session checks its own binding (see
            // the module documentation).
            let bypassed = attempt
                .call(OUTSIDE, &f.outside, How::Bypass)
                .await?
                .into_result();
            if let Ok(v) = &bypassed {
                return Err(format!(
                    "{} answered {v} when handed to the session",
                    f.outside.op
                ));
            }
            if (f.state)()? != before {
                return Err(format!("{} changed the provider's state", f.outside.op));
            }
            firsts.insert(OUTSIDE, refused);
            Ok(())
        }
        .await
        .map(|()| Played::Yes),
    ));

    out.push((
        "a_sessions_refusal_is_journaled_once_and_answered_the_same_on_replay",
        async {
            let before = (f.state)()?;
            let first = d.attempt().await?;
            let answer = first.call(REFUSED, &f.refused, How::Loop).await?;
            let refused = answer.result().clone();
            if let Ok(v) = &refused {
                return Err(format!(
                    "{} answered {v}: the session did not refuse it",
                    f.refused.op
                ));
            }
            let row = d
                .row(REFUSED)?
                .ok_or("the refusal was not journaled: the next attempt would ask again")?;
            if row.ok() {
                return Err("the refusal was journaled as a success".into());
            }
            // A row the core wrote holds the refusal as it was answered;
            // one the provider wrote first is in the provider's own
            // form, and is held by what the call made again answers.
            if answer.journal() != Journal::Written && recorded(&row) != refused {
                return Err(format!(
                    "the row at the refusal's place holds {:?} and the call answered {refused:?}: \
                     the provider wrote a row of its own and left the answer to the core",
                    recorded(&row)
                ));
            }
            drop(first);
            let second = d.attempt().await?;
            let replayed = second.made(REFUSED, &f.refused).await?;
            same(
                "the refused call made again with its entry",
                &refused,
                &replayed,
            )?;
            let lost = second
                .call(REFUSED, &f.refused, How::EntryLost)
                .await?
                .into_result();
            same(
                "the refused call made again without its entry",
                &refused,
                &lost,
            )?;
            if (f.state)()? != before {
                return Err(format!(
                    "{}, refused, changed the provider's state",
                    f.refused.op
                ));
            }
            firsts.insert(REFUSED, refused);
            Ok(())
        }
        .await
        .map(|()| Played::Yes),
    ));

    out.push((
        "a_begin_that_fails_leaves_the_step_to_be_begun_again",
        async {
            let fail = match &f.fail_once {
                Maybe::Has(fail) => fail,
                Maybe::Lacks(why) => return Ok(Played::No(why)),
            };
            let before = (f.state)()?;
            let rows = || {
                f.store
                    .bot_calls(&f.run.id, d.step)
                    .map(|rows| rows.len())
                    .map_err(|e| e.to_string())
            };
            let journaled = rows()?;
            if let Err(why) = fail(Hook::Begin) {
                return said(why);
            }
            if d.attempt().await.is_ok() {
                return Err("the fixture's `fail_once` did not make `begin` fail".into());
            }
            if (f.state)()? != before {
                return Err("a `begin` that failed changed the provider's state".into());
            }
            if rows()? != journaled {
                return Err("a `begin` that failed changed the step's journal".into());
            }
            // The step is still open, and is begun again by the next
            // case: a session opens, and answers what the first did.
            Ok(Played::Yes)
        }
        .await,
    ));

    let mut parts: Option<StepParts> = None;
    out.push((
        "an_abandoned_session_leaves_nothing_a_later_attempt_cannot_repeat",
        async {
            // Every attempt before this one was dropped.
            let attempt = d.attempt().await?;
            for (seq, call) in [
                (READ, &f.read),
                (WRITE, &f.write),
                (OUTSIDE, &f.outside),
                (REFUSED, &f.refused),
            ] {
                let first = firsts.get(&seq).ok_or("an earlier case did not run")?;
                same(&call.op, first, &attempt.made(seq, call).await?)?;
            }
            if (f.state)()? != state_after_write {
                return Err("an attempt that replayed the step changed what it wrote".into());
            }
            let more = attempt.made(SECOND_WRITE, &f.second_write).await?;
            if let Err(e) = &more {
                return Err(format!("the step's next write failed: {e:?}"));
            }
            if (f.state)()? == state_after_write {
                return Err("the step's next write changed nothing".into());
            }
            firsts.insert(SECOND_WRITE, more);
            parts = Some(attempt.finish(StepEnd::Commit));
            Ok(())
        }
        .await
        .map(|()| Played::Yes),
    ));

    out.push((
        "finish_answers_the_same_parts_on_a_replayed_attempt",
        async {
            let first = parts.clone().ok_or("the step was not finished")?;
            let state = (f.state)()?;
            let attempt = d.attempt().await?;
            for (seq, call) in [
                (READ, &f.read),
                (WRITE, &f.write),
                (OUTSIDE, &f.outside),
                (REFUSED, &f.refused),
                (SECOND_WRITE, &f.second_write),
            ] {
                let was = firsts.get(&seq).ok_or("an earlier case did not run")?;
                same(&call.op, was, &attempt.made(seq, call).await?)?;
            }
            let again = attempt.finish(StepEnd::Commit);
            if again != first {
                return Err(format!(
                    "the replayed attempt's parts are {again:?}, the first's {first:?}"
                ));
            }
            if (f.state)()? != state {
                return Err("the replayed attempt wrote again".into());
            }
            let ctx = crate::run::bot_ctx(&f.run.person);
            f.store
                .commit_bot_step(
                    &ctx,
                    &f.run.id,
                    &d.driver,
                    d.step,
                    StepCommit {
                        payload: Value::Object(payload_of(
                            &d.binding.surface,
                            &again,
                            Fields::new(),
                            &mut Merged::new(),
                        )?),
                        draft: again.draft,
                        ..StepCommit::default()
                    },
                )
                .map_err(|e| format!("the step's parts do not commit: {e}"))?;
            Ok(())
        }
        .await
        .map(|()| Played::Yes),
    ));
    let _ = f.store.release_run(&f.run.id, &d.driver);
}

/// A script step of the fixture's cancelled run: a write that stands,
/// a write that waits for the commit, the script cancelled, and the
/// step committed as the run's `stopped` step. The second result is of
/// the write that waits.
async fn the_cancelled_step(f: &ProviderFixture) -> (Case, Case) {
    let mut staged: Case = match &f.staged {
        Maybe::Has(_) => Err("the cancelled step did not get as far as its commit".into()),
        Maybe::Lacks(why) => Ok(Played::No(why)),
    };
    let stood = async {
        let run = &f.cancelled_run;
        let d = Driven::new(f, run, &f.write)?;
        let before = (f.state)()?;
        let attempt = d.attempt().await?;
        if let Err(e) = attempt.made(0, &f.write).await? {
            return Err(format!("the write failed: {e:?}"));
        }
        let written = (f.state)()?;
        if written == before {
            return Err("the write changed nothing".into());
        }
        let waiting = match &f.staged {
            Maybe::Lacks(_) => None,
            Maybe::Has(s) => {
                let before = (s.state)()?;
                if let Err(e) = attempt.made(1, &s.write).await? {
                    return Err(format!("the write that waits failed: {e:?}"));
                }
                Some((s, before))
            }
        };
        // The script is cancelled here.
        let session_attempt = attempt.cx.attempt;
        let parts = attempt.finish(StepEnd::Cancelled);
        if !parts.draft.is_empty() {
            return Err(format!(
                "a cancelled session answered {} rows for the step's commit",
                parts.draft.len()
            ));
        }
        if (f.state)()? != written {
            return Err("cancelling the step changed what its write wrote".into());
        }
        // The same step is begun again, as the run's `stopped` step.
        let end = End::stopped("cancelled");
        let cx = d.begin(StepKind::Stopped)?;
        if cx.attempt != session_attempt + 1 {
            return Err(format!(
                "the stopped step is attempt {} after the session's {session_attempt}",
                cx.attempt
            ));
        }
        let fields = f
            .provider
            .ending(cx, &end, None)
            .await
            .map_err(|e| format!("`ending` failed for a cancelled run: {e}"))?;
        let mut payload = Fields::new();
        payload.insert("reason".into(), json!(end.reason()));
        let mut merged = Merged::new();
        let surface = &d.binding.surface;
        let mut payload = payload_of(surface, &parts, payload, &mut merged)?;
        // The `ending`'s fields join the session's: a name both
        // answered is the provider's fault.
        merge_fields(&mut payload, &mut merged, &PAYLOAD_KEYS, surface, fields)
            .map_err(|e| e.to_string())?;
        // The commit keeps the end the provider names, as the run
        // loop's does.
        let ends: Vec<_> = f.provider.end_for(run, &end, None).into_iter().collect();
        f.store
            .commit_bot_step_on(
                &crate::run::bot_ctx(&run.person),
                &run.id,
                &d.driver,
                d.step,
                StepCommit {
                    payload: Value::Object(payload),
                    draft: parts.draft,
                    reason: end.reason().map(str::to_string),
                    ..StepCommit::default()
                },
                None,
                &ends,
            )
            .map_err(|e| format!("the cancelled step does not commit as stopped: {e}"))?;
        f.provider
            .ended(run, &end, None)
            .await
            .map_err(|e| format!("`ended` failed for a cancelled run: {e:?}"))?;
        if (f.state)()? != written {
            return Err("ending the cancelled run changed what its write wrote".into());
        }
        if let Some((s, before)) = waiting {
            staged = if (s.state)()? == before {
                Ok(Played::Yes)
            } else {
                Err(format!(
                    "{}, made in a step that was cancelled, was kept",
                    s.write.op
                ))
            };
        }
        Ok(())
    }
    .await;
    (held(stood), staged)
}

/// The terminal step of the fixture's ending run. The first result is
/// of an `ending` that failed in the step's first attempt.
async fn the_end(f: &ProviderFixture) -> (Case, Case) {
    let mut failed: Case = match &f.fail_once {
        Maybe::Has(_) => Err("the terminal step could not be begun".into()),
        Maybe::Lacks(why) => Ok(Played::No(why)),
    };
    let ended = async {
        let run = &f.ending_run;
        let d = Driven::new(f, run, &f.read)?;
        let kind = f.end.step_kind();
        let result = f.result.as_ref();
        let armed = match &f.fail_once {
            Maybe::Lacks(_) => false,
            Maybe::Has(fail) => match fail(Hook::Ending) {
                Ok(()) => true,
                Err(why) => {
                    failed = said(why);
                    false
                }
            },
        };
        if armed {
            // The attempt is abandoned, as for a commit that failed, and
            // the step begun again: every `ending` is called again.
            let cx = d.begin(kind)?;
            if f.provider.ending(cx, &f.end, result).await.is_ok() {
                failed = Err("the fixture's `fail_once` did not make `ending` fail".into());
                return Err("`ending` did not fail when the fixture made it".into());
            }
            // Made again in the step's next attempt, it answers.
            let cx = d.begin(kind)?;
            failed = match f.provider.ending(cx, &f.end, result).await {
                Ok(_) => Ok(Played::Yes),
                Err(e) => Err(format!(
                    "`ending` failed again in the attempt after the one the fixture failed: {e}"
                )),
            };
        }
        let first: Fields = f
            .provider
            .ending(d.begin(kind)?, &f.end, result)
            .await
            .map_err(|e| format!("`ending` failed: {e}"))?;
        let noticed = (f.noticed)()?;
        // The terminal step is begun again: a crash before its commit.
        let again = f
            .provider
            .ending(d.begin(kind)?, &f.end, result)
            .await
            .map_err(|e| format!("`ending` failed when its step was begun again: {e}"))?;
        if again != first {
            return Err(format!(
                "`ending` answered {again:?} when its step was begun again, {first:?} before"
            ));
        }
        if (f.noticed)()? != noticed {
            return Err("`ending` wrote again when its step was begun again".into());
        }
        let mut payload = Fields::new();
        payload.insert("reason".into(), json!(f.end.reason()));
        let surface = &d.binding.surface;
        merge_fields(
            &mut payload,
            &mut Merged::new(),
            &PAYLOAD_KEYS,
            surface,
            first,
        )
        .map_err(|e| e.to_string())?;
        let ends: Vec<_> = f
            .provider
            .end_for(run, &f.end, result)
            .into_iter()
            .collect();
        f.store
            .commit_bot_step_on(
                &crate::run::bot_ctx(&run.person),
                &run.id,
                &d.driver,
                d.step,
                StepCommit {
                    payload: Value::Object(payload),
                    reason: f.end.reason().map(str::to_string),
                    ..StepCommit::default()
                },
                None,
                &ends,
            )
            .map_err(|e| format!("the terminal step does not commit: {e}"))?;
        f.provider
            .ended(run, &f.end, result)
            .await
            .map_err(|e| format!("`ended` failed: {e:?}"))?;
        if (f.noticed)()? != noticed {
            return Err("`ended` changed what `ending` wrote".into());
        }
        let state = (f.state)()?;
        // Told again: the run loop could not tell that the first call
        // arrived.
        f.provider
            .ended(run, &f.end, result)
            .await
            .map_err(|e| format!("`ended` failed when it was told a second time: {e:?}"))?;
        if (f.noticed)()? != noticed || (f.state)()? != state {
            return Err("`ended`, told a second time, changed what the provider holds".into());
        }
        Ok(())
    }
    .await;
    (failed, held(ended))
}

/// Runs every case against `f`; each case's result by name.
///
/// `not_applicable` names the cases the caller accepts as not played
/// against this provider. A case the fixture could not have played
/// ([`Maybe::Lacks`]) fails with the fixture's reason unless it is
/// named there, and a case named there that was played, or that is no
/// case of the suite, fails too.
pub async fn check_provider(
    f: &ProviderFixture,
    not_applicable: &[&str],
) -> Vec<(&'static str, CaseResult)> {
    let mut out: Results = Vec::new();
    the_step(f, &mut out).await;
    let (stood, staged) = the_cancelled_step(f).await;
    let cancelled_ended = stood.is_ok();
    out.push((
        "a_cancelled_step_keeps_what_it_wrote_and_commits_as_stopped",
        stood,
    ));
    out.push((
        "a_cancelled_step_keeps_no_write_that_waited_for_its_commit",
        staged,
    ));
    let (failed, ended) = the_end(f).await;
    out.push((
        "an_ending_that_failed_is_made_again_in_the_next_attempt",
        failed,
    ));
    let both_ended = cancelled_ended && ended.is_ok();
    out.push((
        "ending_is_repeatable_inside_the_terminal_step_and_ended_after_its_commit",
        ended,
    ));
    out.push((
        "the_hooks_the_provider_was_told_are_what_the_run_loop_owes",
        match &f.lifecycle {
            Maybe::Lacks(why) => Ok(Played::No(why)),
            Maybe::Has(_) if !both_ended => {
                Err("the cancelled run and the run that was ended did not both end".into())
            }
            Maybe::Has(check) => [&f.cancelled_run, &f.ending_run]
                .iter()
                .try_for_each(|run| check(&run.id))
                .map(|()| Played::Yes),
        },
    ));
    let reasons = [
        ("disturb", reason(&f.disturb)),
        ("staged", reason(&f.staged)),
        ("unwritten", reason(&f.unwritten)),
        ("waiting", reason(&f.waiting)),
        ("untagged", reason(&f.untagged)),
        ("fail_once", reason(&f.fail_once)),
        ("lifecycle", reason(&f.lifecycle)),
    ];
    let unsaid: Vec<&str> = reasons
        .iter()
        .filter(|(_, why)| why.is_some_and(|w| w.trim().is_empty()))
        .map(|(field, _)| *field)
        .collect();
    out.push((
        "the_fixture_says_why_it_lacks_what_it_lacks",
        if unsaid.is_empty() {
            Ok(Played::Yes)
        } else {
            Err(format!(
                "the fixture gives no reason for lacking {unsaid:?}: `Maybe::Lacks` says why"
            ))
        },
    ));
    let mut results: Vec<(&'static str, CaseResult)> = out
        .into_iter()
        .map(|(name, case)| {
            let accepted = not_applicable.contains(&name);
            let result = match case {
                Err(e) => Err(e),
                Ok(Played::Yes) if accepted => Err(
                    "the case was played, and the caller names it as not applicable".to_string(),
                ),
                Ok(Played::Yes) => Ok(()),
                Ok(Played::No(_)) if accepted => Ok(()),
                Ok(Played::No(why)) => Err(format!(
                    "the case was not played ({why}), and the caller does not name it as not \
                     applicable"
                )),
            };
            (name, result)
        })
        .collect();
    let unknown: Vec<&&str> = not_applicable
        .iter()
        .filter(|n| !results.iter().any(|(name, _)| name == *n))
        .collect();
    results.push((
        "the_cases_named_as_not_applicable_are_cases_of_the_suite",
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(format!("{unknown:?} are no cases of the suite"))
        },
    ));
    results
}
