//! A script step: the turn's `run_script` code in the sandbox, every
//! call it makes given its place in the step, looked up in the step's
//! journal, and handed to the surface it is for through the seam
//! ([`crate::surface`], "One script step, in order").
//!
//! This module knows no surface. It holds what is the same whatever a
//! script calls:
//!
//! - the sandbox, its limits ([`crate::policy::RunLimits::script`]) and
//!   the step's fixed clock and random seed;
//! - a call's place ([`CallPlace::in_script`]) and the step's journal;
//! - the check that a script run again makes the calls it made: a call
//!   that is not the one recorded at its place ends the script with
//!   `replay_diverged`, and the step commits with what completed;
//! - [`dispatch`], which routes a call to its binding, checks its
//!   arguments against that surface's definition, asks the binding's
//!   session and journals what the session leaves to the run loop;
//! - what is told of each call: its event, its entry in the step's
//!   `calls`, its audit record and its part of the requests line;
//! - the step's commit: each session's parts joined ([`StepJoin`]), and
//!   what the model is told of the script, trimmed to
//!   [`crate::policy::RunLimits::tool_result_bytes`] after the sessions'
//!   fields are in it.
//!
//! What a call does, what a session keeps of its step and what a
//! cancelled step keeps are each surface's own
//! ([`crate::surface::StepSession`]).

use crate::api::CODE_REPLAY_DIVERGED;
use crate::ending::Ending;
use crate::events::Event;
use crate::host::args_hash;
use crate::registry::CODE_OPS_INVALID;
use crate::run::Observed;
use crate::run::{CANCELLED, Next, Tick, TickOutcome, Wait, compute_qty, step_usage, watched};
use crate::surface::{
    Answer, Binding, Bound, CallNote, CallPlace, Claim, End, FieldClash, Fields, RunInfo, StepCx,
    StepEnd, StepJoin, StepSession, SurfaceProvider, dispatch, no_place, route,
};
use clax_bot_sandbox::{ErrorKind, Host, HostError, HostReply, Script, ScriptError, ScriptOutcome};
use clax_core::audit::AuditRecord;
use clax_core::{CoreError, Result};
use clax_sample::messages::Block;
use clax_store::bot_runs::{
    CLAX_SURFACE, CallEntry, JournalPlace, Park, StepKind, call_record, script_record,
};
use clax_store::credits::{LINE_SCRIPT_COMPUTE, Metered};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

/// One surface a step's script can call: a binding of the run, and the
/// provider of its surface.
#[derive(Clone, Copy)]
pub(crate) struct Surface<'a> {
    pub binding: &'a Binding,
    pub provider: &'a dyn SurfaceProvider,
}

/// What the run loop keeps of one call that reached it.
struct CallLog {
    seq: i64,
    op: String,
    ok: bool,
    code: Option<String>,
    /// The call came with its journal entry and was the call the entry
    /// is of.
    replayed: bool,
    /// How long the call took, in milliseconds: 0 for one that was
    /// replayed, and for one the run loop refused without asking a
    /// session.
    ms: u64,
    /// What the call's surface says of it.
    note: CallNote,
}

/// The calls of one attempt, as they are made.
struct Calls {
    /// The next call's place.
    next: i64,
    log: Vec<CallLog>,
    /// Where the replay left the journal, and why.
    diverged: Option<(i64, String)>,
    /// The call the attempt waits on, when it waits on one.
    parked: Option<Waited>,
}

/// The call a script's attempt waits on.
struct Waited {
    /// Its place among the step's calls.
    seq: i64,
    op: String,
    /// Its key: `<run>:<step>:<seq>`.
    key: String,
    /// The surface that holds it.
    surface: String,
    /// When its wait ends if nothing answers it.
    deadline: Option<chrono::DateTime<chrono::Utc>>,
}

/// The code a script is thrown when its call would wait and its step
/// has parked as often as it may
/// ([`crate::policy::RunLimits::max_parks_per_step`]).
pub const CODE_PARK_LIMIT: &str = "park_limit";
/// The code a script is thrown when its call would wait and its person
/// has as many runs parked as they may
/// ([`crate::policy::RunLimits::max_parked_runs`]).
pub const CODE_PARKED_RUNS_LIMIT: &str = "parked_runs_limit";

