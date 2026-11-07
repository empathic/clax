//! A script step: the turn's `run_script` code in the sandbox, every call
//! it makes checked ([`api::check_args`]), limited to the run's threads
//! and artifact, journaled, and performed over the run's draft or through
//! the [`BotHost`].
//!
//! A script run again (its step resumed) is answered from the step's
//! journal call by call: draft operations are performed again (they are
//! the run's own, and deterministic over the same draft), host reads and
//! writes and publishes answer with what was recorded. A call that is not
//! the one recorded at its place ends the script with `replay_diverged`;
//! the step commits with what completed. The script's clock and random
//! numbers are fixed per step ([`clax_bot_sandbox::Determinism`]), so a
//! script that only reads what it read before makes the same calls.
//!
//! `clax.web.*` calls are performed by the run itself, through the run's
//! web source, and journaled with their charge (see [`web`]).
//!
//! A workspace call's effect is outside the store, so no transaction holds
//! it and its entry. Its intent is journaled first, at [`INTENT_SEQ`] past
//! its place, and its answer at its place after. A step executed again that
//! finds an intent and no answer does not repeat the call blindly:
//! `shell.exec` joins the command if the workspace still knows it
//! ([`workspace::Workspace::resume_exec`]) and otherwise answers
//! [`workspace::CODE_SHELL_INTERRUPTED`], for the model to look at what the
//! command left, and `change.propose` goes to the workspace again under the
//! same [`workspace::Proposal::key`], which pushes nothing twice, and to
//! [`crate::host::BotHost::record_fix`], which proposes once. Either answer
//! is journaled like any other.
//!
//! On a [`workspace::Workspace::resumable`] workspace those calls last no
//! longer than the tick. When its time runs out the call is dropped and left
//! to go on, unjournaled, and answers [`workspace::CODE_YIELDED`]; so does
//! every later call of the script, and the step ends [`Wait::Budget`] with
//! nothing committed or charged. The next tick runs the script again and
//! joins the call. Such a call is begun only with
//! [`workspace::MIN_CALL_SLACK`] of the tick left, or
//! [`LEAST_BEGUN_IN`] of the tick's budget where that is less, so a tick with
//! a small budget still begins one.
//!
//! Replay answers what the journal answered, whatever the run's rules say by
//! then. A step that ends with a `change.propose` begun and unanswered, by a
//! stop, a failure or a script that did not wait for it, tells the queue's
//! members ([`workspace::PROPOSAL_STRANDED`]), so a change never leaves
//! unsaid.

mod web;

use crate::api;
use crate::check;
use crate::draft::Draft;
use crate::events::Event;
use crate::host::{INTENT_SEQ, PublishDraft, Published, args_hash, run_plan};
use crate::run::{Observed, Stop, Tick, TickOutcome, Wait, compute_qty, step_usage, watched};
use crate::workspace;
use clax_bot_sandbox::{ErrorKind, Host, HostError, Script, ScriptError, ScriptOutcome};
use clax_core::Result;
use clax_core::audit::AuditRecord;
use clax_sample::messages::Block;
use clax_store::bot_runs::{
    CallEntry, RequestKind, StepCommit, StepKind, call_record, script_record,
};
use clax_store::credits::{LINE_SCRIPT_COMPUTE, Metered};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

/// Where a call's notice that its fix went unrecorded is journaled: this,
/// plus a hundred for each place of the call, plus the thread's. Past the
/// run's own notices, short of [`INTENT_SEQ`].
const FIX_NOTICE_SEQ: i64 = 1_500_000;

/// The most bytes of a script's `tool_result`.
pub const MAX_TOOL_RESULT_BYTES: usize = 100 << 10;

struct CallLog {
    seq: i64,
    op: String,
    target: Option<String>,
    ok: bool,
    code: Option<String>,
    replayed: bool,
    /// How long the call took, in milliseconds.
    ms: u64,
    /// What a web call charged, from its journal entry when it was
    /// answered from the journal; `None` for any other call.
    cost_milli: Option<i64>,
    /// What the run panel shows of a web call: `{query}`, or `{url,
    /// source}` where `source` names where the URL came from
    /// ([`crate::web::UrlSource::label`]).
    web: Option<Value>,
}

