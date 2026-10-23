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
//! the run's threads.

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
use clax_store::bot_runs::CallEntry;
use clax_store::bot_runs::{RequestKind, Run};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::Arc;

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

/// The document caller a run reads (`action` `read`) or writes (`write`)
/// as: the owner's own run at the owner's level; otherwise its person,
/// at the `clax-db` level the run's rights ([`Store::bot_rights`]: its
/// person's, narrowed by the asking agent's grant) give that action at its
/// artifact (`view` where none does), the owner's agent as the owner's
/// row.
fn db_caller(st: &Store, run: &Run, action: &str) -> Result<clax_core::db::Caller, HostError> {
    use clax_core::visibility::Author;
    let rights = st
        .bot_rights(&run.person, run.agent_grant.as_deref())
        .map_err(|e| host_error(&e))?;
    let Some(rights) = rights else {
        return Ok(clax_core::db::Caller::new(
            clax_core::db::Level::Owner,
            None,
        ));
    };
    let owner = run.person == OWNER;
    let maker = (!owner).then_some(run.person.as_str());
    let own = |x: &str| st.artifact_created_by(x, maker).unwrap_or(false);
    let level = rights
        .db_level(&run.artifact_id, action, &own)
        .unwrap_or(clax_core::db::Level::View);
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
}

fn forbidden(what: impl Into<String>) -> HostError {
    HostError::new("forbidden", what)
}

impl ServerHost {
    /// The host over `state`.
    pub fn new(state: AppState) -> ServerHost {
        ServerHost {
            state,
            wake: Arc::default(),
        }
    }

    /// Another handle on this host.
    /// Hands the follow-ups a run's end starts to `runs` (the daemon's
    /// [`driver`] does it for its own; the Worker for its alarm's). The
    /// first call wins.
    pub fn set_runs(&self, runs: Arc<dyn clax_http::bot::Runs>) {
        let _ = self.wake.set(runs);
    }

