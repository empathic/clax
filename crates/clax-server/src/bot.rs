//! ClaxBot in the server (spec
//! `docs/superpowers/specs/2026-10-21-claxbot-design.md` §4, §6): the
//! [`BotHost`] over the routes' own code paths ([`ServerHost`]), the
//! sandbox the build has ([`sandbox`]: embedded QuickJS with the
//! `native-sandbox` feature on macOS and Linux, else
//! [`UnavailableSandbox`]), and the native
//! daemon's driver ([`start_driver`]).
//!
//! The host acts as the person a run acts for, with that person's rights:
//! it reads only what they may read, only on the run's artifact, and only
//! the run's threads. On a hosted daemon those rights hold within the
//! person's reach alone (the owner's included, spec 2026-10-22 §3): an
//! artifact another person made is reached only through its share with
//! them.
//!
//! A run proposes a fix on its report as itself
//! ([`clax_store::queues::ReportWriter::Run`]): an agent's fix, kept with
//! the run, and the audit trail names ClaxBot acting for the run's person.
//! ClaxBot is no harness and takes no feedback rows: what wakes it is
//! [`report_follow_up`].

use crate::routes::bot::reads_artifact;
use clax_bot::host::{
    ArtifactMeta, CommentMeta, PlanFuture, PublishDraft, RequestContext, ThreadMeta, WritePlan,
    host_error,
};
use clax_bot::{BotHost, OpCtx};
use clax_bot_sandbox::{
    CancelToken, ErrorKind, Host, HostError, Limits, Sandbox, Script, ScriptOutcome,
};
use clax_core::ArtifactId;
use clax_core::rights::RightType;
use clax_http::auth::Caller;
use clax_http::state::AppState;
use clax_store::Store;
use clax_store::app_data::{AppDataOp, WireCaller};
use clax_store::bot_runs::CallEntry;
use clax_store::bot_runs::{RequestKind, Run};
use clax_store::queues::{ReportWriter, Visibility};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;

/// The sandbox of a build without one (`native-sandbox` off, Android and
/// iOS, or the Worker until it installs its own): every script ends `unavailable`
/// before it runs, so its run waits, uncharged.
pub struct UnavailableSandbox;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Sandbox for UnavailableSandbox {
    async fn run(
        &self,
        _script: &Script,
        _host: &dyn Host,
        _limits: &Limits,
        _cancel: &CancelToken,
    ) -> ScriptOutcome {
        ScriptOutcome::failed(
            ErrorKind::Unavailable,
            "this Clax was built without a script sandbox",
        )
    }
}

/// The sandbox this build runs scripts in.
pub fn sandbox() -> Arc<dyn Sandbox> {
    #[cfg(all(
        feature = "native-sandbox",
        not(any(target_arch = "wasm32", target_os = "android", target_os = "ios"))
    ))]
    {
        Arc::new(clax_bot_js::QuickJs::new(clax_bot::api::op_names()))
    }
    #[cfg(not(all(
        feature = "native-sandbox",
        not(any(target_arch = "wasm32", target_os = "android", target_os = "ios"))
    )))]
    {
        Arc::new(UnavailableSandbox)
    }
}

/// The [`Caller`] the person a run acts for is: the owner (by hand), or
/// the connected person by their row; `None` when they are no longer one.
///
/// # Errors
/// The store's.
pub fn person_caller(st: &Store, person: &str) -> clax_core::Result<Option<Caller>> {
    if person == OWNER {
        return Ok(Some(Caller::Owner { browser: false }));
    }
    Ok(st
        .viewer_by_public_id(person)?
        .filter(|v| v.person && v.person_revoked_at.is_none())
        .map(|viewer| Caller::Person { viewer }))
}

/// The guard of a ClaxBot document write of `run`'s, run inside the
/// instance's transactions around it: whether its person may still write
/// documents on the run's artifact.
fn bot_guard<'a>(
    st: &'a Store,
    run: &'a Run,
) -> impl Fn(&clax_store::Tx<'_>) -> clax_core::Result<()> + Send + Sync + 'a {
    move |tx: &clax_store::Tx<'_>| {
        st.check_bot_write_in(
            tx,
            &run.person,
            run.agent_grant.as_deref(),
            Some(&run.artifact_id),
            RightType::Db,
            "write",
        )
    }
}

/// A read [`ServerHost::read_now`] answers, or the one its artifact's own
/// store answers, and how to show that store's reply.
enum ReadNow {
    Done(Value),
    Data(ArtifactId, Box<AppDataOp>, DataRead),
}

impl ReadNow {
    fn data(id: ArtifactId, op: AppDataOp, kind: DataRead) -> ReadNow {
        ReadNow::Data(id, Box::new(op), kind)
    }
}

/// How a read of an artifact's own store shows its reply.
enum DataRead {
    /// `db.collections`: the collection names.
    Collections,
    /// `db.get`: the document, or `null`.
    Get,
    /// `db.list` (`{docs, next_cursor}`) or `db.query` (the documents).
    Page { list: bool },
}

/// The document caller a run reads (`action` `read`) or writes (`write`)
/// as: the owner's own run at the owner's level; otherwise its person,
/// at the `clax-db` level the run's rights ([`Store::bot_rights`]: its
/// person's, narrowed by the asking agent's grant) give that action at its
/// artifact (`view` where none does), the owner's agent as the owner's
/// row. On a hosted daemon (`hosted`), at an artifact beyond its person's
/// reach (another person's, the owner's own run included), the level is
/// the one the artifact's share with that person gives, never above the
/// run's rights' (`view` where none does).
fn db_caller(
    st: &Store,
    hosted: bool,
    run: &Run,
    action: &str,
) -> Result<clax_core::db::Caller, HostError> {
    use clax_core::db::Level;
    use clax_core::visibility::Author;
    let rights = st
        .bot_rights(&run.person, run.agent_grant.as_deref())
        .map_err(|e| host_error(&e))?;
    let owner = run.person == OWNER;
    let maker = (!owner).then_some(run.person.as_str());
    let beyond = hosted
        && st
            .reach(&run.artifact_id, maker)
            .map_err(|e| host_error(&e))?
            != clax_store::reach::Reach::Within;
    if rights.is_none() && !beyond {
        return Ok(clax_core::db::Caller::new(Level::Owner, None));
    }
    let own = |x: &str| st.artifact_created_by(x, maker).unwrap_or(false);
    let granted = rights
        .as_ref()
        .map(|r| r.db_level(&run.artifact_id, action, &own));
    let level = if beyond {
        let shared = clax_http::hosted::shares_of(st, maker)
            .map_err(|e| host_error(&e))?
            .db_level(&run.artifact_id, action, &|_| false);
        match granted {
            None => shared,
            Some(g) => shared.zip(g).map(|(s, g)| s.min(g)),
        }
    } else {
        granted.flatten()
    }
    .unwrap_or(Level::View);
    let viewer = if owner {
        st.owner().map_err(|e| host_error(&e))?.map(|v| v.public_id)
    } else {
        Some(run.person.clone())
    };
    Ok(clax_core::db::Caller {
        level,
        viewer,
        person: if owner {
            Author::Owner
        } else {
            Author::Person(run.person.clone())
        },
        via_page: false,
    })
}

/// ClaxBot's [`BotHost`] over the server's state.
pub struct ServerHost {
    pub state: AppState,
    /// The driver the follow-ups a run's end starts are handed to, once
    /// one drives this host ([`driver`] sets it).
    wake: Arc<std::sync::OnceLock<Arc<dyn clax_http::bot::Runs>>>,
    /// This host's runs propose fixes, so it answers their reporters
    /// ([`report_follow_up`]): it has a workspace, which says which
    /// queues' targets do. Without one nothing follows.
    workspace: Option<HeldWorkspace>,
    /// Reports' changes wait for [`ServerHost::follow_reports`], rather than
    /// a task of the host's own.
    asked: bool,
    /// The task listening for reports' changes was started, or `changes`
    /// set.
    listening: Arc<std::sync::atomic::AtomicBool>,
    /// The bus's events [`ServerHost::follow_reports`] has not read.
    changes: Arc<std::sync::Mutex<Option<Receiver<clax_core::Stamped>>>>,
}

/// A host's workspace, as a job on the store takes it. On wasm32 a workspace
/// is not `Send` (a Worker has one thread), so it is held in a `SendWrapper`
/// there, as [`DriverRuns`] holds a driver.
#[derive(Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct HeldWorkspace(Arc<dyn clax_bot::Workspace>);
/// A host's workspace, on the Worker's one thread.
#[derive(Clone)]
#[cfg(target_arch = "wasm32")]
struct HeldWorkspace(send_wrapper::SendWrapper<Arc<dyn clax_bot::Workspace>>);

