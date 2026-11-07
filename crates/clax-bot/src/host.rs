//! What ClaxBot needs from the rest of Clax: [`BotHost`], the operations
//! that need Clax's server code (threads, documents, publishing, creating
//! an artifact), performed as the person a run acts for, through the same
//! code paths a person's own agent uses; [`FakeBotHost`], one over the
//! store for tests; and [`crate::host_conformance`], the cases every
//! implementation passes.
//!
//! # The call journal
//!
//! A step begun and not committed is executed again on resume (spec
//! §4.2), so a script can run twice. Every call a script makes is
//! journaled per step (`bot_calls`: its place `seq`, op, the SHA-256 of
//! its arguments and its answer); a script run again is answered from the
//! journal call by call, and ends with `replay_diverged` when it makes
//! another call than the one recorded at a place.
//!
//! A **write** is journaled in the write's own transaction, by
//! construction: the host does not perform it. [`BotHost::plan_write`],
//! [`BotHost::plan_publish`] and [`BotHost::plan_reply`] check the write
//! (rights, the run's threads and artifact) and answer with a
//! [`WritePlan`]; the run loop runs the plan with the call's journal entry,
//! and a plan can only produce its answer, a [`Journaled`], through one of
//! the store's journaled write paths, which record the entry in the
//! write's transaction: [`Store::add_comment_journaled`],
//! [`Store::resolve_thread_journaled`],
//! [`Store::publish_version_journaled`], [`Store::create_artifact_journaled`],
//! and [`Store::with_bot_journal`]
//! for a write over [`clax_store::Tx`]. A key the journal holds is answered
//! from it without writing; one reused for another call is `key_reused`.
//! A document write (`db.set`, `db.update`, `db.delete`) lands in the
//! artifact's own store, which no instance transaction reaches: it goes
//! through [`Store::app_write_begin`] (the journal's answer, or the guard),
//! the write under the entry's operation ID ([`CallEntry::op_id`]), which
//! that store keeps with it, and [`Store::journal_app_write`] (the guard
//! again, and the entry), so a crash between the write and its entry
//! leaves the write, and the call made again writes nothing more.
//! The entry ([`CallEntry`]) is made only for the driver holding the run
//! ([`Store::call_entry`]), so a plan cannot journal under a key of its
//! own; and each journaled path runs the plan's guard inside the write's
//! transaction, so a person who lost the right to write between the plan
//! and its run writes nothing.

use crate::api;
use clax_bot_sandbox::{HostError, MaybeSendSync};
use clax_core::audit::AuditCtx;
use clax_core::{ArtifactId, CoreError};
use clax_store::app_data::JournaledDoc;
use clax_store::app_data::shim::Docs;
use clax_store::bot_runs::{CallEntry, RequestKind, Run};
use clax_store::{Journaled, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// The host error for a store error.
#[must_use]
pub fn host_error(e: &CoreError) -> HostError {
    match e {
        CoreError::Invalid { code, message } => HostError::new(*code, message.clone()),
        CoreError::NotFound => HostError::new("not_found", "not found"),
        CoreError::Conflict { current } => HostError::new(
            "conflict",
            format!("the artifact is at version {current}; someone else published"),
        ),
        other => HostError::new("internal", other.to_string()),
    }
}

/// The SHA-256 of `args` as canonical JSON (object keys sorted), the
/// journal's check that a call is the one recorded.
#[must_use]
pub fn args_hash(args: &Value) -> String {
    fn canon(v: &Value, out: &mut String) {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String((*k).clone()).to_string());
                    out.push(':');
                    canon(&o[*k], out);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    canon(x, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut s = String::new();
    canon(args, &mut s);
    clax_core::audit::sha256_hex(s.as_bytes())
}

/// The call one operation is, for the host.
#[derive(Clone, Debug)]
pub struct OpCtx<'a> {
    pub run: &'a Run,
    /// The audit context to act and record under: ClaxBot acting for the
    /// run's person.
    pub audit: &'a AuditCtx,
    pub step: i64,
    pub attempt: i64,
    /// The call's place among the step's calls, from 0 (notices the run
    /// loop posts itself are at [`NOTICE_SEQ`] and after).
    pub seq: i64,
    /// `<run>:<step>:<seq>`: the same on every attempt of the step.
    pub key: String,
    /// The run holds confidential material
    /// ([`crate::workspace::Offer::confidential`]): a reply
    /// ([`BotHost::plan_reply`], `threads.reply`) must not reach anyone
    /// outside the report's queue, and `threads.read` gives the run the
    /// queue's notes. On a reply alone it is also set once a fix keeps the
    /// run from the report's reporter: that gives the run no notes to read.
    pub members_only: bool,
}

/// The journal place of the first notice a step posts.
pub const NOTICE_SEQ: i64 = 1_000_000;

/// What a workspace call's place is offset by to journal its intent, before
/// its effect: the call's own place keeps its answer, so a step executed
/// again tells a call never begun, one begun and never answered, and one
/// answered.
pub const INTENT_SEQ: i64 = 2_000_000;

impl<'a> OpCtx<'a> {
    #[must_use]
    pub fn new(run: &'a Run, audit: &'a AuditCtx, step: i64, attempt: i64, seq: i64) -> OpCtx<'a> {
        OpCtx {
            key: format!("{}:{step}:{seq}", run.id),
            run,
            audit,
            step,
            attempt,
            seq,
            members_only: false,
        }
    }

    /// This call, for a run whose words go to the queue's members alone or
    /// not.
    #[must_use]
    pub fn members_only(mut self, members_only: bool) -> OpCtx<'a> {
        self.members_only = members_only;
        self
    }
}