/// The [`Host`] a script's calls reach.
struct StepHost<'a> {
    tick: &'a Tick<'a>,
    attempt: i64,
    claim: Claim<'a>,
    /// The step's journal as the attempt found it, by place.
    journal: BTreeMap<i64, CallEntry>,
    /// The step's bindings, each with its provider and its session.
    bound: Vec<Bound<'a>>,
    calls: Mutex<Calls>,
}

impl StepHost<'_> {
    /// Why the step may not park on a call to `op` that waits, when it
    /// may not: it has parked as often as the run's limits allow, or
    /// its person has as many runs parked as they may. What the script
    /// is thrown in the waiting answer's place, which the run loop
    /// journals, so that the call's surface is no longer waited on.
    fn may_not_park(&self, op: &str) -> Option<HostError> {
        let tick = self.tick;
        let limits = &tick.limits;
        if let Some(most) = limits.max_parks_per_step {
            let parks = tick
                .store
                .run_parking(&tick.run.id)
                .ok()
                .flatten()
                .map_or(0, |p| p.parks);
            if parks >= i64::from(most) {
                return Some(HostError::new(
                    CODE_PARK_LIMIT,
                    format!(
                        "clax.{op} has no answer yet, and this step has waited {parks} times \
                         already, which is the limit: go on without it"
                    ),
                ));
            }
        }
        if let Some(most) = limits.max_parked_runs {
            let parked = tick.store.parked_runs_of(&tick.run.person).unwrap_or(0);
            if parked >= i64::from(most) {
                return Some(HostError::new(
                    CODE_PARKED_RUNS_LIMIT,
                    format!(
                        "clax.{op} has no answer yet, and {parked} runs are waiting for this \
                         person already, which is the limit: go on without it"
                    ),
                ));
            }
        }
        None
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tells the run's events and the attempt's log of the call at `seq`
    /// and what it was answered, and answers the script.
    fn told(
        &self,
        seq: i64,
        op: &str,
        answer: Answer,
        replayed: bool,
        ms: u64,
    ) -> std::result::Result<Value, HostError> {
        let (ok, code) = match answer.result() {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e.code.clone())),
        };
        let note = answer.note().clone();
        self.tick
            .emit(self.attempt, Event::call(op, ok, code.clone(), ms, &note));
        self.lock().log.push(CallLog {
            seq,
            op: op.into(),
            ok,
            code,
            replayed,
            ms,
            note,
        });
        answer.into_result()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Host for StepHost<'_> {
    /// [`Host::ask`], for a caller that cannot wait: a call that waits
    /// is a failure to it.
    async fn call(&self, op: &str, args: Value) -> std::result::Result<Value, HostError> {
        match self.ask(op, args).await {
            HostReply::Answer(answer) => answer,
            HostReply::Waiting => Err(HostError::new(
                clax_bot_sandbox::CODE_WAITING,
                "the call has no answer yet",
            )),
        }
    }

    async fn ask(&self, op: &str, args: Value) -> HostReply {
        let (seq, diverged) = {
            let mut calls = self.lock();
            // The attempt waits on a call: no call after it is placed
            // or reaches a session, whatever the sandbox sends.
            if calls.parked.is_some() {
                return HostReply::Waiting;
            }
            calls.next += 1;
            (calls.next - 1, calls.diverged.clone())
        };
        // After a divergence no session sees a later call.
        if let Some((at, why)) = diverged {
            let stopped = HostError::new(
                CODE_REPLAY_DIVERGED,
                format!("the replay stopped at call {at}: {why}"),
            );
            return HostReply::Answer(self.told(seq, op, Answer::refused(stopped), false, 0));
        }
        let run = &self.tick.run.id;
        let Some(place) = CallPlace::in_script(run, self.tick.n, self.attempt, seq) else {
            let refused = Answer::refused(no_place(seq));
            return HostReply::Answer(self.told(seq, op, refused, false, 0));
        };
        let started = clax_rt::time::Instant::now();
        let replay = self.journal.get(&seq);
        let other = replay.filter(|e| e.op() != op || e.args_hash() != args_hash(&args));
        let (answer, replayed) = match other {
            // The journal holds another call at this place: neither
            // journaled nor performed.
            Some(e) => {
                let why = format!(
                    "call {seq} was {} before and is {op} now: the script took another path when run again",
                    e.op()
                );
                self.lock().diverged = Some((seq, why.clone()));
                let diverged = HostError::new(CODE_REPLAY_DIVERGED, why);
                (Answer::refused(diverged), false)
            }
            None => (
                dispatch(&self.bound, &self.claim, &place, op, &args, replay).await,
                replay.is_some(),
            ),
        };
        let ms = if replayed {
            0
        } else {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        // The call's surface holds the call and has no answer yet
        // ([`Answer::waiting`]): nothing is journaled, and the call is
        // no call of the step's yet. The script is not answered: the
        // sandbox ends the attempt here, and the step parks the run on
        // the call. A step that may not park is refused in the answer's
        // place.
        if answer.is_waiting() {
            if let Some(refusal) = self.may_not_park(op) {
                let refused =
                    Answer::refused(refusal).naming(answer.note().target().map(str::to_string));
                self.claim.record(&place, op, &args, &refused);
                return HostReply::Answer(self.told(seq, op, refused, false, ms));
            }
            self.tick.emit(
                self.attempt,
                Event::call(
                    op,
                    false,
                    Some(clax_bot_sandbox::CODE_WAITING.into()),
                    ms,
                    answer.note(),
                ),
            );
            let surface = route(&self.tick.surfaces.info.bindings, op)
                .map(|binding| binding.surface.clone())
                .unwrap_or_default();
            self.lock().parked = Some(Waited {
                seq,
                op: op.to_string(),
                key: place.key.clone(),
                surface,
                deadline: answer.deadline(),
            });
            return HostReply::Waiting;
        }
        HostReply::Answer(self.told(seq, op, answer, replayed, ms))
    }
}