impl HeldWorkspace {
    fn of(workspace: Arc<dyn clax_bot::Workspace>) -> HeldWorkspace {
        #[cfg(not(target_arch = "wasm32"))]
        {
            HeldWorkspace(workspace)
        }
        #[cfg(target_arch = "wasm32")]
        {
            HeldWorkspace(send_wrapper::SendWrapper::new(workspace))
        }
    }

    fn get(&self) -> &dyn clax_bot::Workspace {
        self.0.as_ref()
    }
}

/// The host's subscription, whatever a panic left of its lock.
fn lock_changes(
    changes: &std::sync::Mutex<Option<Receiver<clax_core::Stamped>>>,
) -> std::sync::MutexGuard<'_, Option<Receiver<clax_core::Stamped>>> {
    changes.lock().unwrap_or_else(|e| e.into_inner())
}

fn forbidden(what: impl Into<String>) -> HostError {
    HostError::new("forbidden", what)
}

impl ServerHost {
    /// The host over `state`, with no workspace. On wasm32 it
    /// [follows reports when asked](ServerHost::follows_when_asked).
    pub fn new(state: AppState) -> ServerHost {
        ServerHost {
            state,
            wake: Arc::default(),
            workspace: None,
            asked: cfg!(target_arch = "wasm32"),
            listening: Arc::default(),
            changes: Arc::default(),
        }
    }

    /// This host, with `workspace` saying which queues' reports it follows
    /// ([`report_follow_up`]). The runs get theirs from [`clax_bot::Deps`].
    #[must_use]
    pub fn with_workspace(mut self, workspace: Arc<dyn clax_bot::Workspace>) -> ServerHost {
        self.workspace = Some(HeldWorkspace::of(workspace));
        self
    }

    /// This host, with no task listening for reports' changes: they wait for
    /// [`ServerHost::follow_reports`], which its owner calls when the work
    /// that changes them ends. A Worker's host is always so: a task of its
    /// own could start a run after the request that woke it had looked for
    /// wakes, and the alarm would never hear of it.
    #[must_use]
    pub fn follows_when_asked(mut self) -> ServerHost {
        self.asked = true;
        self
    }

    /// Hands the follow-ups a run's end starts to `runs` (the daemon's
    /// [`driver`] does it for its own; the Worker for its alarm's). The
    /// first call wins.
    pub fn set_runs(&self, runs: Arc<dyn clax_http::bot::Runs>) {
        let _ = self.wake.set(runs);
    }

    /// Another handle on this host.
    fn me(&self) -> ServerHost {
        ServerHost {
            state: self.state.clone(),
            wake: self.wake.clone(),
            workspace: self.workspace.clone(),
            asked: self.asked,
            listening: self.listening.clone(),
            changes: self.changes.clone(),
        }
    }

    /// What follows a report's change on this host.
    fn follow(&self) -> Follow {
        Follow {
            workspace: self.workspace.clone(),
            s: self.state.feedback_ctx(),
        }
    }