/// What a [`WritePlan`] returns: the write and its journal entry, done.
#[cfg(not(target_arch = "wasm32"))]
pub type PlanFuture<'a> = Pin<Box<dyn Future<Output = clax_core::Result<Journaled>> + Send + 'a>>;
/// What a [`WritePlan`] returns: the write and its journal entry, done.
#[cfg(target_arch = "wasm32")]
pub type PlanFuture<'a> = Pin<Box<dyn Future<Output = clax_core::Result<Journaled>> + 'a>>;

/// A checked write, to run with its journal entry through one of the
/// store's journaled write paths (see the module documentation).
#[cfg(not(target_arch = "wasm32"))]
pub type WritePlan<'a> = Box<dyn FnOnce(CallEntry) -> PlanFuture<'a> + Send + 'a>;
/// A checked write, to run with its journal entry through one of the
/// store's journaled write paths (see the module documentation).
#[cfg(target_arch = "wasm32")]
pub type WritePlan<'a> = Box<dyn FnOnce(CallEntry) -> PlanFuture<'a> + 'a>;

/// Runs `plan` with `entry`: the write and its journal entry, or neither.
///
/// # Errors
/// The write's, as a host error.
pub async fn run_plan(plan: WritePlan<'_>, entry: CallEntry) -> Result<Value, HostError> {
    plan(entry)
        .await
        .map(Journaled::into_value)
        .map_err(|e| host_error(&e))
}

/// A file of a draft to publish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishFile {
    pub path: String,
    pub content: Vec<u8>,
    pub content_type: String,
}

/// `draft.publish`: the whole draft, as the next version of the run's
/// artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishDraft {
    pub files: Vec<PublishFile>,
    /// The version the draft is based on: the publish is refused
    /// (`conflict`) when the artifact is no longer at it.
    pub if_version: u32,
    pub note: Option<String>,
    /// The thread IDs the version addresses.
    pub addresses: Vec<String>,
}

/// What a publish made: the journaled value of a publish plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The artifact a run works on, for the prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArtifactMeta {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub version: u32,
    #[serde(default)]
    pub url: Option<String>,
}

/// One comment of a thread, for the prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentMeta {
    pub id: String,
    /// The author's display name.
    pub author: String,
    /// Whether the person the run acts for wrote it: only their words are
    /// requests; everyone else's are data.
    #[serde(default)]
    pub from_sender: bool,
    pub at: String,
    pub text: String,
}