    fn me(&self) -> ServerHost {
        ServerHost {
            state: self.state.clone(),
            wake: self.wake.clone(),
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
            || !Self::allows(st, run, RightType::Artifacts, "read")?
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
    ) -> Result<Value, HostError> {
        let run = cx.run;
        let a = self.artifact(st, run)?;
        let id = ArtifactId::parse(&a.id).map_err(|e| host_error(&e))?;
        let s = |i: usize| {
            args.get(i)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let dbc = db_caller(st, run, "read")?;
        let doc =
            |d: clax_store::docs::Doc| json!({"id": d.id, "version": d.version, "data": d.data});
        match op {
            "artifact.get" => Ok(
                json!({"id": a.id, "title": a.title, "description": a.description,
                                       "version": a.current_version}),
            ),
            "db.collections" => st
                .readable_doc_collections(&id, &dbc)
                .map(|c| json!(c))
                .map_err(|e| host_error(&e)),
            "db.get" => st
                .doc_get(&id, &format!("{}/{}", s(0), s(1)), &dbc)
                .map(|d| d.map_or(Value::Null, doc))
                .map_err(|e| host_error(&e)),
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
                let (docs, next) = st.doc_query(&id, &q, &dbc).map_err(|e| host_error(&e))?;
                let docs: Vec<Value> = docs.into_iter().map(doc).collect();
                Ok(if op == "db.list" {
                    json!({"docs": docs, "next_cursor": next})
                } else {
                    Value::Array(docs)
                })
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
                Ok(Value::Array(out))
            }
            "threads.read" => {
                let t = Self::thread(st, run, &s(0))?;
                let m = Self::thread_meta(st, run, &t);
                Ok(
                    json!({"id": m.id, "resolved": m.resolved, "anchor": m.anchor,
                          "comments": m.comments}),
                )
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
    /// own.
    fn allows_at(
        st: &Store,
        run: &Run,
        kind: RightType,
        action: &str,
        at: Option<&str>,
    ) -> Result<bool, HostError> {
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
    fn allows(st: &Store, run: &Run, kind: RightType, action: &str) -> Result<bool, HostError> {
        Self::allows_at(st, run, kind, action, Some(&run.artifact_id))
    }

    /// Whether the run may create artifacts.
    fn allows_create(st: &Store, run: &Run) -> Result<bool, HostError> {
        Self::allows_at(st, run, RightType::Artifacts, "create", None)
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
        if !Self::allows(st, run, kind, action)? {
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
    /// as agent ClaxBot for its person, announced as any comment is.
    fn reply_plan<'s>(&'s self, cx: &OpCtx<'_>, tid: &str, markdown: &str) -> WritePlan<'s> {
        let (state, ctx, run, tid, body) = (
            self.state.clone(),
            cx.audit.clone(),
            cx.run.clone(),
            tid.to_string(),
            markdown.to_string(),
        );
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
                        let j = st.add_comment_journaled(&ctx, &tid, c, &entry, &guard)?;
                        announce_thread(&s, st, &tid);
                        Ok(j)
                    })
                    .await
            }) as PlanFuture<'s>
        })
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
        if let Err(e) = crate::routes::threads::publish_thread(ctx, st, &t) {
            tracing::warn!(error = %e, "a ClaxBot thread change was not announced");
        }
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
            let mut threads = Vec::new();
            for tid in &run.thread_ids {
                let t = ServerHost::thread(st, &run, tid)?;
                threads.push(ServerHost::thread_meta(st, &run, &t));
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
        let (op, args) = (op.to_string(), args.to_vec());
        self.on_store(move |st| {
            let cx = OpCtx::new(&run, &audit, step, attempt, seq);
            me.read_now(st, &cx, &op, &args)
        })
        .await
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
                self.on_store(move |st| {
                    me.check_write(st, &r, RightType::Comments, action)?;
                    ServerHost::thread(st, &r, &tid).map(|_| ())
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
                        let events = state.events.clone();
                        state
                            .store
                            .call(move |st| {
                                let id = ArtifactId::parse(&run.artifact_id)?;
                                let caller = db_caller(st, &run, "write").map_err(|e| {
                                    clax_core::CoreError::invalid("forbidden", e.message)
                                })?;
                                let s = |i: usize| {
                                    args.get(i)
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string()
                                };
                                let path = format!("{}/{}", s(0), s(1));
                                let guard = |tx: &clax_store::Tx<'_>| {
                                    st.check_bot_write_in(
                                        tx,
                                        &run.person,
                                        run.agent_grant.as_deref(),
                                        Some(&run.artifact_id),
                                        RightType::Db,
                                        "write",
                                    )
                                };
                                let j = match op.as_str() {
                                    "db.set" => st.doc_set_journaled(
                                        &ctx,
                                        &id,
                                        &path,
                                        args.get(2).cloned().unwrap_or(Value::Null),
                                        pin_of(args.get(3)),
                                        &caller,
                                        &entry,
                                        &guard,
                                    )?,
                                    "db.update" => st.doc_update_journaled(
                                        &ctx,
                                        &id,
                                        &path,
                                        args.get(2).cloned().unwrap_or(Value::Null),
                                        pin_of(args.get(3)),
                                        &caller,
                                        &entry,
                                        &guard,
                                    )?,
                                    _ => st.doc_delete_journaled(
                                        &ctx,
                                        &id,
                                        &path,
                                        pin_of(args.get(2)),
                                        &caller,
                                        &entry,
                                        &guard,
                                    )?,
                                };
                                // Announced to whoever reads at the owner's
                                // level; other pages read it at their next fetch.
                                events.publish(clax_core::Event::Doc {
                                    artifact_id: id.as_str().to_string(),
                                    path,
                                    version: j.value()["version"].as_u64(),
                                    private_to: None,
                                    read_level: clax_core::db::Level::Owner,
                                    self_read: None,
                                });
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
        let bot = self.state.bot.clone();
        let started = self
            .state
            .store
            .call(move |st| crate::routes::bot::drain_follow_ups(st, &bot))
            .await?;
        Ok(started.len())
    }

    async fn working(&self, run: &Run, on: bool) {
        show_working(&self.state, run, on);
        if on {
            return;
        }
        // The run ended (or stopped): the people queued on its threads
        // meanwhile start their follow-ups, one thread's at a time.
        let (bot, id, wake) = (self.state.bot.clone(), run.id.clone(), self.wake.clone());
        let started = self
            .state
            .store
            .call(move |st| match st.run(&id)? {
                Some(r) => crate::routes::bot::after_run(st, &bot, &r),
                None => Ok(Vec::new()),
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
/// host, this build's sandbox, the state's settings and keys, and the
/// store's clock; the routes' events hub. Installs it as the state's run
/// seam and returns it, not yet started.
#[cfg(not(target_arch = "wasm32"))]
pub fn driver(state: &mut AppState) -> Arc<clax_bot::Driver> {
    let bot = state.bot.clone();
    let host = ServerHost::new(state.clone());
    let wake = host.wake.clone();
    let host: Arc<dyn BotHost> = Arc::new(host);
    let deps = clax_bot::Deps::new(
        state.store.clone(),
        clax_sample::messages::MessagesClient::new(),
        sandbox(),
        host.clone(),
        Arc::new(clax_http::bot::Settings {
            store: state.store.clone(),
            enabled: bot.enabled,
        }),
        bot.keys(),
        Arc::new(StoreClock(state.store.clone())),
    );
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
    });
    driver
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