/// A script's `tool_result` text and whether it is an error. The text is
/// JSON: `value`, `logs`, `calls` and `error`, the fields the step's
/// sessions add (`told`), and `replay` when the script run again took
/// another path. Logs are trimmed to fit `max` bytes
/// ([`crate::policy::RunLimits::tool_result_bytes`]), then the text cut
/// to them: both after the sessions' fields are in it.
fn tool_result_content(
    o: &ScriptOutcome,
    calls: &Calls,
    told: &Fields,
    max: usize,
) -> (String, bool) {
    let made: Vec<Value> = o
        .calls
        .iter()
        .map(|c| json!({"op": c.op, "ok": c.ok, "code": c.code}))
        .collect();
    let mut v = Value::Object(told.clone());
    v["value"] = json!(o.value);
    v["calls"] = json!(made);
    if let Some(e) = &o.error {
        v["error"] = json!({"kind": e.kind, "message": e.message});
    }
    if let Some((at, why)) = &calls.diverged {
        let done: Vec<&str> = calls
            .log
            .iter()
            .filter(|c| c.seq < *at)
            .map(|c| c.op.as_str())
            .collect();
        v["replay"] = json!({"diverged_at": at, "why": why, "completed": done});
    }
    let mut logs = o.logs.clone();
    loop {
        v["logs"] = json!(logs);
        let text = v.to_string();
        if text.len() <= max {
            return (text, o.error.is_some());
        }
        if logs.is_empty() {
            let mut cut = max;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            return (format!("{}… [truncated]", &text[..cut]), o.error.is_some());
        }
        logs.truncate(logs.len() / 2);
        logs.push(clax_bot_sandbox::LOGS_TRUNCATED.to_string());
        if logs.len() == 1 {
            logs.clear();
        }
    }
}

/// The seed of step `n`'s scripts' random numbers.
fn seed_of(run: &str, n: i64) -> u32 {
    let h = clax_core::audit::sha256_hex(format!("{run}:{n}").as_bytes());
    u32::from_str_radix(&h[..8], 16).unwrap_or(0)
}