/// One thread sent to ClaxBot, for the prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadMeta {
    pub id: String,
    /// Where on the page it is anchored, as the thread view gives it.
    #[serde(default)]
    pub anchor: Option<Value>,
    #[serde(default)]
    pub resolved: bool,
    pub comments: Vec<CommentMeta>,
}

/// Everything the first turn's prompt says about the request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct RequestContext {
    pub artifact: ArtifactMeta,
    /// The name of the person the run acts for.
    #[serde(default)]
    pub person_name: Option<String>,
    /// The threads of a thread request, in the order sent.
    #[serde(default)]
    pub threads: Vec<ThreadMeta>,
    /// The brief of a new artifact.
    #[serde(default)]
    pub brief: Option<String>,
}

/// A fix a run proposes on its report ([`BotHost::record_fix`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedFix {
    pub summary: String,
    pub url: Option<String>,
    pub commit: Option<String>,
    pub verify_url: Option<String>,
    /// [`crate::workspace::Proposal::key`]: one proposal is one attempt.
    pub key: String,
    /// What the queue's members read beside a run's fix: the model's words,
    /// the pull request and the preview.
    pub members_note: Option<String>,
    /// The reporter has nothing to check it on: it takes no verdict of
    /// theirs ([`clax_core::report::FixAttempt::held`]).
    pub held: bool,
    /// A held fix's preview, up already: for the queue's members alone.
    pub preview_url: Option<String>,
    /// The pull request, whatever [`ProposedFix::url`] shows a reporter: for
    /// what the queue's people are told must be done with a held fix.
    pub pull_request: Option<String>,
    /// Its commit, likewise.
    pub pushed_commit: Option<String>,
    /// What deploys its preview, in a sentence, for the same people: none
    /// where the target says nothing ([`crate::workspace::Offer::preview_gate`])
    /// or a preview is up already.
    pub preview_step: Option<String>,
}