/// What [`RunHost::perform`] made of a call.
struct Performed {
    res: std::result::Result<Value, HostError>,
    target: Option<String>,
    replayed: bool,
    cost_milli: Option<i64>,
    web: Option<Value>,
}

impl Performed {
    fn plain(
        res: std::result::Result<Value, HostError>,
        target: Option<String>,
        replayed: bool,
    ) -> Performed {
        Performed {
            res,
            target,
            replayed,
            cost_milli: None,
            web: None,
        }
    }
}

struct HostState {
    draft: Draft,
    seq: i64,
    calls: Vec<CallLog>,
    published: Option<Published>,
    replied: Vec<String>,
    /// A reply went to the queue's people alone, for the model to be told.
    reply_kept: bool,
    created: bool,
    /// The base the draft moved to (a publish or a rebase).
    moved: bool,
    /// Where the replay left the journal, and why.
    diverged: Option<(i64, String)>,
    /// The calls the tick gave up on when its time ran out, which go on: see
    /// the module documentation.
    gave_up: BTreeSet<i64>,
}

/// The [`Host`] a script's `clax` calls reach.
struct RunHost<'a> {
    tick: &'a Tick<'a>,
    attempt: i64,
    journal: BTreeMap<i64, CallEntry>,
    state: Mutex<HostState>,
}

/// A failure as the journal keeps it: `code`, `message`, and `retryable`
/// when it is.
fn err_json(e: &HostError) -> Value {
    let mut v = json!({"code": e.code, "message": e.message});
    if e.retryable {
        v["retryable"] = json!(true);
    }
    v
}

/// What a call answers once the tick's time has run out.
/// A tick gives a call that much of its budget at the least, however little
/// that is: a tick that always had too little to begin would come back for
/// ever. A fraction, so that the first call of a tick is always begun.
const LEAST_BEGUN_IN: u32 = 4;

fn yield_error() -> HostError {
    HostError::new(
        workspace::CODE_YIELDED,
        "this step goes on in the next tick: nothing more happens in this one",
    )
}

/// The answer journal entry `e` holds, a failure as [`err_json`] kept it.
fn recorded(e: &CallEntry) -> std::result::Result<Value, HostError> {
    if e.ok() {
        Ok(e.result().clone())
    } else {
        let r = e.result();
        let err = HostError::new(
            r["code"].as_str().unwrap_or("call_failed"),
            r["message"].as_str().unwrap_or(""),
        );
        Err(if r["retryable"] == json!(true) {
            err.retryable()
        } else {
            err
        })
    }
}

