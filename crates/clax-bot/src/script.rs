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

use crate::api;
use crate::check;
use crate::draft::Draft;
use crate::events::Event;
use crate::host::{PublishDraft, Published, args_hash, run_plan};
use crate::run::{Observed, Stop, Tick, TickOutcome, Wait, compute_qty, step_usage, watched};
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
}

struct HostState {
    draft: Draft,
    seq: i64,
    calls: Vec<CallLog>,
    published: Option<Published>,
    replied: Vec<String>,
    created: bool,
    /// The base the draft moved to (a publish or a rebase).
    moved: bool,
    /// Where the replay left the journal, and why.
    diverged: Option<(i64, String)>,
}

/// The [`Host`] a script's `clax` calls reach.
struct RunHost<'a> {
    tick: &'a Tick<'a>,
    attempt: i64,
    journal: BTreeMap<i64, CallEntry>,
    state: Mutex<HostState>,
}

fn err_json(e: &HostError) -> Value {
    json!({"code": e.code, "message": e.message})
}

fn recorded(e: &CallEntry) -> std::result::Result<Value, HostError> {
    if e.ok() {
        Ok(e.result().clone())
    } else {
        Err(HostError::new(
            e.result()["code"].as_str().unwrap_or("call_failed"),
            e.result()["message"].as_str().unwrap_or(""),
        ))
    }
}

impl RunHost<'_> {
    fn lock(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn log(
        &self,
        seq: i64,
        op: &str,
        target: Option<String>,
        res: &std::result::Result<Value, HostError>,
        replayed: bool,
        ms: u64,
    ) {
        let (ok, code) = match res {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e.code.clone())),
        };
        self.tick.emit(
            self.attempt,
            Event::Call {
                op: op.into(),
                target: target.clone(),
                ok,
                code: code.clone(),
                ms,
            },
        );
        self.lock().calls.push(CallLog {
            seq,
            op: op.into(),
            target,
            ok,
            code,
            replayed,
            ms,
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

    async fn perform(
        &self,
        op: &str,
        raw: &Value,
        seq: i64,
    ) -> (std::result::Result<Value, HostError>, Option<String>, bool) {
        let hash = args_hash(raw);
        let replay = self.journal.get(&seq);
        if let Some(e) = replay
            && (e.op() != op || e.args_hash() != hash)
        {
            let why = format!(
                "call {seq} was {} before and is {op} now: the script took another path when run again",
                e.op()
            );
            self.lock().diverged = Some((seq, why.clone()));
            return (
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
                return (res, None, replay.is_some());
            }
        };
        let target = api::target(op, &args, &self.tick.run.artifact_id);
        if let Err(e) = self.policy(op, &args) {
            let res = Err(e);
            if replay.is_none() {
                self.journal(seq, op, raw, &res, false);
            }
            return (res, target, replay.is_some());
        }
        let spec = api::op(op).expect("checked");
        let res = match spec.performer {
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
                    let cx = self.tick.op_ctx(self.attempt, seq);
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
                    }
                }
            },
        };
        if res.is_ok() {
            let mut st = self.lock();
            match op {
                "threads.reply" => st.replied.push(args[0].as_str().unwrap_or("").to_string()),
                "artifacts.create" => st.created = true,
                _ => {}
            }
        }
        (res, target, replay.is_some())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Host for RunHost<'_> {
    async fn call(&self, op: &str, args: Value) -> std::result::Result<Value, HostError> {
        let (seq, diverged) = {
            let mut st = self.lock();
            st.seq += 1;
            (st.seq - 1, st.diverged.clone())
        };
        if let Some((at, why)) = diverged {
            let res = Err(HostError::new(
                api::CODE_REPLAY_DIVERGED,
                format!("the replay stopped at call {at}: {why}"),
            ));
            self.log(seq, op, None, &res, false, 0);
            return res;
        }
        let started = clax_rt::time::Instant::now();
        let (res, target, replayed) = self.perform(op, &args, seq).await;
        let ms = if replayed {
            0
        } else {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        self.log(seq, op, target, &res, replayed, ms);
        res
    }
}

/// A script's `tool_result` text (JSON: `value`, `logs`, `calls`, `error`,
/// `published`, `replay`) and whether it is an error, logs trimmed to fit
/// [`MAX_TOOL_RESULT_BYTES`].
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
                created: false,
                moved: false,
                diverged: None,
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
        for c in &st.calls {
            reported.insert(c.seq);
            let mut r = call_record(at.clone(), &c.op, c.target.as_deref(), c.ok);
            if c.replayed {
                r = r.with("replayed", true);
            }
            events.push(r);
        }
        let calls_json: Vec<Value> = st
            .calls
            .iter()
            .map(
                |c| json!({"op": c.op, "target": c.target, "ok": c.ok, "code": c.code, "ms": c.ms}),
            )
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