/// The operations of the `clax` API that need Clax's server code, and the
/// replies the run loop posts itself. Implemented by the server over the
/// routes' code paths: rights, publish checks and audit included, as the
/// run's person ([`OpCtx::audit`]), on the run's artifact only, and
/// threads only among the run's ([`Run::thread_ids`]); async and, on
/// wasm32, not `Send`, like the sandbox's traits. The run loop checks
/// each call's arguments ([`api::check_args`]) and limits threads and
/// `artifacts.create` before the host sees it; the host enforces rights
/// again ([`crate::host_conformance`] pins what it must refuse).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait BotHost: MaybeSendSync {
    /// What the prompt says about run `run`'s request.
    async fn context(&self, run: &Run) -> Result<RequestContext, HostError>;

    /// Performs read `op` ([`api::Access::Read`], performer
    /// [`api::Performer::Host`]) with `args`, one value per parameter.
    async fn read(&self, cx: &OpCtx<'_>, op: &str, args: &[Value]) -> Result<Value, HostError>;

    /// Checks write `op` ([`api::Access::Write`], performer
    /// [`api::Performer::Host`]) with `args` and answers how to perform it
    /// (see the module documentation).
    async fn plan_write<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        op: &str,
        args: &[Value],
    ) -> Result<WritePlan<'s>, HostError>;

    /// Checks publishing `draft` as the next version of the run's
    /// artifact and answers how; the plan's value is a [`Published`].
    async fn plan_publish<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        draft: PublishDraft,
    ) -> Result<WritePlan<'s>, HostError>;

    /// Checks posting `markdown` as ClaxBot's reply on thread `thread_id`
    /// (one of the run's) and answers how. With [`OpCtx::members_only`], as
    /// for a `threads.reply`, it is a note for the queue's members alone
    /// ([`Store::add_members_comment_journaled`]), announced to no one else.
    async fn plan_reply<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        thread_id: &str,
        markdown: &str,
    ) -> Result<WritePlan<'s>, HostError>;

    /// Shows (or stops showing) ClaxBot as working on the run's threads.
    async fn working(&self, _run: &Run, _on: bool) {}

    /// That the run may propose on its report now, asked before anything
    /// is pushed: the report is not closed and no one's claim holds it, or
    /// the fix under [`OpCtx::key`] is on it already, for a step executed
    /// again.
    async fn check_fix(&self, _cx: &OpCtx<'_>) -> Result<(), HostError> {
        Ok(())
    }

    /// Proposes `fix` on the report the run works on, as ClaxBot acting
    /// for the run's person ([`OpCtx::audit`]), for its reporter's verdict:
    /// the fix is an agent's, kept with the run. [`ProposedFix::key`] makes
    /// it one attempt however often it is called, and a fix still waiting
    /// is superseded: a verdict is on what the reporter was shown.
    async fn record_fix(&self, _cx: &OpCtx<'_>, _fix: &ProposedFix) -> Result<(), HostError> {
        Err(HostError::new(
            "forbidden",
            "this host does not take fixes from a run",
        ))
    }

    /// The preview of the fix run `run` proposed serves at `verify_url`.
    /// With `for_reporter` the fix takes the link and its reporter is told
    /// in fixed words; without, a held fix keeps it for the queue's members,
    /// who alone are told. The run may have ended; called again it adds
    /// nothing, and a fix no longer the run's takes nothing.
    async fn preview_ready(
        &self,
        _run: &Run,
        _audit: &AuditCtx,
        _verify_url: &str,
        _for_reporter: bool,
    ) -> Result<(), HostError> {
        Err(HostError::new(
            "forbidden",
            "this host does not take previews from a run",
        ))
    }

    /// The wait for the preview of the fix run `run` proposed ended with
    /// none: while that fix is the report's, waiting and held, the queue's
    /// members are told [`crate::workspace::PREVIEW_MISSED`]. The run may have
    /// ended; called again it adds nothing.
    async fn preview_missed(&self, _run: &Run, _audit: &AuditCtx) -> Result<(), HostError> {
        Ok(())
    }

    /// Fixed words for the queue's members alone, on the report `run` works
    /// on. Called again it adds nothing.
    async fn note_members(
        &self,
        _run: &Run,
        _audit: &AuditCtx,
        _body: &str,
    ) -> Result<(), HostError> {
        Ok(())
    }

    /// Starts the runs that were waiting when a driver starts: the
    /// follow-ups queued on threads no run works on (a run's end committed
    /// without its `working(false)`, as a crash between them leaves it).
    /// The driver resumes them with the rest. Returns how many started.
    async fn start_waiting(&self) -> clax_core::Result<usize> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// FakeBotHost
// ---------------------------------------------------------------------------

/// One write [`FakeBotHost`] performed (committed).
#[derive(Clone, Debug, PartialEq)]
pub struct FakeWrite {
    pub key: String,
    pub op: String,
    pub args: Vec<Value>,
}

/// The state of a [`FakeBotHost`].
#[derive(Debug, Default)]
pub struct FakeState {
    pub context: RequestContext,
    /// Each write committed, in order.
    pub writes: Vec<FakeWrite>,
    /// Every call that reached the host (key, op), repeats included.
    pub calls: Vec<(String, String)>,
    /// Published drafts, in order.
    pub versions: Vec<PublishDraft>,
    /// Working state changes, in order.
    pub working: Vec<bool>,
    /// People who may read but not write: checked when a write is planned
    /// and again inside its transaction.
    pub readers: Vec<String>,
    /// Each fix recorded, in order, once per report it waits on.
    pub fixes: Vec<ProposedFix>,
    /// Each preview told of, and whether it was for the reporter.
    pub previews: Vec<(String, bool)>,
    /// The runs whose wait for a preview ended with none seen.
    pub previews_missed: Vec<String>,
    /// Each note for the queue's members, by [`BotHost::note_members`].
    pub member_notes: Vec<String>,
    /// An error to answer the next call of an op with (`"context"` for
    /// the next [`BotHost::context`]).
    pub fail_next: BTreeMap<String, HostError>,
    /// [`BotHost::record_fix`] never answers, as a host cut off by a Stop.
    pub record_fix_hangs: bool,
    /// [`BotHost::record_fix`] was reached and is not answering.
    pub record_fix_reached: bool,
    /// Done once, as the next reply is planned.
    pub before_reply: Option<BeforeReply>,
}