impl RunHost<'_> {
    fn lock(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn log(&self, seq: i64, op: &str, p: &Performed, ms: u64) {
        let (ok, code) = match &p.res {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e.code.clone())),
        };
        self.tick.emit(
            self.attempt,
            Event::Call {
                op: op.into(),
                target: p.target.clone(),
                ok,
                code: code.clone(),
                ms,
                cost_milli: p.cost_milli,
                web: p.web.clone(),
            },
        );
        self.lock().calls.push(CallLog {
            seq,
            op: op.into(),
            target: p.target.clone(),
            ok,
            code,
            replayed: p.replayed,
            ms,
            cost_milli: p.cost_milli,
            web: p.web.clone(),
        });
    }

    /// Journals a call the host did not journal itself (reads, draft
    /// operations, refusals); failures to journal are logged.
    fn journal(
        &self,
        seq: i64,
        op: &str,
        raw: &Value,
        res: &std::result::Result<Value, HostError>,
        keep_value: bool,
    ) {
        let Ok(entry) = self.tick.entry(self.attempt, seq, op, raw) else {
            return;
        };
        let entry = entry.answered(
            res.is_ok(),
            match res {
                Ok(v) if keep_value => v.clone(),
                Ok(_) => Value::Null,
                Err(e) => err_json(e),
            },
        );
        if let Err(e) = self.tick.store.record_bot_call(&entry) {
            tracing::warn!(
                "clax-bot: journaling call {seq} of run {}: {e}",
                self.tick.run.id
            );
        }
    }

    /// What the run's rules refuse before the host sees the call.
    fn policy(&self, op: &str, args: &[Value]) -> std::result::Result<(), HostError> {
        let run = &self.tick.run;
        if workspace::refused_when_confidential(op) && self.tick.holds_secrets() {
            return Err(HostError::new(
                workspace::CODE_CONFIDENTIAL,
                format!(
                    "this run holds confidential material: clax.{op} would carry it out of the \
                     queue"
                ),
            ));
        }
        if matches!(op, "threads.read" | "threads.reply" | "threads.resolve") {
            let id = args[0].as_str().unwrap_or("");
            if !run.thread_ids.iter().any(|t| t == id) {
                return Err(HostError::new(
                    "forbidden",
                    format!("thread {id} is not part of this request"),
                ));
            }
        }
        if op == "artifacts.create" {
            if run.request != RequestKind::Brief {
                return Err(HostError::new(
                    "forbidden",
                    "only a new artifact's brief may create artifacts",
                ));
            }
            if self.tick.hist.created || self.lock().created {
                return Err(HostError::new(
                    "forbidden",
                    "this run already created an artifact",
                ));
            }
        }
        Ok(())
    }

    /// Journals that call `seq` is about to take effect; an effect is never
    /// begun without it.
    fn intend(&self, seq: i64, op: &str, raw: &Value) -> std::result::Result<(), HostError> {
        let intent = self.tick.entry(self.attempt, INTENT_SEQ + seq, op, raw)?;
        self.tick
            .store
            .record_bot_call(&intent)
            .map(drop)
            .map_err(|e| crate::host::host_error(&e))
    }

    /// Gives call `seq` up to the next tick. A failure that merely says
    /// [`workspace::CODE_YIELDED`] is an answer: only this makes a yield.
    fn give_up(&self, seq: i64) -> HostError {
        self.lock().gave_up.insert(seq);
        yield_error()
    }

    /// `work` of call `seq`, on a resumable workspace for as long as the tick
    /// has left.
    async fn until_yield<T>(
        &self,
        seq: i64,
        resumable: bool,
        work: impl Future<Output = std::result::Result<T, HostError>>,
    ) -> std::result::Result<T, HostError> {
        if !resumable {
            return work.await;
        }
        match clax_rt::time::timeout(self.tick.available(), work).await {
            Ok(done) => done,
            Err(_) => Err(self.give_up(seq)),
        }
    }

    /// `clax.shell.exec` or `clax.change.propose`, through the run's
    /// workspace.
    async fn workspace_op(
        &self,
        op: &str,
        args: &[Value],
        raw: &Value,
        seq: i64,
    ) -> std::result::Result<Value, HostError> {
        let run = &self.tick.run;
        // A call begun goes on to its end, whatever became of the offer: a
        // change may be out in the world with only this to record it.
        let begun = self.journal.contains_key(&(INTENT_SEQ + seq));
        let lapsed = self.tick.deps.offers_lapsed(run);
        let offered = self.offered();
        let allowed = begun
            || if op == workspace::OP_EXEC {
                offered.shell
            } else {
                offered.change
            };
        let ws = self
            .tick
            .deps
            .workspace
            .as_ref()
            .filter(|_| allowed)
            .ok_or_else(|| {
                HostError::new(
                    workspace::CODE_SHELL_DISABLED,
                    format!("this run does not have clax.{op}"),
                )
            })?;
        let left = self.tick.remaining();
        if left.is_zero() && !begun {
            return Err(HostError::new(
                workspace::CODE_RUN_CAP,
                "this run is out of time: finish now",
            ));
        }
        let resumable = ws.resumable();
        if begun && op == workspace::OP_EXEC && !resumable {
            return Err(workspace::shell_interrupted());
        }
        // With none of the tick left nothing is begun or waited for, and with
        // too little nothing is begun, unless the run's own time is what is
        // short: the next tick would have no more.
        let in_tick = self.tick.available();
        let slack = self
            .tick
            .opts
            .budget
            .map_or(workspace::MIN_CALL_SLACK, |b| {
                workspace::MIN_CALL_SLACK.min(b / LEAST_BEGUN_IN)
            });
        let short = in_tick < slack && in_tick < left;
        if resumable && (in_tick.is_zero() || (short && !begun)) {
            return Err(self.give_up(seq));
        }
        // Before the first effect: what comes after it holds what it reads.
        if offered.confidential {
            self.tick
                .mark_confidential()
                .map_err(|e| crate::host::host_error(&e))?;
        }
        let cx = self.tick.op_ctx(self.attempt, seq);
        let host = &self.tick.deps.host;
        let queue = self
            .tick
            .deps
            .run_queue(run)
            .map_err(|e| crate::host::host_error(&e))?;
        // What the queue's members were told does not go out in a public
        // pull request. A call begun may have left already, and goes on.
        if op == workspace::OP_PROPOSE
            && !begun
            && offered.confidential
            && self.tick.deps.target_may_be_public(run)
        {
            if let Err(n) = host
                .note_members(run, &self.tick.ctx, workspace::PROPOSAL_WITHHELD)
                .await
            {
                tracing::warn!(
                    "clax-bot: telling of a withheld proposal of {}: {n}",
                    run.id
                );
            }
            return Err(HostError::new(
                workspace::CODE_CONFIDENTIAL,
                workspace::PROPOSAL_PUBLIC,
            ));
        }
        // Nothing leaves that the report could not take. A call begun may
        // have left already, and no fix will say so.
        if op == workspace::OP_PROPOSE
            && queue.is_some()
            && let Err(e) = host.check_fix(&cx).await
        {
            if begun
                && let Err(n) = host
                    .note_members(run, &self.tick.ctx, workspace::PROPOSAL_STRANDED)
                    .await
            {
                tracing::warn!(
                    "clax-bot: telling of a stranded proposal of {}: {n}",
                    run.id
                );
            }
            return Err(e);
        }
        if !begun {
            self.intend(seq, op, raw)?;
        }
        if op == workspace::OP_EXEC {
            let opts = &args[1];
            let timeout = opts["timeout"]
                .as_u64()
                .unwrap_or(workspace::DEFAULT_TIMEOUT_SECS)
                .clamp(1, workspace::MAX_TIMEOUT_SECS)
                .min(left.as_secs().max(1));
            let call = async {
                if begun {
                    let joined = ws.resume_exec(run, &cx.key).await?;
                    return joined.ok_or_else(workspace::shell_interrupted);
                }
                ws.exec_keyed(
                    run,
                    queue.as_deref(),
                    &cx.key,
                    args[0].as_str().unwrap_or(""),
                    opts["cwd"].as_str(),
                    timeout,
                )
                .await
            };
            let out = self.until_yield(seq, resumable, call).await;
            // The first command opened the target as it stands by then, which
            // may not be what the offer above said. What comes next holds what
            // this read.
            if self.tick.deps.workspace_offer(run).confidential {
                self.tick
                    .mark_confidential()
                    .map_err(|e| crate::host::host_error(&e))?;
            }
            let mut out = out?;
            if let Some(text) = out["output"].as_str() {
                out["output"] = Value::String(workspace::clip_output(text));
            }
            return Ok(out);
        }
        let change = &args[0];
        let proposal = workspace::Proposal {
            title: change["title"].as_str().unwrap_or("").to_string(),
            summary: change["summary"].as_str().unwrap_or("").to_string(),
            key: cx.key.clone(),
        };
        let coming = self.tick.deps.fix_coming(&run.id);
        let done = self
            .until_yield(seq, resumable, ws.propose(run, &proposal))
            .await;
        // An offer read in a lapse says nothing of this run, and what the
        // workspace answered brought back what does.
        let (cx, offered) = if lapsed {
            let offered = self.offered();
            if offered.confidential {
                self.tick
                    .mark_confidential()
                    .map_err(|e| crate::host::host_error(&e))?;
            }
            (self.tick.op_ctx(self.attempt, seq), offered)
        } else {
            (cx, offered)
        };
        let done = self
            .fix_of(&cx, queue.is_some(), offered, &proposal, done)
            .await;
        coming.settled().await;
        done
    }

    /// What the run gets of its workspace now, in the light of what it holds.
    fn offered(&self) -> workspace::Offer {
        let mut offered = self.tick.deps.workspace_offer(&self.tick.run);
        offered.confidential |= self.tick.holds_secrets();
        offered
    }

    /// Records the fix of `proposal`, which `done` answered, on the run's
    /// report if it has one.
    async fn fix_of(
        &self,
        cx: &crate::host::OpCtx<'_>,
        on_report: bool,
        offered: workspace::Offer,
        proposal: &workspace::Proposal,
        done: std::result::Result<Value, HostError>,
    ) -> std::result::Result<Value, HostError> {
        let mut done = done?;
        let host = &self.tick.deps.host;
        let seq = cx.seq;
        // The report's reporter hears of it through the fix. What was
        // pushed stands whatever comes of that: the answer and a notice say
        // where it is.
        if on_report && let Err(e) = host.record_fix(cx, &offered.fix(proposal, &done)).await {
            let url = done["url"]
                .as_str()
                .unwrap_or("the pull request")
                .to_string();
            done["fix_recorded"] = json!(false);
            done["fix_error"] = json!(e.code);
            let notice = format!(
                "I proposed {url}, but could not record the fix on this report ({}).",
                e.code
            );
            self.tick
                .notify_at(self.attempt, FIX_NOTICE_SEQ + seq * 100, &notice)
                .await;
        }
        Ok(done)
    }

    async fn draft_op(
        &self,
        op: &str,
        args: &[Value],
        replay: Option<&CallEntry>,
    ) -> std::result::Result<Value, HostError> {
        let s = |i: usize| args[i].as_str().unwrap_or("");
        match op {
            "draft.check" => {
                let report = check::check(&self.lock().draft);
                Ok(serde_json::to_value(report).expect("serialisable"))
            }
            "draft.rebase" => {
                // A rebase made again goes to the version it went to.
                let version = match replay.and_then(|e| e.result()["version"].as_u64()) {
                    Some(v) => u32::try_from(v).unwrap_or(0),
                    None => {
                        let aid = clax_core::ArtifactId::parse(&self.tick.run.artifact_id)
                            .map_err(|e| crate::host::host_error(&e))?;
                        self.tick
                            .store
                            .get_artifact(&aid)
                            .map_err(|e| crate::host::host_error(&e))?
                            .ok_or_else(|| HostError::new("not_found", "the artifact is gone"))?
                            .current_version
                    }
                };
                let mut draft = self.lock().draft.clone();
                let r = draft
                    .rebase(self.tick.store, &self.tick.run, version)
                    .await
                    .map_err(|e| crate::host::host_error(&e))?;
                let mut st = self.lock();
                st.draft = draft;
                st.moved = true;
                Ok(serde_json::to_value(r).expect("serialisable"))
            }
            _ => {
                let mut st = self.lock();
                match op {
                    "files.list" => Ok(st.draft.list()),
                    "files.read" => st.draft.read(s(0)).map(Value::String),
                    "files.write" => st.draft.write(s(0), s(1)),
                    "files.replace" => st.draft.replace(s(0), s(1), s(2)),
                    "files.delete" => st.draft.delete(s(0)),
                    "draft.theirs" => st.draft.theirs(s(0)).map(|t| json!(t)),
                    other => Err(HostError::new(
                        "unknown_op",
                        format!("{other} is not a draft operation"),
                    )),
                }
            }
        }
    }

    async fn publish(
        &self,
        args: &[Value],
        raw: &Value,
        seq: i64,
        replay: Option<&CallEntry>,
    ) -> std::result::Result<Value, HostError> {
        if let Some(e) = replay {
            let v = recorded(e)?;
            let p: Published = serde_json::from_value(v.clone())
                .map_err(|e| HostError::new("internal", e.to_string()))?;
            let mut st = self.lock();
            st.draft.base_version = p.version;
            st.published = Some(p);
            st.moved = true;
            return Ok(v);
        }
        let (files, base) = {
            let st = self.lock();
            if st.published.is_some() {
                return Err(HostError::new(
                    "already_published",
                    "this script already published the draft; publish again from a later script",
                ));
            }
            (st.draft.publish_files(), st.draft.base_version)
        };
        let opts = &args[0];
        let addresses = match opts["addresses"].as_array() {
            Some(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            None => self.tick.run.thread_ids.clone(),
        };
        let cx = self.tick.op_ctx(self.attempt, seq);
        let plan = self
            .tick
            .deps
            .host
            .plan_publish(
                &cx,
                PublishDraft {
                    files,
                    if_version: base,
                    note: opts["note"].as_str().map(str::to_string),
                    addresses,
                },
            )
            .await?;
        let entry = self.tick.entry(self.attempt, seq, "draft.publish", raw)?;
        let got = run_plan(plan, entry).await?;
        let p: Published = serde_json::from_value(got.clone())
            .map_err(|e| HostError::new("internal", e.to_string()))?;
        let mut st = self.lock();
        st.draft.base_version = p.version;
        st.published = Some(p);
        st.moved = true;
        Ok(got)
    }

    async fn perform(&self, op: &str, raw: &Value, seq: i64) -> Performed {
        let hash = args_hash(raw);
        let replay = self.journal.get(&seq);
        if let Some(e) = replay.or_else(|| self.journal.get(&(INTENT_SEQ + seq)))
            && (e.op() != op || e.args_hash() != hash)
        {
            let why = format!(
                "call {seq} was {} before and is {op} now: the script took another path when run again",
                e.op()
            );
            self.lock().diverged = Some((seq, why.clone()));
            return Performed::plain(
                Err(HostError::new(api::CODE_REPLAY_DIVERGED, why)),
                None,
                false,
            );
        }
        let args = match api::check_args(op, raw) {
            Ok(a) => a,
            Err(e) => {
                let res = Err(e);
                if replay.is_none() {
                    self.journal(seq, op, raw, &res, false);
                }
                return Performed::plain(res, None, replay.is_some());
            }
        };
        let target = api::target(op, &args, &self.tick.run.artifact_id);
        // What the journal answered was allowed or refused when it was made:
        // the rules as they stand now do not rewrite it.
        if replay.is_none()
            && let Err(e) = self.policy(op, &args)
        {
            let res = Err(e);
            self.journal(seq, op, raw, &res, false);
            return Performed::plain(res, target, false);
        }
        let spec = api::op(op).expect("checked");
        let res = match spec.performer {
            api::Performer::Web => {
                let done = self.web(op, &args, raw, seq, replay).await;
                return Performed {
                    res: done.res,
                    target,
                    replayed: replay.is_some(),
                    cost_milli: done.cost_milli,
                    web: Some(done.web),
                };
            }
            api::Performer::Workspace => match replay {
                Some(e) => recorded(e),
                None => {
                    let res = self.workspace_op(op, &args, raw, seq).await;
                    if !self.lock().gave_up.contains(&seq) {
                        self.journal(seq, op, raw, &res, true);
                    }
                    res
                }
            },
            api::Performer::Draft => {
                let res = self.draft_op(op, &args, replay).await;
                if replay.is_none() {
                    self.journal(seq, op, raw, &res, op == "draft.rebase");
                }
                res
            }
            api::Performer::Publish => {
                let res = self.publish(&args, raw, seq, replay).await;
                if replay.is_none() && res.is_err() {
                    self.journal(seq, op, raw, &res, false);
                }
                res
            }
            api::Performer::Host => match replay {
                Some(e) => recorded(e),
                None => {
                    let cx = if op == "threads.reply" {
                        let tid = args[0].as_str().unwrap_or("");
                        self.tick.reply_ctx(self.attempt, seq, tid)
                    } else {
                        self.tick.op_ctx(self.attempt, seq)
                    };
                    match spec.access {
                        api::Access::Read => {
                            let res = self.tick.deps.host.read(&cx, op, &args).await;
                            self.journal(seq, op, raw, &res, true);
                            res
                        }
                        api::Access::Write => {
                            let res = match self.tick.deps.host.plan_write(&cx, op, &args).await {
                                Ok(plan) => match self.tick.entry(self.attempt, seq, op, raw) {
                                    Ok(entry) => run_plan(plan, entry).await,
                                    Err(e) => Err(e),
                                },
                                Err(e) => Err(e),
                            };
                            if res.is_err() {
                                self.journal(seq, op, raw, &res, false);
                            }
                            res
                        }
                        // Unreachable: every Spend operation is `Performer::Web`.
                        api::Access::Spend => {
                            let res = Err(HostError::new(
                                api::CODE_UNKNOWN_OP,
                                format!("clax.{op} is not a host operation"),
                            ));
                            self.journal(seq, op, raw, &res, false);
                            res
                        }
                    }
                }
            },
        };
        if let Ok(v) = &res {
            let mut st = self.lock();
            match op {
                "threads.reply" => {
                    st.replied.push(args[0].as_str().unwrap_or("").to_string());
                    st.reply_kept |= v["members_only"] == json!(true);
                }
                "artifacts.create" => st.created = true,
                _ => {}
            }
        }
        Performed::plain(res, target, replay.is_some())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Host for RunHost<'_> {
    async fn call(&self, op: &str, args: Value) -> std::result::Result<Value, HostError> {
        let (seq, diverged) = {
            let mut st = self.lock();
            if !st.gave_up.is_empty() {
                return Err(yield_error());
            }
            st.seq += 1;
            (st.seq - 1, st.diverged.clone())
        };
        if let Some((at, why)) = diverged {
            let p = Performed::plain(
                Err(HostError::new(
                    api::CODE_REPLAY_DIVERGED,
                    format!("the replay stopped at call {at}: {why}"),
                )),
                None,
                false,
            );
            self.log(seq, op, &p, 0);
            return p.res;
        }
        let started = clax_rt::time::Instant::now();
        let p = self.perform(op, &args, seq).await;
        if self.lock().gave_up.contains(&seq) {
            return p.res;
        }
        let ms = if p.replayed {
            0
        } else {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        self.log(seq, op, &p, ms);
        p.res
    }
}

/// A script's `tool_result` text (JSON: `value`, `logs`, `calls`, `error`,
/// `published`, `replay`, `reply_note`) and whether it is an error, logs
/// trimmed to fit [`MAX_TOOL_RESULT_BYTES`]. `reply_note` is there whatever
/// the script returned, so the model learns where its reply went.
fn tool_result_content(o: &ScriptOutcome, st: &HostState) -> (String, bool) {
    let calls: Vec<Value> = o
        .calls
        .iter()
        .map(|c| json!({"op": c.op, "ok": c.ok, "code": c.code}))
        .collect();
    let mut logs = o.logs.clone();
    loop {
        let mut v = json!({"value": o.value, "logs": logs, "calls": calls});
        if let Some(e) = &o.error {
            v["error"] = json!({"kind": e.kind, "message": e.message});
        }
        if let Some(p) = &st.published {
            v["published"] = json!(p);
        }
        if st.reply_kept {
            v["reply_note"] = json!(workspace::REPLY_KEPT);
        }
        if let Some((at, why)) = &st.diverged {
            let done: Vec<&str> = st
                .calls
                .iter()
                .filter(|c| c.seq < *at)
                .map(|c| c.op.as_str())
                .collect();
            v["replay"] = json!({"diverged_at": at, "why": why, "completed": done});
        }
        let text = v.to_string();
        if text.len() <= MAX_TOOL_RESULT_BYTES {
            return (text, o.error.is_some());
        }
        if logs.is_empty() {
            let mut cut = MAX_TOOL_RESULT_BYTES;
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
    pub(crate) async fn script(
        &self,
        tool_use_id: &str,
        title: &str,
        code: &str,
    ) -> Result<TickOutcome> {
        let s = self.begin(StepKind::Script)?;
        self.emit(
            s.attempt,
            Event::Script {
                title: title.into(),
                code: code.into(),
            },
        );
        let base = self.hist.base.unwrap_or(self.run.base_version);
        let draft = Draft::load(self.store, &self.run, base).await?;
        let journal: BTreeMap<i64, CallEntry> = self
            .store
            .bot_calls(&self.run.id, self.n)?
            .into_iter()
            .map(|c| (c.seq(), c))
            .collect();
        let host = RunHost {
            tick: self,
            attempt: s.attempt,
            journal,
            state: Mutex::new(HostState {
                draft,
                seq: 0,
                calls: Vec::new(),
                published: None,
                replied: Vec::new(),
                reply_kept: false,
                created: false,
                moved: false,
                diverged: None,
                gave_up: BTreeSet::new(),
            }),
        };
        let now_ms = chrono::DateTime::parse_from_rfc3339(&s.started_at)
            .map(|t| t.timestamp_millis())
            .unwrap_or(0);
        let script =
            Script::new(title, code).with_determinism(now_ms, seed_of(&self.run.id, self.n));
        let limits = self.settings.limits();
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
        if lost.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(TickOutcome::wait(
                Wait::NotOurs,
                Some(self.n),
                Some(StepKind::Script),
            ));
        }
        if !host.lock().gave_up.is_empty() && !self.opts.cancel.is_cancelled() {
            return Ok(TickOutcome::wait(
                Wait::Budget,
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
        let st = host.state.into_inner().unwrap_or_else(|e| e.into_inner());
        if let Some((_, why)) = &st.diverged {
            outcome.value = None;
            outcome.error = Some(ScriptError::new(
                ErrorKind::HostError,
                format!("HostError {}: {why}", api::CODE_REPLAY_DIVERGED),
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
        let call_ops: Vec<String> = outcome.calls.iter().map(|c| c.op.clone()).collect();
        // The audit trail outlives the run and has readers of its own: of a
        // confidential run it keeps no words of the model's.
        let open = !self.holds_secrets();
        let mut events: Vec<AuditRecord> = vec![script_record(
            at.clone(),
            if open { title } else { "" },
            code,
            &call_ops,
            i64::try_from(outcome.cpu_us.div_ceil(1000)).unwrap_or(i64::MAX),
            i64::try_from(outcome.mem_peak_bytes).unwrap_or(i64::MAX),
            outcome
                .error
                .as_ref()
                .filter(|_| open)
                .map(|e| e.message.as_str()),
        )];
        let mut reported = BTreeSet::new();
        for c in &st.calls {
            reported.insert(c.seq);
            let target = c.target.as_deref().filter(|_| open);
            let mut r = call_record(at.clone(), &c.op, target, c.ok);
            if c.replayed {
                r = r.with("replayed", true);
            }
            events.push(r);
        }
        let calls_json: Vec<Value> = st
            .calls
            .iter()
            .map(|c| {
                let mut v = json!({"op": c.op, "target": c.target, "ok": c.ok, "code": c.code,
                                   "ms": c.ms});
                crate::events::with_web_fields(&mut v, c.cost_milli, c.web.as_ref());
                v
            })
            .collect();
        let base = st.moved.then_some(st.draft.base_version);
        let cancelled = outcome
            .error
            .as_ref()
            .is_some_and(|e| e.kind == ErrorKind::Cancelled)
            || self.opts.cancel.is_cancelled();
        if cancelled {
            // What the script used is charged; its draft changes are not
            // kept; what it published and wrote stands.
            return self
                .stop(
                    Stop::cancelled(),
                    vec![compute],
                    json!({"title": title, "code": code, "outcome": outcome, "calls": calls_json,
                           "published": st.published, "replied": st.replied, "base": base}),
                    events,
                    &reported,
                    Observed::default(),
                )
                .await;
        }
        events.extend(self.unreported_calls(&reported));
        // A script may end without waiting for its proposal.
        self.tell_stranded().await;
        let (content, is_error) = tool_result_content(&outcome, &st);
        let result = Block::tool_result(tool_use_id, content, is_error).to_json();
        let payload = json!({
            "tool_use_id": tool_use_id,
            "title": title,
            "code": code,
            "outcome": outcome,
            "result": result,
            "calls": calls_json,
            "changed": st.draft.changed_paths(),
            "replied": st.replied,
            "published": st.published,
            "created": st.created,
            "base": base,
        });
        let mut usage = vec![compute];
        usage.extend(step_usage(
            &self.settings,
            self.n,
            st.calls.len(),
            &payload,
            ks,
        ));
        let c = self.commit(
            s.attempt,
            StepKind::Script,
            StepCommit {
                payload,
                usage,
                events,
                draft: st.draft.changes(),
                reason: None,
            },
            Observed::default(),
        )?;
        Ok(self.outcome(StepKind::Script, c))
    }
}