    /// Starts hearing of reports' changes, once: a task that hands each to
    /// [`report_follow_up`] or, on a host that
    /// [follows when asked](ServerHost::follows_when_asked), a subscription
    /// for [`ServerHost::follow_reports`] to read. An event lost to a full
    /// bus or a crash is made up by the report's next change or its run's
    /// end.
    fn listen_to_reports(&self) {
        use std::sync::atomic::Ordering;
        use tokio::sync::broadcast::error::RecvError;
        let Some(ws) = self.workspace.clone() else {
            return;
        };
        if self.listening.swap(true, Ordering::SeqCst) {
            return;
        }
        let (state, wake) = (self.state.clone(), self.wake.clone());
        let mut events = state.events.subscribe();
        if self.asked {
            *lock_changes(&self.changes) = Some(events);
            return;
        }
        clax_rt::spawn(async move {
            loop {
                let tid = match events.recv().await {
                    Ok(s) => match s.event {
                        clax_core::Event::Report { report_id, .. } => report_id,
                        _ => continue,
                    },
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                };
                let (bot, s, ws) = (state.bot.clone(), state.feedback_ctx(), ws.clone());
                let started = state
                    .store
                    .call(move |st| report_follow_up(st, &bot, &s, ws.get(), &tid))
                    .await;
                match started {
                    Ok(Some(run)) => {
                        if let Some(w) = wake.get() {
                            w.started(&run);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "a report's follow-up did not start"),
                }
            }
        });
    }

    /// Follows up the reports changed since it was last called, on a host
    /// that [follows when asked](ServerHost::follows_when_asked): each goes to
    /// [`report_follow_up`], under the rules a daemon's listener follows them
    /// by, and what it starts goes to [`ServerHost::set_runs`]'s seam before
    /// this returns. A host that fell behind the bus has lost changes, so it
    /// looks for them as a driver's start does. Its owner calls this after
    /// the work that changes reports, and before it looks for wakes.
    pub async fn follow_reports(&self) {
        let (mut tids, behind) = self.changed_reports();
        if tids.is_empty() && !behind {
            return;
        }
        let (bot, follow) = (self.state.bot.clone(), self.follow());
        let started = self
            .state
            .store
            .call(move |st| {
                if behind {
                    tids.extend(st.reports_of_run_fixes()?);
                }
                Ok(follow.reports(st, &bot, &tids))
            })
            .await;
        match started {
            Ok(runs) => {
                if let Some(w) = self.wake.get() {
                    for run in &runs {
                        w.started(run);
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "a report's follow-up did not start"),
        }
    }

    /// The reports the bus has told of since the last look, and whether it
    /// dropped events first.
    fn changed_reports(&self) -> (std::collections::BTreeSet<String>, bool) {
        use tokio::sync::broadcast::error::TryRecvError;
        let (mut tids, mut behind) = (std::collections::BTreeSet::new(), false);
        let mut changes = lock_changes(&self.changes);
        let Some(events) = changes.as_mut() else {
            return (tids, behind);
        };
        loop {
            match events.try_recv() {
                Ok(s) => {
                    if let clax_core::Event::Report { report_id, .. } = s.event {
                        tids.insert(report_id);
                    }
                }
                Err(TryRecvError::Lagged(_)) => behind = true,
                Err(TryRecvError::Empty | TryRecvError::Closed) => return (tids, behind),
            }
        }
    }

    /// The run's artifact, when its person may still read it.
    fn artifact(&self, st: &Store, run: &Run) -> Result<clax_core::model::Artifact, HostError> {
        let id = ArtifactId::parse(&run.artifact_id).map_err(|e| host_error(&e))?;
        let a = st
            .get_artifact(&id)
            .map_err(|e| host_error(&e))?
            .ok_or_else(|| HostError::new("not_found", "the run's artifact is gone"))?;
        let caller = person_caller(st, &run.person)
            .map_err(|e| host_error(&e))?
            .ok_or_else(|| forbidden("the person this run acts for is no longer connected"))?;
        let live = self.state.live_ids.contains(&run.artifact_id);
        let hosted = self.state.hosted.is_some();
        if !reads_artifact(st, hosted, &caller, &run.artifact_id, live)
            .map_err(|e| host_error(&e))?
            || !self.allows(st, run, RightType::Artifacts, "read")?
        {
            return Err(forbidden(
                "the person this run acts for may not read its artifact",
            ));
        }
        Ok(a)
    }

    /// One of the run's threads, on its artifact.
    fn thread(st: &Store, run: &Run, tid: &str) -> Result<clax_core::model::Thread, HostError> {
        if !run.thread_ids.iter().any(|t| t == tid) {
            return Err(HostError::new(
                "not_found",
                format!("thread {tid} is not one of this run's"),
            ));
        }
        match st.get_thread(tid).map_err(|e| host_error(&e))? {
            Some(t) if t.artifact_id == run.artifact_id => Ok(t),
            _ => Err(HostError::new("not_found", format!("no thread {tid}"))),
        }
    }

    /// Refuses a run closing report `t` while a fix waits for its verdict:
    /// that is its reporter's to give, or a person's, and closed it would take
    /// no verdict and no fix from anyone.
    fn check_verdict_open(st: &Store, t: &clax_core::model::Thread) -> Result<(), HostError> {
        let Some(q) = &t.queue_id else {
            return Ok(());
        };
        let report = st
            .report(q, &t.id, Visibility::All)
            .map_err(|e| host_error(&e))?;
        if report.is_some_and(|r| r.state == clax_core::queue::State::FixProposed) {
            return Err(HostError::new(
                "fix_waiting",
                "this report's fix waits for its reporter's verdict: leave the report open and \
                 finish",
            ));
        }
        Ok(())
    }

    /// Whether `c` is a comment by the run's person.
    fn by_sender(st: &Store, run: &Run, c: &clax_core::model::Comment) -> bool {
        if c.author_kind != clax_core::model::AUTHOR_VIEWER {
            return false;
        }
        let want = if run.person == OWNER {
            st.owner().ok().flatten().map(|v| v.public_id)
        } else {
            Some(run.person.clone())
        };
        want.is_some() && c.author_public_id == want
    }

    /// [`ServerHost::thread_meta`], with the notes for its queue's members
    /// when `t` is a report the run may read as one of them
    /// ([`Store::run_reads_queue`]): what an earlier run told them is this
    /// run's to read, and no other's. A person's note makes a run that reads
    /// it confidential ([`clax_bot::run::Deps::confidential`]), which is
    /// settled when a step begins, so one is read only by a run that is
    /// `confidential` already: a note newer than that waits for the next step.
    fn thread_with_notes(
        st: &Store,
        run: &Run,
        t: &clax_core::model::Thread,
        confidential: bool,
    ) -> Result<ThreadMeta, HostError> {
        let mut m = Self::thread_meta(st, run, t);
        let Some(q) = &t.queue_id else {
            return Ok(m);
        };
        let grant = run.agent_grant.as_deref();
        if !st
            .run_reads_queue(&run.person, grant, q)
            .map_err(|e| host_error(&e))?
        {
            return Ok(m);
        }
        let report = st
            .report(q, &t.id, Visibility::All)
            .map_err(|e| host_error(&e))?;
        for c in report.iter().flat_map(|r| &r.comments) {
            let by_a_run = c.author_kind == clax_core::model::AUTHOR_AGENT;
            if c.triage_only && (confidential || by_a_run) {
                m.comments.push(CommentMeta {
                    id: c.id.clone(),
                    author: c.author_name.clone(),
                    from_sender: false,
                    at: c.created_at.clone(),
                    text: c.body.clone(),
                });
            }
        }
        m.comments.sort_by(|a, b| a.at.cmp(&b.at));
        Ok(m)
    }

    fn thread_meta(st: &Store, run: &Run, t: &clax_core::model::Thread) -> ThreadMeta {
        ThreadMeta {
            id: t.id.clone(),
            anchor: serde_json::to_value(&t.anchor).ok(),
            resolved: t.status == "resolved",
            comments: t
                .comments
                .iter()
                .map(|c| CommentMeta {
                    id: c.id.clone(),
                    author: c.author_name.clone(),
                    from_sender: Self::by_sender(st, run, c),
                    at: c.created_at.clone(),
                    text: c.body.clone(),
                })
                .collect(),
        }
    }

    fn read_now(
        &self,
        st: &Store,
        cx: &OpCtx<'_>,
        op: &str,
        args: &[Value],
    ) -> Result<ReadNow, HostError> {
        let run = cx.run;
        let a = self.artifact(st, run)?;
        let id = ArtifactId::parse(&a.id).map_err(|e| host_error(&e))?;
        let s = |i: usize| {
            args.get(i)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let dbc = db_caller(st, self.state.hosted.is_some(), run, "read")?;
        let caps = st.doc_capabilities(&id).map_err(|e| host_error(&e))?;
        match op {
            "artifact.get" => Ok(ReadNow::Done(
                json!({"id": a.id, "title": a.title, "description": a.description,
                                       "version": a.current_version}),
            )),
            "db.collections" => Ok(ReadNow::data(
                id,
                AppDataOp::Collections {
                    caps,
                    caller: Some(WireCaller::from(&dbc)),
                },
                DataRead::Collections,
            )),
            "db.get" => Ok(ReadNow::data(
                id,
                AppDataOp::Get {
                    caps,
                    path: format!("{}/{}", s(0), s(1)),
                    caller: WireCaller::from(&dbc),
                },
                DataRead::Get,
            )),
            "db.list" | "db.query" => {
                let opts = args.get(1).cloned().unwrap_or(Value::Null);
                let filters = if op == "db.query" {
                    opts.as_object()
                        .map(|o| {
                            o.iter()
                                .map(|(k, v)| json!([k, "==", v]))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let q = clax_store::docs::DocQuery {
                    collection: s(0),
                    filters: clax_store::docs::parse_where(&json!(filters))
                        .map_err(|e| host_error(&e))?,
                    order_by: None,
                    descending: false,
                    limit: if op == "db.list" {
                        opts["limit"].as_u64().map(|n| n as usize)
                    } else {
                        None
                    },
                    cursor: if op == "db.list" {
                        opts["cursor"].as_str().map(str::to_string)
                    } else {
                        None
                    },
                };
                Ok(ReadNow::data(
                    id,
                    AppDataOp::Query {
                        caps,
                        query: q,
                        caller: WireCaller::from(&dbc),
                        view: None,
                    },
                    DataRead::Page {
                        list: op == "db.list",
                    },
                ))
            }
            "threads.list" => {
                let all = args
                    .first()
                    .and_then(|o| o["include_resolved"].as_bool())
                    .unwrap_or(false);
                let mut out = Vec::new();
                for tid in &run.thread_ids {
                    let t = Self::thread(st, run, tid)?;
                    if all || t.status != "resolved" {
                        out.push(json!({"id": t.id, "resolved": t.status == "resolved",
                                        "comments": t.comments.len()}));
                    }
                }
                Ok(ReadNow::Done(Value::Array(out)))
            }
            "threads.read" => {
                let t = Self::thread(st, run, &s(0))?;
                let m = Self::thread_with_notes(st, run, &t, cx.members_only)?;
                Ok(ReadNow::Done(
                    json!({"id": m.id, "resolved": m.resolved, "anchor": m.anchor,
                          "comments": m.comments}),
                ))
            }
            other => Err(HostError::new(
                "unknown_op",
                format!("{other} is not a host read"),
            )),
        }
    }

    async fn on_store<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> Result<T, HostError> + Send + 'static,
    ) -> Result<T, HostError> {
        self.state
            .store
            .call(move |st| Ok(f(st)))
            .await
            .map_err(|e| host_error(&e))?
    }

    /// Whether the run holds right `action` of `kind` at its artifact
    /// (`at` none: anywhere, as `create` asks): the rights
    /// [`Store::bot_rights`] gives, its person's narrowed by the grant of
    /// the agent that asked for it, so a run never exceeds that agent; the
    /// owner's own run every right. An artifact its person made is their
    /// own. On a hosted daemon (`hosted`) an artifact must also allow it by
    /// its person's scope ([`clax_http::hosted::MemberRights::allows_at`],
    /// the owner's for the owner's run), as the person's own request
    /// there would be asked.
    fn allows_at(
        st: &Store,
        hosted: bool,
        run: &Run,
        kind: RightType,
        action: &str,
        at: Option<&str>,
    ) -> Result<bool, HostError> {
        if let (true, Some(aid)) = (hosted, at) {
            use clax_http::hosted::MemberRights;
            let scope = if run.person == OWNER {
                MemberRights::owner(st)
            } else {
                MemberRights::of_person(st, &run.person)
            }
            .map_err(|e| host_error(&e))?;
            if !scope
                .allows_at(st, (kind, action), aid)
                .map_err(|e| host_error(&e))?
            {
                return Ok(false);
            }
        }
        let rights = st
            .bot_rights(&run.person, run.agent_grant.as_deref())
            .map_err(|e| host_error(&e))?;
        let Some(rights) = rights else {
            return Ok(true);
        };
        let maker = (run.person != OWNER).then_some(run.person.as_str());
        let own = |x: &str| st.artifact_created_by(x, maker).unwrap_or(false);
        Ok(rights.allows(kind, action, at, &own))
    }

    /// [`ServerHost::allows_at`] at the run's artifact.
    fn allows(
        &self,
        st: &Store,
        run: &Run,
        kind: RightType,
        action: &str,
    ) -> Result<bool, HostError> {
        let hosted = self.state.hosted.is_some();
        Self::allows_at(st, hosted, run, kind, action, Some(&run.artifact_id))
    }

    /// Whether the run may create artifacts (its person's own, so no
    /// artifact's scope applies).
    fn allows_create(st: &Store, run: &Run) -> Result<bool, HostError> {
        Self::allows_at(st, false, run, RightType::Artifacts, "create", None)
    }

    /// That the run's person may still write `kind`'s `action` on its
    /// artifact (and read it).
    fn check_write(
        &self,
        st: &Store,
        run: &Run,
        kind: RightType,
        action: &str,
    ) -> Result<(), HostError> {
        self.artifact(st, run)?;
        if !self.allows(st, run, kind, action)? {
            return Err(forbidden(format!(
                "the person this run acts for may not {action} {} here",
                match kind {
                    RightType::Comments => "comments",
                    RightType::Artifacts => "artifacts",
                    RightType::Db => "documents",
                    _ => "this",
                }
            )));
        }
        Ok(())
    }

    /// ClaxBot's name on a comment: acting for its person ("ClaxBot (for
    /// Sam)", spec §6.5).
    fn author(st: &Store, run: &Run) -> String {
        match clax_http::bot::Names::new(st).name(&run.person) {
            Ok(Some(n)) => format!("{} (for {n})", clax_http::bot::NAME),
            _ => clax_http::bot::NAME.to_string(),
        }
    }

    /// The plan of ClaxBot's reply `markdown` on the run's thread `tid`,
    /// as agent ClaxBot for its person, announced as any comment is; for a
    /// run holding confidential material ([`OpCtx::members_only`]) a note
    /// for the queue's members alone.
    fn reply_plan<'s>(&'s self, cx: &OpCtx<'_>, tid: &str, markdown: &str) -> WritePlan<'s> {
        let (state, ctx, run, tid, body) = (
            self.state.clone(),
            cx.audit.clone(),
            cx.run.clone(),
            tid.to_string(),
            markdown.to_string(),
        );
        let members_only = cx.members_only;
        Box::new(move |entry: CallEntry| {
            Box::pin(async move {
                let s = state.feedback_ctx();
                state
                    .store
                    .call(move |st| {
                        let c = clax_store::threads::NewComment {
                            author_kind: clax_core::model::AUTHOR_AGENT,
                            author_name: ServerHost::author(st, &run),
                            author_public_id: None,
                            via_session_id: None,
                            body,
                            via_page: false,
                        };
                        let guard = |tx: &clax_store::Tx<'_>| {
                            st.check_bot_write_in(
                                tx,
                                &run.person,
                                run.agent_grant.as_deref(),
                                Some(&run.artifact_id),
                                RightType::Comments,
                                "reply",
                            )
                        };
                        // Its reporter reads a report's comments: this is
                        // a note for the queue's members, announced to them
                        // alone, as the report's state.
                        if members_only {
                            let j =
                                st.add_members_comment_journaled(&ctx, &tid, c, &entry, &guard)?;
                            if let Some(t) = st.get_thread(&tid)? {
                                crate::routes::threads::publish_thread(&s, st, &t);
                            }
                            return Ok(j);
                        }
                        let j = st.add_comment_journaled(&ctx, &tid, c, &entry, &guard)?;
                        announce_thread(&s, st, &tid);
                        Ok(j)
                    })
                    .await
            }) as PlanFuture<'s>
        })
    }
}

/// The report run `run` works on: the first of its threads a queue holds.
fn report_thread(st: &Store, run: &Run) -> clax_core::Result<clax_core::model::Thread> {
    run.thread_ids
        .iter()
        .find_map(|tid| {
            st.get_thread(tid)
                .ok()
                .flatten()
                .filter(|t| t.queue_id.is_some())
        })
        .ok_or(clax_core::CoreError::NotFound)
}

/// [`report_follow_up`] as a host has it, to hand to the store's thread.
struct Follow {
    workspace: Option<HeldWorkspace>,
    s: clax_realtime::feedback::FeedbackCtx,
}

impl Follow {
    fn on(&self) -> bool {
        self.workspace.is_some()
    }

    /// The runs [`report_follow_up`] starts on `tids`; one that fails is
    /// logged and the rest still asked.
    fn reports<'t>(
        &self,
        st: &Store,
        bot: &clax_http::bot::Bot,
        tids: impl IntoIterator<Item = &'t String>,
    ) -> Vec<String> {
        let Some(w) = &self.workspace else {
            return Vec::new();
        };
        let mut started = Vec::new();
        for tid in tids {
            match report_follow_up(st, bot, &self.s, w.get(), tid) {
                Ok(run) => started.extend(run),
                Err(e) => {
                    tracing::warn!(error = %e, report = tid, "a report's follow-up did not start");
                }
            }
        }
        started
    }
}

/// The most runs ClaxBot starts on its own on one report since a person
/// last sent one or commented.
const MAX_AUTO_RUNS: usize = 3;

/// Whether a run's end leaves its report to ClaxBot: it finished, or ran out
/// of its time. A person's Stop, a refusal, a failure or a run that gave up
/// stays as it is until a person sends the report again.
fn may_follow(last: &Run) -> bool {
    last.status == clax_store::bot_runs::RunStatus::Finished
        || last.reason.as_deref() == Some("run_cap")
}

/// A fixed comment of ClaxBot's, for `run`'s person, `body`: on the report
/// for everyone, or with `members` for its queue's members alone.
fn post_fixed(
    st: &Store,
    s: &clax_realtime::feedback::FeedbackCtx,
    run: &Run,
    tid: &str,
    body: &str,
    members: bool,
) -> clax_core::Result<()> {
    let ctx = clax_bot::run::bot_ctx(&run.person);
    let c = clax_store::threads::NewComment {
        author_kind: clax_core::model::AUTHOR_AGENT,
        author_name: ServerHost::author(st, run),
        author_public_id: None,
        via_session_id: None,
        body: body.to_string(),
        via_page: false,
    };
    if members {
        st.add_members_comment(&ctx, tid, c)?;
        if let Some(t) = st.get_thread(tid)? {
            crate::routes::threads::publish_thread(s, st, &t);
        }
    } else {
        st.add_comment(&ctx, tid, c)?;
        announce_thread(s, st, tid);
    }
    Ok(())
}

/// What a reporter's word on a fix a run proposed does (`docs/feedback.md`),
/// asked at every change of a report, every run's end and a driver's start,
/// so it answers the same however often it is asked. Off unless
/// `workspace`'s target for the queue offers a change.
///
/// The fix may be a person's or an agent's that took over from a run's held
/// one ([`Store::last_fix_run`]): everything here holds of it the same.
///
/// A reopen (the report open, its last fix rejected) starts a run; a reply
/// to a fix that waits starts one only on a target that opts in
/// ([`clax_bot::workspace::Offer::wake_on_reply`]), and that run may only
/// answer. Neither does while anyone's claim holds the report, a run works
/// on it, or unless the word came after the last run began and that run
/// ended on its own ([`may_follow`]): a person's Stop stays stopped. A target
/// with [`clax_bot::workspace::Offer::no_follow_ups`] starts none at all. At
/// [`MAX_AUTO_RUNS`] since a person last sent a run or commented it stops
/// and tells the queue's members, once. The run acts for the person and
/// agent of the run that proposed the fix, who must still read the queue
/// ([`Store::run_reads_queue`]), may send the report and not be refused: a
/// refused person is passed over quietly, however many events ask. A reply
/// no run will answer to its reporter, a held fix's among them, is
/// acknowledged in fixed words.
/// Returns the run made. The reporter's words are data to it, as to any.
///
/// # Errors
/// The store's.
pub fn report_follow_up(
    st: &Store,
    bot: &clax_http::bot::Bot,
    s: &clax_realtime::feedback::FeedbackCtx,
    workspace: &dyn clax_bot::Workspace,
    tid: &str,
) -> clax_core::Result<Option<String>> {
    use clax_bot::workspace::{AUTO_RUNS_STOPPED, REPLY_RECEIVED};
    use clax_core::CoreError;
    use clax_core::queue::{AUTHOR_REPORTER, State};
    use clax_core::report::FixOutcome;
    use clax_store::bot_runs::WokenBy;
    let Some(qid) = st.get_thread(tid)?.and_then(|t| t.queue_id) else {
        return Ok(None);
    };
    let Some(proposer) = st.last_fix_run(tid)?.map_or(Ok(None), |r| st.run(&r))? else {
        return Ok(None);
    };
    let offer = workspace.offer(&proposer, Some(&qid));
    if !offer.change {
        return Ok(None);
    }
    let Some(report) = st.report(&qid, tid, Visibility::All)? else {
        return Ok(None);
    };
    let Some(fix) = report.fixes.last() else {
        return Ok(None);
    };
    let reopened = report.state == State::Open && fix.outcome == FixOutcome::Rejected;
    let waiting = report.state == State::FixProposed && fix.outcome == FixOutcome::Pending;
    let from_reporter = |c: &&clax_core::queue::ReportComment| c.author_kind == AUTHOR_REPORTER;
    let word = if reopened {
        report.comments.iter().rfind(|c| c.capture.is_some())
    } else if waiting {
        report.comments.iter().rfind(from_reporter)
    } else {
        return Ok(None);
    };
    let Some(word) = word else {
        return Ok(None);
    };
    let last_said = report.comments.iter().rfind(|c| !c.triage_only);
    // A run's words do not reach the reporter of a held fix, so nor does an
    // answer: this says the reply was received.
    let no_answer =
        proposer.confidential || !offer.wake_on_reply || offer.no_follow_ups || fix.held;
    if waiting && no_answer && last_said.is_some_and(|c| from_reporter(&c)) {
        post_fixed(st, s, &proposer, tid, REPLY_RECEIVED, false)?;
    }
    if waiting && !offer.wake_on_reply {
        return Ok(None);
    }
    let runs = st.runs_for_thread(tid, 50)?;
    let Some(last) = runs.first() else {
        return Ok(None);
    };
    if !last.status.is_terminal() || word.created_at <= last.created_at || !may_follow(last) {
        return Ok(None);
    }
    let by_person = report
        .comments
        .iter()
        .rfind(|c| c.author_kind == clax_core::model::AUTHOR_VIEWER)
        .map(|c| c.created_at.as_str());
    let automatic = runs
        .iter()
        .take_while(|r| r.woken_by.is_some() && by_person.is_none_or(|p| r.created_at.as_str() > p))
        .count();
    if automatic >= MAX_AUTO_RUNS {
        let told = report.comments.iter().any(|c| {
            c.triage_only && c.body == AUTO_RUNS_STOPPED && c.created_at > last.created_at
        });
        if !told {
            post_fixed(st, s, &proposer, tid, AUTO_RUNS_STOPPED, true)?;
        }
        return Ok(None);
    }
    let grant = proposer.agent_grant.as_deref();
    if !st.run_reads_queue(&proposer.person, grant, &qid)? {
        return Ok(None);
    }
    let Some(caller) = person_caller(st, &proposer.person)? else {
        return Ok(None);
    };
    let aid = ArtifactId::parse(&proposer.artifact_id)?;
    if !crate::routes::people::may_send_at(st, &caller, &aid)?
        || crate::routes::bot::refusal(st, bot, &proposer.person)?.is_some()
    {
        return Ok(None);
    }
    if offer.no_follow_ups {
        return Ok(None);
    }
    let sender = crate::routes::bot::Sender {
        person: proposer.person.clone(),
        agent_grant: proposer.agent_grant.clone(),
    };
    let by = if reopened {
        WokenBy::Reopen
    } else {
        WokenBy::Reply
    };
    match crate::routes::bot::start_run_woken(
        st,
        bot,
        &clax_bot::run::bot_ctx(&proposer.person),
        &sender,
        &aid,
        RequestKind::Thread,
        vec![tid.to_string()],
        Some(by),
    ) {
        Ok(run) => Ok(Some(run.id)),
        Err(e @ (CoreError::NotFound | CoreError::Invalid { .. })) => {
            tracing::info!(error = %e, report = tid, "a report's follow-up was refused");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Tells the artifact's viewers thread `tid` changed (a ClaxBot reply or
/// resolve), as the thread routes do.
fn announce_thread(ctx: &clax_realtime::feedback::FeedbackCtx, st: &Store, tid: &str) {
    if let Ok(Some(t)) = st.get_thread(tid) {
        if let Some(c) = t.comments.last() {
            ctx.events.publish(clax_core::Event::Comment {
                artifact_id: t.artifact_id.clone(),
                thread_id: t.id.clone(),
                comment: json!(c),
            });
        }
        crate::routes::threads::publish_thread(ctx, st, &t);
    }
}

/// A write's pin from its options: `if_version` when given.
fn pin_of(opts: Option<&Value>) -> clax_store::docs::Pin {
    clax_store::docs::Pin {
        if_version: opts
            .and_then(|o| o.get("if_version"))
            .and_then(Value::as_u64),
        lww: false,
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl BotHost for ServerHost {
    async fn context(&self, run: &Run) -> Result<RequestContext, HostError> {
        let (run, me) = (run.clone(), self.me());
        self.on_store(move |st| {
            let a = me.artifact(st, &run)?;
            let mut names = clax_http::bot::Names::new(st);
            let person_name = names.name(&run.person).map_err(|e| host_error(&e))?;
            let held = st.run(&run.id).map_err(|e| host_error(&e))?;
            let confidential = held.is_some_and(|r| r.confidential);
            let mut threads = Vec::new();
            for tid in &run.thread_ids {
                let t = ServerHost::thread(st, &run, tid)?;
                threads.push(ServerHost::thread_with_notes(st, &run, &t, confidential)?);
            }
            let brief = (run.request == RequestKind::Brief)
                .then(|| {
                    threads
                        .first()
                        .and_then(|t| t.comments.first())
                        .map(|c| c.text.clone())
                })
                .flatten();
            if run.request == RequestKind::Brief {
                threads.clear();
            }
            Ok(RequestContext {
                artifact: ArtifactMeta {
                    url: Some(format!("{}/a/{}", me.state.browser_base, a.id)),
                    id: a.id,
                    title: a.title,
                    description: a.description,
                    version: a.current_version,
                },
                person_name,
                threads,
                brief,
            })
        })
        .await
    }

    async fn read(&self, cx: &OpCtx<'_>, op: &str, args: &[Value]) -> Result<Value, HostError> {
        let me = self.me();
        let (run, audit) = (cx.run.clone(), cx.audit.clone());
        let (step, attempt, seq) = (cx.step, cx.attempt, cx.seq);
        let members_only = cx.members_only;
        let (op, args) = (op.to_string(), args.to_vec());
        let now = self
            .on_store(move |st| {
                let cx = OpCtx::new(&run, &audit, step, attempt, seq).members_only(members_only);
                me.read_now(st, &cx, &op, &args)
            })
            .await?;
        let (id, data_op, kind) = match now {
            ReadNow::Done(v) => return Ok(v),
            ReadNow::Data(id, data_op, kind) => (id, data_op, kind),
        };
        let reply = self
            .state
            .app_data
            .call(&id, *data_op)
            .await
            .map_err(|e| host_error(&e))?;
        let doc =
            |d: clax_store::docs::Doc| json!({"id": d.id, "version": d.version, "data": d.data});
        let got = match kind {
            DataRead::Collections => reply
                .into_collections()
                .map(|cs| json!(cs.into_iter().map(|(c, _)| c).collect::<Vec<_>>())),
            DataRead::Get => reply
                .into_doc()
                .map(|d| d.map_or(Value::Null, |(d, _)| doc(d))),
            DataRead::Page { list } => reply.into_page().map(|p| {
                let docs: Vec<Value> = p.docs.into_iter().map(doc).collect();
                if list {
                    json!({"docs": docs, "next_cursor": p.next_cursor})
                } else {
                    Value::Array(docs)
                }
            }),
        };
        got.map_err(|e| host_error(&e))
    }

    async fn plan_write<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        op: &str,
        args: &[Value],
    ) -> Result<WritePlan<'s>, HostError> {
        let s = |i: usize| {
            args.get(i)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let run = cx.run.clone();
        match op {
            "threads.reply" | "threads.resolve" => {
                let (me, r, tid) = (self.me(), run.clone(), s(0));
                let action = if op == "threads.reply" {
                    "reply"
                } else {
                    "resolve"
                };
                let resolving = op == "threads.resolve";
                self.on_store(move |st| {
                    me.check_write(st, &r, RightType::Comments, action)?;
                    let t = ServerHost::thread(st, &r, &tid)?;
                    if resolving {
                        ServerHost::check_verdict_open(st, &t)?;
                    }
                    Ok(())
                })
                .await?;
                if op == "threads.reply" {
                    return Ok(self.reply_plan(cx, &s(0), &s(1)));
                }
                let (state, ctx, tid) = (self.state.clone(), cx.audit.clone(), s(0));
                Ok(Box::new(move |entry: CallEntry| {
                    Box::pin(async move {
                        let st2 = state.feedback_ctx();
                        state
                            .store
                            .call(move |st| {
                                let guard = |tx: &clax_store::Tx<'_>| {
                                    st.check_bot_write_in(
                                        tx,
                                        &run.person,
                                        run.agent_grant.as_deref(),
                                        Some(&run.artifact_id),
                                        RightType::Comments,
                                        "resolve",
                                    )
                                };
                                let j = st.resolve_thread_journaled(
                                    &ctx,
                                    &tid,
                                    clax_http::bot::ACTOR_ID,
                                    Some(&run.person),
                                    &entry,
                                    &guard,
                                )?;
                                announce_thread(&st2, st, &tid);
                                Ok(j)
                            })
                            .await
                    }) as PlanFuture<'s>
                }))
            }
            "db.set" | "db.update" | "db.delete" => {
                let (me, r) = (self.me(), run.clone());
                self.on_store(move |st| me.check_write(st, &r, RightType::Db, "write"))
                    .await?;
                let (state, ctx, op, args) = (
                    self.state.clone(),
                    cx.audit.clone(),
                    op.to_string(),
                    args.to_vec(),
                );
                Ok(Box::new(move |entry: CallEntry| {
                    Box::pin(async move {
                        // The artifact's own store takes the document, so the
                        // write and its journal entry are two: the journal's
                        // answer or the guard first, the write under the
                        // entry's operation ID, then the guard again and the
                        // entry (`Store::app_write_begin`).
                        let id = ArtifactId::parse(&run.artifact_id)?;
                        let s = |i: usize| {
                            args.get(i)
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        };
                        let path = format!("{}/{}", s(0), s(1));
                        let (r, e, i) = (run.clone(), entry.clone(), id.clone());
                        let hosted = state.hosted.is_some();
                        let begun = state
                            .store
                            .call(move |st| {
                                let caller = db_caller(st, hosted, &r, "write").map_err(|e| {
                                    clax_core::CoreError::invalid("forbidden", e.message)
                                })?;
                                let guard = bot_guard(st, &r);
                                let prev = st.app_write_begin(&e, &guard)?;
                                Ok((prev, caller, st.doc_capabilities(&i)?))
                            })
                            .await?;
                        let (caller, caps) = match begun {
                            (Some(prev), _, _) => return Ok(prev),
                            (None, caller, caps) => (caller, caps),
                        };
                        let w = clax_store::app_data::WriteCtx::of(&ctx).with_op(entry.op_id());
                        let caller = clax_store::app_data::WireCaller::from(&caller);
                        let data_op = match op.as_str() {
                            "db.set" => clax_store::app_data::AppDataOp::Set {
                                ctx: w,
                                caps,
                                path: path.clone(),
                                data: args.get(2).cloned().unwrap_or(Value::Null),
                                pin: pin_of(args.get(3)),
                                caller,
                            },
                            "db.update" => clax_store::app_data::AppDataOp::Update {
                                ctx: w,
                                caps,
                                path: path.clone(),
                                patch: args.get(2).cloned().unwrap_or(Value::Null),
                                pin: pin_of(args.get(3)),
                                caller,
                                view: None,
                            },
                            _ => clax_store::app_data::AppDataOp::Delete {
                                ctx: w,
                                caps,
                                path: path.clone(),
                                pin: pin_of(args.get(2)),
                                caller,
                            },
                        };
                        let written = state.app_data.call(&id, data_op).await?.into_written()?;
                        let events = state.events.clone();
                        let r = run.clone();
                        state
                            .store
                            .call(move |st| {
                                let guard = bot_guard(st, &r);
                                let j = st.journal_app_write(&entry, &guard, written.doc_ref())?;
                                if let Some(c) = written.change {
                                    events.publish(clax_core::Event::Doc {
                                        artifact_id: id.as_str().to_string(),
                                        path: c.path,
                                        version: c.version,
                                        private_to: c.private_to,
                                        read_level: c.read_level,
                                        self_read: c.self_read,
                                    });
                                }
                                Ok(j)
                            })
                            .await
                    }) as PlanFuture<'s>
                }))
            }
            "artifacts.create" => {
                if run.request != RequestKind::Brief {
                    return Err(forbidden(
                        "only a new artifact's brief may create artifacts",
                    ));
                }
                let r = run.clone();
                self.on_store(move |st| {
                    if r.person != OWNER && !ServerHost::allows_create(st, &r)? {
                        return Err(forbidden(
                            "the person this run acts for may not create artifacts",
                        ));
                    }
                    Ok(())
                })
                .await?;
                let spec = args.first().cloned().unwrap_or(Value::Null);
                let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
                    "title": spec["title"],
                    "description": spec["description"],
                    "files": {"index.html": {"encoding": "utf8",
                        "content": clax_core::brief::placeholder_html(
                            spec["title"].as_str().unwrap_or("New artifact"))}}
                }))
                .map_err(|e| HostError::new("invalid_args", e.to_string()))?;
                let mut p = clax_core::publish::validate(req).map_err(|e| host_error(&e))?;
                p.person = (run.person != OWNER).then(|| run.person.clone());
                let (state, ctx) = (self.state.clone(), cx.audit.clone());
                Ok(Box::new(move |entry: CallEntry| {
                    Box::pin(async move {
                        state
                            .store
                            .call_async(move |st| async move {
                                let st2 = st.clone();
                                let (person, grant) = (run.person.clone(), run.agent_grant.clone());
                                let guard = move |tx: &clax_store::Tx<'_>| {
                                    st2.check_bot_write_in(
                                        tx,
                                        &person,
                                        grant.as_deref(),
                                        None,
                                        RightType::Artifacts,
                                        "create",
                                    )
                                };
                                st.create_artifact_journaled(&ctx, p, None, &entry, &guard)
                                    .await
                            })
                            .await
                    }) as PlanFuture<'s>
                }))
            }
            other => Err(HostError::new(
                "unknown_op",
                format!("{other} is not a host write"),
            )),
        }
    }

    async fn plan_publish<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        draft: PublishDraft,
    ) -> Result<WritePlan<'s>, HostError> {
        let (me, run) = (self.me(), cx.run.clone());
        let r = run.clone();
        let current = self
            .on_store(move |st| {
                me.check_write(st, &r, RightType::Artifacts, "update")?;
                let id = ArtifactId::parse(&r.artifact_id).map_err(|e| host_error(&e))?;
                let a = st.get_artifact(&id).map_err(|e| host_error(&e))?;
                let files: Vec<String> = match a {
                    Some(a) => st
                        .get_version(&id, a.current_version)
                        .map_err(|e| host_error(&e))?
                        .map(|v| v.files.keys().cloned().collect())
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                Ok(files)
            })
            .await?;
        // The whole draft: what the current version has beyond it goes.
        let mut files: serde_json::Map<String, Value> = draft
            .files
            .iter()
            .map(|f| {
                use base64::Engine as _;
                let v = match std::str::from_utf8(&f.content) {
                    Ok(t) => json!({"content": t, "encoding": "utf8", "content_type": f.content_type}),
                    Err(_) => json!({"content": base64::engine::general_purpose::STANDARD.encode(&f.content),
                                     "encoding": "base64", "content_type": f.content_type}),
                };
                (f.path.clone(), v)
            })
            .collect();
        for path in current {
            files.entry(path).or_insert(Value::Null);
        }
        let addresses: Vec<String> = draft
            .addresses
            .iter()
            .filter(|t| run.thread_ids.contains(t))
            .cloned()
            .collect();
        let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
            "files": files,
            "note": draft.note,
            "if_version": draft.if_version,
            "addresses": addresses,
        }))
        .map_err(|e| HostError::new("invalid_args", e.to_string()))?;
        let p = clax_core::publish::validate(req).map_err(|e| host_error(&e))?;
        // A clax.json the draft changes or removes declares, and a change of
        // the declaration is its maker's, as a publish's (`admin_change`).
        if let Some(caps) = p.capabilities.clone() {
            let (aid, person) = (run.artifact_id.clone(), run.person.clone());
            let hosted = self.state.hosted.is_some();
            self.on_store(move |st| {
                let id = ArtifactId::parse(&aid).map_err(|e| host_error(&e))?;
                let Some(cur) = st.get_artifact(&id).map_err(|e| host_error(&e))? else {
                    return Ok(());
                };
                if caps == cur.capabilities {
                    return Ok(());
                }
                let administers = if person == OWNER {
                    !hosted || st.artifact_created_by(&aid, None)
                        .map_err(|e| host_error(&e))?
                } else {
                    st.artifact_created_by(&aid, Some(&person))
                        .map_err(|e| host_error(&e))?
                };
                if administers {
                    Ok(())
                } else {
                    Err(HostError::new(
                        "admin_only",
                        "only the page's maker (on a local Clax, the owner too) changes its capabilities: leave clax.json as it is",
                    ))
                }
            })
            .await?;
        }
        let (state, ctx) = (self.state.clone(), cx.audit.clone());
        Ok(Box::new(move |entry: CallEntry| {
            Box::pin(async move {
                let events = state.events.clone();
                let base = state.browser_base.clone();
                state
                    .store
                    .call_async(move |st| async move {
                        let id = ArtifactId::parse(&run.artifact_id)?;
                        let st2 = st.clone();
                        let (person, aid, grant) = (
                            run.person.clone(),
                            run.artifact_id.clone(),
                            run.agent_grant.clone(),
                        );
                        let guard = move |tx: &clax_store::Tx<'_>| {
                            st2.check_bot_write_in(
                                tx,
                                &person,
                                grant.as_deref(),
                                Some(&aid),
                                RightType::Artifacts,
                                "update",
                            )
                        };
                        let j = st
                            .publish_version_journaled(&ctx, &id, p, None, &entry, &guard)
                            .await?;
                        if let Some(a) = st.get_artifact(&id)? {
                            events.publish(clax_core::Event::Version {
                                artifact_id: a.id.clone(),
                                n: a.current_version,
                                by_page: false,
                                title: Some(a.title.clone()),
                                at: Some(a.updated_at.clone()),
                            });
                        }
                        let _ = base;
                        Ok(j)
                    })
                    .await
            }) as PlanFuture<'s>
        }))
    }

    async fn plan_reply<'s>(
        &'s self,
        cx: &OpCtx<'_>,
        thread_id: &str,
        markdown: &str,
    ) -> Result<WritePlan<'s>, HostError> {
        let (me, r, tid) = (self.me(), cx.run.clone(), thread_id.to_string());
        self.on_store(move |st| {
            me.check_write(st, &r, RightType::Comments, "reply")?;
            ServerHost::thread(st, &r, &tid).map(|_| ())
        })
        .await?;
        Ok(self.reply_plan(cx, thread_id, markdown))
    }

    async fn start_waiting(&self) -> clax_core::Result<usize> {
        self.listen_to_reports();
        let bot = self.state.bot.clone();
        let follow = self.follow();
        let started = self
            .state
            .store
            .call(move |st| {
                let mut started = crate::routes::bot::drain_follow_ups(st, &bot)?;
                // A reporter's word a crash left between its commit and its
                // event.
                if follow.on() {
                    started.extend(follow.reports(st, &bot, &st.reports_of_run_fixes()?));
                }
                Ok(started)
            })
            .await?;
        if let Some(w) = self.wake.get() {
            for run in &started {
                w.started(run);
            }
        }
        Ok(started.len())
    }

    async fn check_fix(&self, cx: &OpCtx<'_>) -> Result<(), HostError> {
        use clax_core::CoreError;
        let (run, key) = (cx.run.clone(), cx.key.clone());
        self.state
            .store
            .call(move |st| {
                let t = report_thread(st, &run)?;
                let qid = t.queue_id.clone().ok_or(CoreError::NotFound)?;
                if st.fix_proposed_under(&t.id, &key)? {
                    return Ok(());
                }
                st.check_run_may_propose(&qid, &t.id)
            })
            .await
            .map_err(|e| match e {
                CoreError::ThreadTaken { name } => {
                    HostError::new("thread_taken", format!("{name} holds this report"))
                }
                CoreError::NotClaimable => HostError::new("not_claimable", "this report is closed"),
                other => host_error(&other),
            })
    }

    async fn record_fix(
        &self,
        cx: &OpCtx<'_>,
        fix: &clax_bot::host::ProposedFix,
    ) -> Result<(), HostError> {
        let (run, ctx, fix) = (cx.run.clone(), cx.audit.clone(), fix.clone());
        let an = crate::routes::queue_agent::Announcer::of(&self.state);
        self.state
            .store
            .call(move |st| {
                let t = report_thread(st, &run)?;
                let qid = t.queue_id.clone().ok_or(clax_core::CoreError::NotFound)?;
                let (key, note, held) = (fix.key.clone(), fix.members_note.clone(), fix.held);
                let preview_url = fix.preview_url.clone();
                // Before the proposal, whose notice reads them.
                st.keep_fix_links(
                    &t.id,
                    &key,
                    &clax_store::queue_notifiers::FixLinks {
                        url: fix.pull_request,
                        commit: fix.pushed_commit,
                        preview_step: fix.preview_step,
                    },
                )?;
                let new = clax_core::report::NewFix {
                    summary: fix.summary,
                    url: fix.url,
                    commit: fix.commit,
                    release: None,
                    verify_url: fix.verify_url,
                    verify_after: None,
                };
                let by = ReportWriter::Run {
                    run: run.id.clone(),
                    name: clax_http::bot::NAME.to_string(),
                    key,
                    note,
                    held,
                    preview_url,
                };
                // Made once under its key; announced however often it is
                // asked, so a step executed again makes up for a crash.
                let report = st.propose_fix(&ctx, &qid, &t.id, &by, new)?;
                let last = report
                    .comments
                    .iter()
                    .rfind(|c| !c.triage_only)
                    .map(|c| c.id.clone());
                if let Err(e) =
                    crate::routes::queue_agent::announce_comment(&an, st, &t.id, last.as_deref())
                {
                    tracing::warn!(error = ?e, "announcing a run's fix failed");
                }
                Ok(())
            })
            .await
            .map_err(|e| host_error(&e))
    }

    async fn preview_ready(
        &self,
        run: &Run,
        audit: &clax_core::audit::AuditCtx,
        verify_url: &str,
        for_reporter: bool,
    ) -> Result<(), HostError> {
        let (run, ctx, url) = (run.clone(), audit.clone(), verify_url.to_string());
        let body = format!("{} {verify_url}", clax_bot::workspace::PREVIEW_READY);
        let s = self.state.feedback_ctx();
        self.state
            .store
            .call(move |st| {
                let t = report_thread(st, &run)?;
                let qid = t.queue_id.clone().ok_or(clax_core::CoreError::NotFound)?;
                // Each write finds the run's own fix: a later fix has its own
                // preview to wait for, and a person's is theirs.
                if for_reporter {
                    if st.set_fix_verify_url(&ctx, &qid, &t.id, &run.id, &url)? {
                        post_fixed(st, &s, &run, &t.id, &body, false)?;
                    }
                } else if st.set_fix_preview(&t.id, &run.id, &url)? {
                    post_fixed(st, &s, &run, &t.id, &body, true)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| host_error(&e))
    }

    async fn preview_missed(
        &self,
        run: &Run,
        _audit: &clax_core::audit::AuditCtx,
    ) -> Result<(), HostError> {
        let (run, workspace) = (run.clone(), self.workspace.clone());
        let s = self.state.feedback_ctx();
        self.state
            .store
            .call(move |st| {
                let t = report_thread(st, &run)?;
                if st.set_fix_preview_missed(&t.id, &run.id)? {
                    let offer = workspace.as_ref().map_or_else(Default::default, |w| {
                        w.get().offer(&run, t.queue_id.as_deref())
                    });
                    let commit = st.run_fix_commit(&t.id, &run.id)?;
                    let body = offer.preview_missed(commit.as_deref());
                    post_fixed(st, &s, &run, &t.id, &body, true)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| host_error(&e))
    }

    async fn note_members(
        &self,
        run: &Run,
        _audit: &clax_core::audit::AuditCtx,
        body: &str,
    ) -> Result<(), HostError> {
        let (run, body) = (run.clone(), body.to_string());
        let s = self.state.feedback_ctx();
        self.state
            .store
            .call(move |st| {
                let t = report_thread(st, &run)?;
                let qid = t.queue_id.clone().ok_or(clax_core::CoreError::NotFound)?;
                let told = st
                    .report(&qid, &t.id, Visibility::All)?
                    .is_some_and(|r| r.comments.iter().any(|c| c.triage_only && c.body == body));
                if !told {
                    post_fixed(st, &s, &run, &t.id, &body, true)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| host_error(&e))
    }

    async fn working(&self, run: &Run, on: bool) {
        show_working(&self.state, run, on);
        if on {
            return;
        }
        // The run ended (or stopped): the people queued on its threads
        // meanwhile start their follow-ups, one thread's at a time; then
        // the reporter who wrote while it worked.
        let (bot, id, wake) = (self.state.bot.clone(), run.id.clone(), self.wake.clone());
        let follow = self.follow();
        let started = self
            .state
            .store
            .call(move |st| {
                let Some(r) = st.run(&id)? else {
                    return Ok(Vec::new());
                };
                let mut started = crate::routes::bot::after_run(st, &bot, &r)?;
                if r.status.is_terminal() {
                    started.extend(follow.reports(st, &bot, &r.thread_ids));
                }
                Ok(started)
            })
            .await;
        match started {
            Ok(ids) => {
                if let Some(w) = wake.get() {
                    for rid in &ids {
                        w.started(rid);
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "a run's follow-up did not start"),
        }
    }
}

/// The working record a run shows on its threads, by session key
/// `claxbot:<run>` (no harness session has one).
fn working_actor(run: &Run) -> clax_core::working::Actor {
    clax_core::working::Actor {
        session_id: format!("claxbot:{}", run.id),
        harness: clax_http::bot::HANDLE.into(),
        agent: clax_http::bot::HANDLE.into(),
        name: Some(clax_http::bot::NAME.into()),
    }
}

/// Shows ClaxBot working on run `run`'s threads (`on`, renewing the
/// record each step), or stops showing it, and announces the change to the
/// artifact's viewers. The records are not journaled: the run's own
/// `bot.*` events say what it did.
pub fn show_working(state: &AppState, run: &Run, on: bool) {
    let who = working_actor(run);
    let changed = if on {
        state
            .working
            .set(
                &who,
                &run.artifact_id,
                clax_core::working::SetWorking {
                    thread_ids: Some(run.thread_ids.clone()),
                    message: Some("ClaxBot is working on this".into()),
                },
            )
            .1
    } else {
        state
            .working
            .end_session(&who.session_id, clax_core::working::End::Done)
            .0
    };
    clax_realtime::working::announce(&state.events, &state.working, &changed);
}

/// The routes' seam to a driver ([`clax_http::bot::Runs`]). On wasm32 the
/// driver is not `Send` (a Worker has one thread), so it is held in a
/// `SendWrapper` there.
#[cfg(not(target_arch = "wasm32"))]
pub struct DriverRuns(pub Arc<clax_bot::Driver>);
/// The routes' seam to a driver ([`clax_http::bot::Runs`]), on the Worker's
/// one thread.
#[cfg(target_arch = "wasm32")]
pub struct DriverRuns(pub send_wrapper::SendWrapper<Arc<clax_bot::Driver>>);

impl DriverRuns {
    /// The seam to `driver`.
    pub fn of(driver: Arc<clax_bot::Driver>) -> DriverRuns {
        #[cfg(not(target_arch = "wasm32"))]
        {
            DriverRuns(driver)
        }
        #[cfg(target_arch = "wasm32")]
        {
            DriverRuns(send_wrapper::SendWrapper::new(driver))
        }
    }
}

impl clax_http::bot::Runs for DriverRuns {
    fn started(&self, run: &str) {
        self.0.wake(run);
    }

    fn cancelled(&self, run: &str) {
        self.0.cancel(run);
    }
}

/// The driver of a daemon on `state` (native): its deps over the server's
/// host, this build's sandbox, the state's settings and keys, the store's
/// clock and the daemon's web source ([`crate::web_source::for_daemon`],
/// its fetches naming a hosted daemon's public origin, a local one's
/// nothing, and never reaching a hosted daemon's own hosts); the routes'
/// events hub.
/// Installs it as the state's run seam and returns it, not yet started.
#[cfg(not(target_arch = "wasm32"))]
pub fn driver(state: &mut AppState) -> Arc<clax_bot::Driver> {
    driver_for_home(state, None)
}

/// The workspace of the home at `home`, if it has one, its store links with
/// it where there is a key to open them, and what it is for the routes to
/// say ([`clax_http::bot::RuntimeFacts`]). One that is set up and failed runs
/// nothing and says so.
#[cfg(not(target_arch = "wasm32"))]
fn workspace_of(
    state: &AppState,
    home: Option<&std::path::Path>,
) -> (
    clax_http::bot::RuntimeFacts,
    Option<Arc<dyn clax_bot::Workspace>>,
) {
    use clax_bot_workspace::config::{Links, for_home};
    use clax_http::bot::RuntimeFacts;
    use clax_store::queue_repos::Egress;
    let links = state.bot.kek.clone().map(|kek| Links {
        store: state.store.clone(),
        kek,
    });
    match home.map_or(Ok(None), |h| for_home(h, links)) {
        Ok(Some(w)) => {
            let facts = RuntimeFacts {
                configured: true,
                third_party: w.runtime.third_party(),
                egress_held: w.egress() == Egress::Held,
                failed: false,
                operator_queues: w.operator_queues(),
                operator_only: false,
            };
            (facts, Some(Arc::new(w)))
        }
        Ok(None) => (RuntimeFacts::default(), None),
        Err(e) => {
            tracing::warn!(
                error = format!("{e:#}"),
                "ClaxBot has no container: workspace.toml"
            );
            let facts = RuntimeFacts {
                configured: true,
                third_party: true,
                failed: true,
                ..RuntimeFacts::default()
            };
            (facts, None)
        }
    }
}

/// [`driver`], with the workspace of the home at `home`, if it has one. The
/// one place a daemon's workspace is wired: its file, its store links, and
/// what the routes say of it.
#[cfg(not(target_arch = "wasm32"))]
pub fn driver_for_home(
    state: &mut AppState,
    home: Option<&std::path::Path>,
) -> Arc<clax_bot::Driver> {
    let hosted = state.hosted.as_ref();
    let (web, unavailable) = crate::web_source::daemon_web(
        state.store.clone(),
        state.bot.kek.clone(),
        hosted.map(|h| h.origin.as_str()),
        hosted.and_then(|h| h.sites.as_ref()).map(|s| s.domain()),
    );
    if let Some(why) = unavailable {
        state
            .bot
            .web_check
            .set(clax_bot::web::WebCheck::Unusable(why));
    }
    let (facts, workspace) = workspace_of(state, home);
    let _ = state.bot.runtime_facts.set(facts);
    driver_with(state, web, workspace)
}

/// [`driver`], with `web` as the runs' web source.
#[cfg(not(target_arch = "wasm32"))]
pub fn driver_with_web(
    state: &mut AppState,
    web: Arc<dyn clax_bot::web::WebSource>,
) -> Arc<clax_bot::Driver> {
    driver_with(state, web, None)
}

/// [`driver_with_web`], with `workspace` as the runs' container machinery.
#[cfg(not(target_arch = "wasm32"))]
pub fn driver_with(
    state: &mut AppState,
    web: Arc<dyn clax_bot::web::WebSource>,
    workspace: Option<Arc<dyn clax_bot::Workspace>>,
) -> Arc<clax_bot::Driver> {
    let bot = state.bot.clone();
    let mut host = ServerHost::new(state.clone());
    host.workspace = workspace.clone().map(HeldWorkspace::of);
    let wake = host.wake.clone();
    let host: Arc<dyn BotHost> = Arc::new(host);
    let deps = clax_bot::Deps::new(
        state.store.clone(),
        crate::routes::admin::messages_client(&state.store),
        sandbox(),
        host.clone(),
        Arc::new(clax_http::bot::Settings {
            store: state.store.clone(),
            enabled: bot.enabled,
        }),
        bot.keys(),
        Arc::new(StoreClock(state.store.clone())),
    )
    .with_web(web);
    let deps = match workspace {
        Some(w) => deps.with_workspace(w),
        None => deps,
    };
    let driver = Arc::new(clax_bot::Driver::new(
        clax_store::bot_runs::DriverId::mint(),
        Arc::new(deps),
        bot.events.clone(),
    ));
    let runs: Arc<dyn clax_http::bot::Runs> = Arc::new(DriverRuns::of(driver.clone()));
    let _ = wake.set(runs.clone());
    state.bot = Arc::new(clax_http::bot::Bot {
        kek: bot.kek.clone(),
        env_key: bot.env_key.as_ref().map(|k| clax_http::bot::EnvKey {
            var: k.var.clone(),
            key: clax_core::secret::Secret::new(k.key.expose().to_string()),
        }),
        enabled: bot.enabled,
        views: bot.views.clone(),
        runs,
        events: bot.events.clone(),
        catalog_url: bot.catalog_url.clone(),
        catalog_flight: bot.catalog_flight.clone(),
        // The runs have a source now: the daemon's own fetches at least,
        // unless the host said its source does more (the test fixtures).
        web_configured: if bot.web_configured.any() {
            bot.web_configured
        } else {
            clax_bot::web::WebShape {
                search: false,
                fetch: true,
            }
        },
        web_check: bot.web_check.clone(),
        admin_web_check: bot.admin_web_check.clone(),
        runtime_facts: bot.runtime_facts.clone(),
    });
    driver
}

/// Which of ClaxBot's web operations are configured on this host now:
/// neither once the host knows its source cannot be used (its origin or
/// site domain names no host, or the source cannot run where it runs:
/// [`clax_http::bot::Bot::web_check`]); else those the host's source
/// performs ([`clax_http::bot::Bot::web_configured`]: both for the
/// Worker's AI binding and the daemon's test fixtures, fetch for the
/// daemon's own source), and, for a native host whose source fetches but
/// does not search on its own (the daemon's), search once the instance
/// holds a key-encryption key, a Cloudflare account ID and a sealed token
/// ([`crate::web_source::has_credentials`]), read at each call so an
/// admin's change takes effect without a restart.
///
/// # Errors
/// The store's.
pub fn web_configured(
    st: &Store,
    bot: &clax_http::bot::Bot,
) -> clax_core::Result<clax_bot::web::WebShape> {
    use clax_bot::web::WebShape;
    if bot.web_check.why().is_some() {
        return Ok(WebShape::default());
    }
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut shape = bot.web_configured;
    // Every decision on web use (a run's start, the person's and the
    // admin's views) comes here: with web use on, the host is asked to
    // check its source, which it does after the request (the Worker; the
    // daemon checks nothing).
    if shape.any() && web_on(st)? {
        bot.web_check.want();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if shape.fetch && !shape.search {
        shape.search = crate::web_source::has_credentials(st, bot.kek.as_ref())?;
    }
    Ok(shape)
}

/// Whether an admin turned web search or web fetch on.
fn web_on(st: &Store) -> clax_core::Result<bool> {
    use clax_store::bot_settings::Setting;
    let on =
        |s: Setting| -> clax_core::Result<bool> { Ok(st.bot_setting(s)?.as_bool() == Some(true)) };
    Ok(on(Setting::WebSearchEnabled)? || on(Setting::WebFetchEnabled)?)
}

/// A clock reading the store's, owned.
pub struct StoreClock(pub Arc<Store>);

impl clax_core::working::Clock for StoreClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.now_utc()
    }
}

/// Starts `driver` on a daemon: takes over the runs a previous process
/// left (a native daemon is its home's only driver) and drives runs until
/// the process ends. Returns the driving task.
#[cfg(not(target_arch = "wasm32"))]
pub fn start_driver(driver: &Arc<clax_bot::Driver>) -> clax_rt::JoinHandle<()> {
    // Its start (the orphan sweep, then the runs a previous process left)
    // wakes the driving loop for each run it queues.
    let d = driver.clone();
    clax_rt::spawn(async move {
        match d.start(true).await {
            Ok(n) if n > 0 => tracing::info!(runs = n, "resuming ClaxBot runs"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "ClaxBot runs could not be resumed"),
        }
    });
    driver.clone().spawn_forever()
}