/// What a test does as a [`FakeBotHost`] plans a reply, after the run has
/// said who the reply is for.
pub struct BeforeReply(pub Box<dyn FnOnce() + Send>);

impl std::fmt::Debug for BeforeReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BeforeReply")
    }
}

/// A [`BotHost`] over the store for tests: threads, replies, resolves,
/// documents, versions and created artifacts are the store's own, written
/// through its journaled write paths, each guarded inside its transaction
/// by the acting person's write rights ([`FakeState::readers`]). It
/// enforces what a real host must ([`crate::host_conformance`]).
pub struct FakeBotHost {
    pub store: Arc<Store>,
    pub state: Mutex<FakeState>,
}

impl std::fmt::Debug for FakeBotHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBotHost").finish_non_exhaustive()
    }
}

fn forbidden(m: impl Into<String>) -> HostError {
    HostError::new("forbidden", m)
}

/// The document path of `db.*` arguments `collection` and `id`.
fn doc_path(args: &[Value]) -> String {
    let s = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or("");
    format!("{}/{}", s(0), s(1))
}

/// A caller with every right on the artifact: the host's own checks
/// stand for the person's.
fn caller() -> clax_core::db::Caller {
    clax_core::db::Caller::new(clax_core::db::Level::Owner, None)
}

fn doc_json(d: &clax_store::docs::Doc) -> Value {
    json!({"id": d.id, "version": d.version, "data": d.data})
}

impl FakeBotHost {
    #[must_use]
    pub fn new(store: Arc<Store>, context: RequestContext) -> FakeBotHost {
        FakeBotHost {
            store,
            state: Mutex::new(FakeState {
                context,
                ..FakeState::default()
            }),
        }
    }

    /// The state, locked.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// ClaxBot's replies in the store on `threads`, by thread, oldest
    /// first.
    #[must_use]
    pub fn replies(&self, threads: &[&str]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for t in threads {
            if let Ok(Some(t)) = self.store.get_thread(t) {
                for c in t.comments.iter().filter(|c| c.author_kind == "agent") {
                    out.push((t.id.clone(), c.body.clone()));
                }
            }
        }
        out
    }

    fn artifact(&self, run: &Run) -> Result<ArtifactId, HostError> {
        ArtifactId::parse(&run.artifact_id).map_err(|e| host_error(&e))
    }

    fn current_version(&self, run: &Run) -> Result<u32, HostError> {
        self.store
            .get_artifact(&self.artifact(run)?)
            .map_err(|e| host_error(&e))?
            .map(|a| a.current_version)
            .ok_or_else(|| HostError::new("not_found", "the artifact is gone"))
    }

    /// Thread `id`, refused unless it is on the run's artifact and one of
    /// the run's.
    fn thread(&self, run: &Run, id: &str) -> Result<clax_core::model::Thread, HostError> {
        let t = self
            .store
            .get_thread(id)
            .map_err(|e| host_error(&e))?
            .filter(|t| t.artifact_id == run.artifact_id)
            .ok_or_else(|| {
                HostError::new("not_found", format!("no thread {id} on this artifact"))
            })?;
        if !run.thread_ids.iter().any(|x| x == id) {
            return Err(forbidden(format!(
                "thread {id} is not part of this request"
            )));
        }
        Ok(t)
    }

    /// Whether `person` may write: the check a write's guard repeats inside
    /// its transaction.
    fn may_write(&self, person: &str) -> clax_core::Result<()> {
        if self.lock().readers.iter().any(|r| r == person) {
            return Err(CoreError::invalid(
                "forbidden",
                format!("{person} may not write here"),
            ));
        }
        Ok(())
    }

    fn check_write(&self, cx: &OpCtx<'_>, op: &str) -> Result<(), HostError> {
        self.lock().calls.push((cx.key.clone(), op.to_string()));
        self.may_write(&cx.run.person).map_err(|e| host_error(&e))?;
        if op == "artifacts.create" && cx.run.request != RequestKind::Brief {
            return Err(forbidden(
                "only a new artifact's brief may create artifacts",
            ));
        }
        if let Some(e) = self.lock().fail_next.remove(op) {
            return Err(e);
        }
        Ok(())
    }