impl Tick<'_> {
    /// Runs the script the last turn asked for as the run's next step,
    /// for the run `info` describes, whose bindings and their providers
    /// are `surfaces`.
    ///
    /// The sessions are opened, finished and joined in one order: the
    /// surface that declared the run's kind first, then the run's other
    /// bindings in the order they were bound.
    ///
    /// # Errors
    /// The store's; a session that could not be opened
    /// ([`SurfaceProvider::begin`]); and a provider that answered a
    /// field it may not ([`FieldClash`]), which is counted against the
    /// step as a commit that failed. Each leaves the step open, to be
    /// begun again. (In a cancelled script's step a clash is counted
    /// too, that binding's fields are left out, and the stop is taken.)
    pub(crate) async fn script(
        &self,
        info: &RunInfo,
        surfaces: &[Surface<'_>],
        tool_use_id: &str,
        title: &str,
        code: &str,
    ) -> Result<TickOutcome> {
        // The operations the script's `clax` has: the run's surfaces'
        // ([`crate::registry::RunSurfaces::operations`]). A list no
        // sandbox can build is the run's surfaces' fault and none of
        // the model's, which could write no script that fares better:
        // the run ends `failed`, before the step is begun.
        let ops = self.surfaces.operations();
        if let Err(why) = clax_bot_sandbox::validate_ops(ops) {
            return self.ops_invalid(&why).await;
        }
        let s = self.begin(StepKind::Script)?;
        self.emit(
            s.attempt,
            Event::Script {
                title: title.into(),
                code: code.into(),
            },
        );
        let journal: BTreeMap<i64, CallEntry> = self
            .store
            .bot_calls(&self.run.id, self.n)?
            .into_iter()
            .map(|c| (c.seq(), c))
            .collect();
        // The surface that declared the run's kind comes first, the
        // run's other bindings after it in the order they were bound:
        // the order the sessions are opened, finished and joined in.
        let kind = info.kind_surface();
        let mut surfaces: Vec<Surface<'_>> = surfaces.to_vec();
        surfaces.sort_by_key(|surface| surface.binding.surface != kind);
        let claim = Claim::new(self.store, self.driver, &self.run.id);
        let mut sessions: Vec<Box<dyn StepSession + '_>> = Vec::with_capacity(surfaces.len());
        for surface in &surfaces {
            let cx = StepCx::new(
                info,
                surface.binding,
                self.n,
                s.attempt,
                &self.steps,
                claim,
                &self.opts.cancel,
            );
            sessions.push(surface.provider.begin(cx).await?);
        }
        let host = StepHost {
            tick: self,
            attempt: s.attempt,
            claim,
            journal,
            bound: surfaces
                .iter()
                .zip(&sessions)
                .map(|(surface, session)| Bound {
                    binding: surface.binding,
                    provider: surface.provider,
                    session: &**session,
                })
                .collect(),
            calls: Mutex::new(Calls {
                next: 0,
                log: Vec::new(),
                diverged: None,
                parked: None,
            }),
        };
        let now_ms = chrono::DateTime::parse_from_rfc3339(&s.started_at)
            .map(|t| t.timestamp_millis())
            .unwrap_or(0);
        let script = Script::new(title, code)
            .with_ops(ops.iter().cloned())
            .with_determinism(now_ms, seed_of(&self.run.id, self.n));
        let limits = self.limits.script;
        let lost = AtomicBool::new(false);
        let mut outcome = watched(
            self.store,
            &self.run.id,
            self.driver,
            self.settings.renew_every,
            &self.opts.cancel,
            &lost,
            self.deps
                .sandbox
                .run(&script, &host, &limits, &self.opts.cancel),
        )
        .await;
        // A driver that lost its claim and a sandbox that could not take
        // the script drop the sessions: nothing of the attempt is kept.
        if lost.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(TickOutcome::wait(
                Wait::NotOurs,
                Some(self.n),
                Some(StepKind::Script),
            ));
        }
        if outcome
            .error
            .as_ref()
            .is_some_and(|e| e.kind == ErrorKind::Unavailable)
        {
            return Ok(TickOutcome::wait(
                Wait::Stalled,
                Some(self.n),
                Some(StepKind::Script),
            ));
        }
        // A sandbox that refuses the run's operations where the check
        // above did not ran nothing: the sessions are dropped, and the
        // run ends as it does there.
        if let Some(e) = outcome
            .error
            .as_ref()
            .filter(|e| e.kind == ErrorKind::InvalidOps)
        {
            drop(host);
            drop(sessions);
            return self.ops_invalid(&e.message).await;
        }
        // The attempt waits on a call: a cancel asked for by now wins
        // (below). Otherwise nothing of the attempt is kept or charged:
        // its sessions are dropped without `finish`, as a lost claim
        // drops them, the step stays begun, and the run parks on the
        // call and is let go of.
        if outcome
            .error
            .as_ref()
            .is_some_and(|e| e.kind == ErrorKind::Parked)
            && !self.opts.cancel.is_cancelled()
        {
            let waited = host
                .calls
                .into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .parked;
            drop(sessions);
            return self.park(waited).await;
        }
        let StepHost { calls, journal, .. } = host;
        let mut calls = calls.into_inner().unwrap_or_else(|e| e.into_inner());
        // A script that waited for a call and, run again with its
        // answer, ends before it reaches the call took another path:
        // the answer the run waited for would go unused in a step that
        // commits as if nothing were wrong.
        // Only a script that ended of its own accord: one that hit a
        // limit or failed keeps its own error. And only a run with a
        // surface beside Clax's own, none of whose operations waits.
        if calls.diverged.is_none()
            && outcome.error.is_none()
            && info.bindings.iter().any(|b| b.surface != CLAX_SURFACE)
            && let Some(seq) = self.unreached_wait(&calls, &journal)?
        {
            let why = format!(
                "the script ended before call {seq}, which it waited for when it ran before: \
                 the script took another path when run again"
            );
            calls.diverged = Some((seq, why));
        }
        if let Some((_, why)) = &calls.diverged {
            outcome.value = None;
            outcome.error = Some(ScriptError::new(
                ErrorKind::HostError,
                format!("HostError {CODE_REPLAY_DIVERGED}: {why}"),
            ));
        }
        let ks = self.key_source();
        // Memory at the configured limit, or what the script measurably
        // held beyond it (a Worker isolate may hold up to its own limit).
        let peak_mib = u32::try_from(outcome.mem_peak_bytes.div_ceil(1 << 20)).unwrap_or(u32::MAX);
        let compute = Metered::new(
            LINE_SCRIPT_COMPUTE,
            compute_qty(
                outcome.cpu_us,
                self.settings.script_memory_mib.max(peak_mib),
            ),
            ks,
        );
        let at = self.store.now();
        // The script's record lists the calls the sandbox counted; the
        // call records, the step's `calls` and the requests line are of
        // the calls that reached the run loop.
        let call_ops: Vec<String> = outcome.calls.iter().map(|c| c.op.clone()).collect();
        let mut events: Vec<AuditRecord> = vec![script_record(
            at.clone(),
            title,
            code,
            &call_ops,
            i64::try_from(outcome.cpu_us.div_ceil(1000)).unwrap_or(i64::MAX),
            i64::try_from(outcome.mem_peak_bytes).unwrap_or(i64::MAX),
            outcome.error.as_ref().map(|e| e.message.as_str()),
        )];
        let mut reported = BTreeSet::new();
        for c in &calls.log {
            reported.insert(c.seq);
            let mut r = call_record(at.clone(), &c.op, c.note.target(), c.ok);
            if c.replayed {
                r = r.with("replayed", true);
            }
            events.push(r);
        }
        let calls_json: Vec<Value> = calls
            .log
            .iter()
            .map(|c| {
                let mut v = json!({"op": c.op, "target": c.note.target(), "ok": c.ok,
                                   "code": c.code, "ms": c.ms});
                c.note.add_to(&mut v);
                v
            })
            .collect();
        // Cancelled when the script's error says so, or when the cancel
        // was asked for by the time it returned.
        let cancelled = outcome
            .error
            .as_ref()
            .is_some_and(|e| e.kind == ErrorKind::Cancelled)
            || self.opts.cancel.is_cancelled();
        let end = if cancelled {
            StepEnd::Cancelled
        } else {
            StepEnd::Commit
        };
        let mut join = StepJoin::new();
        let mut clashed = false;
        for (surface, session) in surfaces.iter().zip(sessions) {
            if let Err(clash) = join.add(&surface.binding.surface, session.finish(end)) {
                // A cancelled script's stop is taken all the same, with
                // none of that binding's fields: the run was told to
                // stop, and what the script used is charged. The clash
                // is counted with the stop ([`Tick::end`]).
                if !cancelled {
                    return Err(self.clashed(&clash));
                }
                tracing::warn!(
                    "clax-bot: step {} of run {} stops without the fields of a surface: {clash}",
                    self.n,
                    self.run.id
                );
                clashed = true;
            }
        }
        let mut core = Fields::new();
        core.insert("title".into(), json!(title));
        core.insert("code".into(), json!(code));
        core.insert("calls".into(), json!(calls_json));
        if cancelled {
            // What the script used is charged; what its sessions kept
            // for the commit is not kept; what its calls wrote stands.
            // The sessions' fields go to the stop as they were joined,
            // for the bindings' `ending` fields to join.
            core.insert("outcome".into(), json!(outcome));
            let mut stop = Ending::stopped(End::stopped(CANCELLED));
            stop.core.extend(core);
            return self
                .end(Ending {
                    usage: vec![compute],
                    events,
                    reported,
                    join,
                    cancelled_script: Some(clashed),
                    ..stop
                })
                .await;
        }
        events.extend(self.unreported_calls(&reported));
        let (content, is_error) =
            tool_result_content(&outcome, &calls, join.told(), self.limits.tool_result_bytes);
        core.insert("tool_use_id".into(), json!(tool_use_id));
        core.insert("outcome".into(), json!(outcome));
        core.insert(
            "result".into(),
            Block::tool_result(tool_use_id, content, is_error).to_json(),
        );
        let payload = join.payload(core);
        let mut usage = vec![compute];
        usage.extend(step_usage(
            &self.settings,
            self.n,
            calls.log.len(),
            &payload,
            ks,
        ));
        let c = self.commit(
            s.attempt,
            StepKind::Script,
            join.commit(payload, usage, events),
            Observed::default(),
        )?;
        Ok(self.outcome(StepKind::Script, c))
    }

    /// The first place of the step's journal, as the attempt found it
    /// (`journal`), that this attempt's script
    /// did not reach and that holds the answer of a call the run waited
    /// for (a pending call of the store's that was marked as one a run
    /// may park on). A journal row the script did not reach for any
    /// other reason (a crashed attempt went further) is left as it
    /// always was: reported as a call no committed attempt made.
    fn unreached_wait(
        &self,
        calls: &Calls,
        journal: &BTreeMap<i64, CallEntry>,
    ) -> Result<Option<i64>> {
        for &seq in journal.keys() {
            if seq < calls.next || seq >= crate::host::NOTICE_SEQ {
                continue;
            }
            let waited = self
                .store
                .pending_call_at(&self.run.id, self.n, seq)?
                .is_some_and(|call| call.waits);
            if waited {
                return Ok(Some(seq));
            }
        }
        Ok(None)
    }

    /// Parks the run on `waited`, the call its script's attempt waits
    /// on, until the call's answer is journaled or its deadline comes
    /// (`Store::park_run`): the claim is let go of, and the tick waits
    /// ([`Wait::Parked`]). An answer that is in the journal already
    /// (it came before the park) leaves the run running, to execute the
    /// step again at once; so does a run that was asked to stop
    /// meanwhile, whose next tick takes its `stopped` step.
    ///
    /// A sandbox that says a script parked when no call of its waits
    /// broke its contract: the run ends `failed`.
    async fn park(&self, waited: Option<Waited>) -> Result<TickOutcome> {
        let Some(waited) = waited else {
            tracing::warn!(
                "clax-bot: the sandbox parked a script of run {} that waits on no call",
                self.run.id
            );
            let why = "the sandbox parked a script whose calls were all answered";
            return self.end(Ending::failed("internal_error", Some(why))).await;
        };
        let now = self.deps.clock.now();
        let waiting_on = json!({
            "surface": waited.surface,
            "op": waited.op,
            "key": waited.key,
            "since": clax_core::time::stamp(now),
            "deadline": waited.deadline.map(clax_core::time::stamp),
        });
        let place = JournalPlace {
            step_n: self.n,
            seq: waited.seq,
        };
        let again = TickOutcome {
            next: Next::Runnable,
            step: Some(self.n),
            kind: Some(StepKind::Script),
            credits_milli: 0,
        };
        match self.store.park_run(
            &self.run.id,
            self.driver,
            place,
            &waiting_on,
            waited.deadline,
            now,
        ) {
            Ok(Park::Parked(_)) => Ok(TickOutcome::wait(
                Wait::Parked,
                Some(self.n),
                Some(StepKind::Script),
            )),
            Ok(Park::Answered(_)) => Ok(again),
            Err(CoreError::Invalid {
                code: "run_invalid",
                ..
            }) => Ok(again),
            Err(e) => Err(e),
        }
    }

    /// Ends the run `failed` ([`CODE_OPS_INVALID`]): no sandbox can
    /// build a `clax` of the operations its surfaces have together, for
    /// the reason `why` gives.
    async fn ops_invalid(&self, why: &str) -> Result<TickOutcome> {
        tracing::warn!(
            "clax-bot: run {} ends: the operations of its surfaces are not usable: {why}",
            self.run.id
        );
        self.end(Ending::failed(CODE_OPS_INVALID, Some(why))).await
    }

    /// A provider answered a field it may not: logged, and counted
    /// against the step as a commit that failed. The error is what the
    /// tick fails with when the attempt is abandoned, so a provider
    /// that clashes on every attempt ends the run `failed` and cannot
    /// hold it open.
    fn clashed(&self, clash: &FieldClash) -> CoreError {
        tracing::warn!(
            "clax-bot: step {} of run {} is abandoned: {clash}",
            self.n,
            self.run.id
        );
        let _ = self.store.note_commit_failure(&self.run.id, self.n);
        CoreError::invalid("surface_field_clash", clash.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_bot_sandbox::CallRecord;

    /// The run loop's script step reads nothing of Clax's own surface:
    /// no module of its provider, its draft, its page checks or the
    /// web, and neither the host nor the web source of the tick's deps.
    /// This holds only which modules the file reaches. That the step
    /// performs no operation of any surface itself, and needs nothing
    /// of Clax's to run one, is held by a run whose one surface is the
    /// fake's: `tests/generic.rs`, the test of the meaning.
    #[test]
    fn the_script_step_reads_no_module_of_claxs_surface() {
        let source = include_str!("script.rs");
        let code = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("the code before the tests");
        for import in ["crate::clax", "crate::draft", "crate::check", "crate::web"] {
            assert!(!code.contains(import), "script.rs reads `{import}`");
        }
        assert!(
            !code.contains("deps.host") && !code.contains("deps.web"),
            "script.rs reaches a host or a source itself"
        );
    }

    /// A script's outcome with `lines` log lines, and what a session
    /// adds to what the model is told.
    fn logged(lines: usize) -> (ScriptOutcome, Calls, Fields) {
        let outcome = ScriptOutcome {
            value: Some(json!("done")),
            logs: (0..lines).map(|i| format!("line {i}")).collect(),
            calls: vec![CallRecord {
                op: "draft.publish".into(),
                ok: true,
                code: None,
                args_bytes: 2,
                us: 0,
            }],
            ..ScriptOutcome::default()
        };
        let calls = Calls {
            next: 1,
            log: Vec::new(),
            diverged: None,
            parked: None,
        };
        let told = Fields::from_iter([("published".to_string(), json!({"version": 2}))]);
        (outcome, calls, told)
    }

    /// What the model is told is measured with the sessions' fields in
    /// it: a result that fits its bound only without them has its logs
    /// cut, and one that fits with them is left whole.
    #[test]
    fn a_result_is_cut_to_its_bound_with_the_sessions_fields_in_it() {
        let (outcome, calls, told) = logged(40);
        let (whole, is_error) = tool_result_content(&outcome, &calls, &told, usize::MAX);
        assert!(!is_error);
        assert!(whole.contains(r#""published":{"version":2}"#), "{whole}");
        assert!(whole.contains(r#""line 39""#));
        // At its own length it is left whole.
        assert_eq!(
            tool_result_content(&outcome, &calls, &told, whole.len()).0,
            whole
        );
        // One byte under: the logs are cut, and the result fits.
        let (cut, _) = tool_result_content(&outcome, &calls, &told, whole.len() - 1);
        assert!(cut.len() < whole.len(), "{} bytes", cut.len());
        assert!(cut.contains(clax_bot_sandbox::LOGS_TRUNCATED));
        assert!(cut.contains(r#""published":{"version":2}"#), "{cut}");
        // A bound the result meets only without the session's field
        // (it is that many bytes under the whole) still cuts the logs.
        let field = r#""published":{"version":2},"#.len();
        for under in 1..=field {
            let max = whole.len() - under;
            let (text, _) = tool_result_content(&outcome, &calls, &told, max);
            assert!(
                text.len() <= max,
                "{} bytes for a bound of {max}",
                text.len()
            );
        }
    }
}