    fn log_write(&self, key: String, op: &str, args: Vec<Value>) {
        self.lock().writes.push(FakeWrite {
            key,
            op: op.to_string(),
            args,
        });
    }

    fn reply_plan<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        thread_id: &str,
        markdown: &str,
    ) -> Result<WritePlan<'s>, HostError> {
        let before = self.lock().before_reply.take();
        if let Some(BeforeReply(act)) = before {
            act();
        }
        self.thread(cx.run, thread_id)?;
        let (ctx, tid, body, key, person) = (
            cx.audit.clone(),
            thread_id.to_string(),
            markdown.to_string(),
            cx.key.clone(),
            cx.run.person.clone(),
        );
        let members_only = cx.members_only;
        Ok(Box::new(move |entry: CallEntry| {
            Box::pin(async move {
                let c = clax_store::threads::NewComment {
                    author_kind: clax_core::model::AUTHOR_AGENT,
                    author_name: "ClaxBot".into(),
                    author_public_id: None,
                    via_session_id: None,
                    body: body.clone(),
                    via_page: false,
                };
                let guard = |_: &clax_store::Tx<'_>| self.may_write(&person);
                let j = if members_only {
                    self.store
                        .add_members_comment_journaled(&ctx, &tid, c, &entry, &guard)?
                } else {
                    self.store
                        .add_comment_journaled(&ctx, &tid, c, &entry, &guard)?
                };
                self.log_write(key, "threads.reply", vec![json!(tid), json!(body)]);
                Ok(j)
            }) as PlanFuture<'s>
        }))
    }
}

/// Whether `op` is a host operation with access `access`.
fn host_op(op: &str, access: api::Access) -> Result<(), HostError> {
    match api::op(op) {
        Some(o) if o.performer == api::Performer::Host && o.access == access => Ok(()),
        _ => Err(HostError::new(
            "unknown_op",
            format!("{op} is not a host {access:?} operation"),
        )),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl BotHost for FakeBotHost {
    async fn context(&self, _run: &Run) -> Result<RequestContext, HostError> {
        let mut st = self.lock();
        if let Some(e) = st.fail_next.remove("context") {
            return Err(e);
        }
        Ok(st.context.clone())
    }

    async fn read(&self, cx: &OpCtx<'_>, op: &str, args: &[Value]) -> Result<Value, HostError> {
        host_op(op, api::Access::Read)?;
        let version = self.current_version(cx.run)?;
        let aid = self.artifact(cx.run)?;
        {
            let mut st = self.lock();
            st.calls.push((cx.key.clone(), op.to_string()));
            if let Some(e) = st.fail_next.remove(op) {
                return Err(e);
            }
        }
        let s = |i: usize| {
            args.get(i)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let he = |e: CoreError| host_error(&e);
        match op {
            "artifact.get" => {
                let st = self.lock();
                let a = &st.context.artifact;
                Ok(
                    json!({"id": a.id, "title": a.title, "description": a.description,
                          "version": version}),
                )
            }
            "threads.read" => {
                let t = self.thread(cx.run, &s(0))?;
                Ok(json!({"id": t.id, "resolved": t.status == "resolved",
                    "comments": t.comments.iter().map(|c| json!({"id": c.id, "author": c.author_name,
                        "at": c.created_at, "text": c.body})).collect::<Vec<_>>()}))
            }
            "threads.list" => {
                let mut out = Vec::new();
                for id in &cx.run.thread_ids {
                    if let Ok(t) = self.thread(cx.run, id) {
                        out.push(json!({"id": t.id, "resolved": t.status == "resolved",
                                        "comments": t.comments.len()}));
                    }
                }
                Ok(Value::Array(out))
            }
            "db.get" => Ok(Docs::of(&self.store)
                .doc_get(&aid, &doc_path(args), &caller())
                .map_err(he)?
                .map_or(Value::Null, |d| doc_json(&d))),
            "db.list" | "db.query" => {
                let q = clax_store::docs::DocQuery {
                    collection: s(0),
                    filters: Vec::new(),
                    order_by: None,
                    descending: false,
                    limit: None,
                    cursor: None,
                };
                let (docs, next) = Docs::of(&self.store)
                    .doc_query(&aid, &q, &caller())
                    .map_err(he)?;
                let docs: Vec<Value> = docs.iter().map(doc_json).collect();
                Ok(if op == "db.list" {
                    json!({"docs": docs, "next_cursor": next})
                } else {
                    Value::Array(docs)
                })
            }
            "db.collections" => Ok(json!([])),
            other => Err(HostError::new(
                "unknown_op",
                format!("{other} is not a host read"),
            )),
        }
    }

    async fn plan_write<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        op: &str,
        args: &[Value],
    ) -> Result<WritePlan<'s>, HostError> {
        host_op(op, api::Access::Write)?;
        self.check_write(cx, op)?;
        let s = |i: usize| {
            args.get(i)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        if op == "threads.reply" {
            return self.reply_plan(cx, &s(0), &s(1));
        }
        if op == "threads.resolve" {
            self.thread(cx.run, &s(0))?;
        }
        let (ctx, key, person) = (cx.audit.clone(), cx.key.clone(), cx.run.person.clone());
        let (op, args) = (op.to_string(), args.to_vec());
        let aid = self.artifact(cx.run)?;
        Ok(Box::new(move |entry: CallEntry| {
            Box::pin(async move {
                let guard = |_: &clax_store::Tx<'_>| self.may_write(&person);
                let path = doc_path(&args);
                let pin = |i: usize| {
                    let v = args
                        .get(i)
                        .and_then(|o| o.get("if_version"))
                        .and_then(Value::as_u64);
                    clax_store::docs::Pin {
                        if_version: v,
                        lww: v.is_none(),
                    }
                };
                let j = match op.as_str() {
                    "threads.resolve" => self.store.resolve_thread_journaled(
                        &ctx,
                        args[0].as_str().unwrap_or(""),
                        "agent:claxbot",
                        Some(&person),
                        &entry,
                        &guard,
                    )?,
                    "db.set" | "db.update" | "db.delete" => {
                        let write = match op.as_str() {
                            "db.set" => JournaledDoc::Set(args[2].clone(), pin(3)),
                            "db.update" => JournaledDoc::Update(args[2].clone(), pin(3)),
                            _ => JournaledDoc::Delete(pin(2)),
                        };
                        Docs::of(&self.store).doc_write_journaled(
                            &ctx,
                            &aid,
                            &path,
                            write,
                            &caller(),
                            &entry,
                            &guard,
                        )?
                    }
                    "artifacts.create" => {
                        let spec = &args[0];
                        let req: clax_core::publish::PublishRequest =
                            serde_json::from_value(json!({
                                "title": spec["title"],
                                "description": spec["description"],
                                "files": {"index.html": {"encoding": "utf8",
                                    "content": "<!doctype html><title>New artifact</title><p>Being built.</p>"}}
                            }))
                            .map_err(|e| CoreError::invalid("invalid", e.to_string()))?;
                        let p = clax_core::publish::validate(req)?;
                        self.store
                            .create_artifact_journaled(&ctx, p, None, &entry, &guard)
                            .await?
                    }
                    other => {
                        return Err(CoreError::invalid(
                            "unknown_op",
                            format!("{other} is not a host write"),
                        ));
                    }
                };
                self.log_write(key, &op, args);
                Ok(j)
            }) as PlanFuture<'s>
        }))
    }

    async fn plan_publish<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        draft: PublishDraft,
    ) -> Result<WritePlan<'s>, HostError> {
        self.check_write(cx, "draft.publish")?;
        let aid = self.artifact(cx.run)?;
        let current = self.current_version(cx.run)?;
        let mut files: serde_json::Map<String, Value> = draft
            .files
            .iter()
            .map(|f| {
                (
                    f.path.clone(),
                    json!({"content": String::from_utf8_lossy(&f.content), "encoding": "utf8"}),
                )
            })
            .collect();
        // The whole draft: what the current version has beyond it is removed.
        if let Some(v) = self
            .store
            .get_version(&aid, current)
            .map_err(|e| host_error(&e))?
        {
            for path in v.files.keys() {
                files.entry(path.clone()).or_insert(Value::Null);
            }
        }
        let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
            "files": files,
            "note": draft.note,
            "if_version": draft.if_version,
        }))
        .map_err(|e| HostError::new("invalid", e.to_string()))?;
        let p = clax_core::publish::validate(req).map_err(|e| host_error(&e))?;
        let (ctx, key, person) = (cx.audit.clone(), cx.key.clone(), cx.run.person.clone());
        Ok(Box::new(move |entry: CallEntry| {
            Box::pin(async move {
                let guard = |_: &clax_store::Tx<'_>| self.may_write(&person);
                let j = self
                    .store
                    .publish_version_journaled(&ctx, &aid, p, None, &entry, &guard)
                    .await?;
                let mut st = self.lock();
                st.writes.push(FakeWrite {
                    key,
                    op: "draft.publish".into(),
                    args: vec![json!(draft.note), json!(draft.addresses)],
                });
                st.versions.push(draft);
                Ok(j)
            }) as PlanFuture<'s>
        }))
    }

    async fn plan_reply<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        thread_id: &str,
        markdown: &str,
    ) -> Result<WritePlan<'s>, HostError> {
        self.lock().calls.push((cx.key.clone(), "reply".into()));
        self.reply_plan(cx, thread_id, markdown)
    }

    async fn working(&self, _run: &Run, on: bool) {
        self.lock().working.push(on);
    }

    async fn check_fix(&self, _cx: &OpCtx<'_>) -> Result<(), HostError> {
        match self.lock().fail_next.remove("check_fix") {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn record_fix(&self, cx: &OpCtx<'_>, fix: &ProposedFix) -> Result<(), HostError> {
        if let Some(e) = self.lock().fail_next.remove("record_fix") {
            return Err(e);
        }
        let hangs = self.lock().record_fix_hangs;
        if hangs {
            self.lock().record_fix_reached = true;
            std::future::pending::<()>().await;
        }
        self.lock().fixes.push(fix.clone());
        let t = cx
            .run
            .thread_ids
            .iter()
            .find_map(|t| self.store.get_thread(t).ok().flatten())
            .filter(|t| t.status != "resolved")
            .ok_or_else(|| HostError::new("not_found", "the run works on no report"))?;
        let Some(qid) = t.queue_id else {
            return Ok(());
        };
        let by = clax_store::queues::ReportWriter::Run {
            run: cx.run.id.clone(),
            name: "ClaxBot".into(),
            key: fix.key.clone(),
            note: fix.members_note.clone(),
            held: fix.held,
            preview_url: fix.preview_url.clone(),
        };
        let new = clax_core::report::NewFix {
            summary: fix.summary.clone(),
            url: fix.url.clone(),
            commit: fix.commit.clone(),
            release: None,
            verify_url: fix.verify_url.clone(),
            verify_after: None,
        };
        self.store
            .propose_fix(cx.audit, &qid, &t.id, &by, new)
            .map(drop)
            .map_err(|e| host_error(&e))
    }

    async fn preview_ready(
        &self,
        _run: &Run,
        _audit: &AuditCtx,
        verify_url: &str,
        for_reporter: bool,
    ) -> Result<(), HostError> {
        self.lock()
            .previews
            .push((verify_url.to_string(), for_reporter));
        Ok(())
    }

    async fn preview_missed(&self, run: &Run, _audit: &AuditCtx) -> Result<(), HostError> {
        self.lock().previews_missed.push(run.id.clone());
        Ok(())
    }

    async fn note_members(
        &self,
        _run: &Run,
        _audit: &AuditCtx,
        body: &str,
    ) -> Result<(), HostError> {
        self.lock().member_notes.push(body.to_string());
        Ok(())
    }
}
