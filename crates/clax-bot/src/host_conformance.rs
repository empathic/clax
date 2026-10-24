//! The cases every [`BotHost`] implementation passes: what it must refuse
//! even when the run loop's own checks are bypassed (the loop checks
//! arguments and limits threads first; the host enforces rights again),
//! how a journaled write answers a repeated key, that a crash between any
//! write and its journal entry leaves neither (so a resumed step writes
//! once), and that a person who lost the right to write between a plan and
//! its run writes nothing.
//!
//! An implementation's tests build a [`HostFixture`] over its store and
//! call [`check_host`]:
//!
//! ```ignore
//! for (name, result) in clax_bot::host_conformance::check_host(&fixture).await {
//!     result.unwrap_or_else(|e| panic!("{name}: {e}"));
//! }
//! ```

use crate::host::{BotHost, OpCtx, PublishDraft, PublishFile, WritePlan, args_hash, run_plan};
use clax_store::Store;
use clax_store::bot_runs::{CallEntry, DriverId, Run};
use serde_json::{Value, json};
use std::sync::Arc;

/// Revokes a person's right to write on the fixture's artifact.
pub type Revoke = Box<dyn Fn(&str) + Send + Sync>;

/// What the cases run against.
pub struct HostFixture {
    pub host: Arc<dyn BotHost>,
    /// The store the host journals in.
    pub store: Arc<Store>,
    /// A thread request run whose person may write on its artifact; its
    /// first thread is a store thread the cases reply on and resolve.
    pub run: Run,
    /// A brief run on the same artifact whose person may write (it may
    /// create artifacts).
    pub brief_run: Run,
    /// A thread of `run`'s artifact that is not one of `run`'s.
    pub thread_outside_run: String,
    /// A thread of another artifact.
    pub foreign_thread: String,
    /// A thread request run on `run`'s artifact acting for a person who
    /// may read it but not write, `None` when the host has no such person.
    pub reader_run: Option<Run>,
    /// A thread request run on `run`'s artifact, with one thread, whose
    /// person [`HostFixture::revoke`] makes unable to write.
    pub revoked_run: Run,
    pub revoke: Revoke,
}

/// The step the cases' calls are journaled under (far from a run's own).
const STEP: i64 = 900_000;

/// Each case's driver, holding every fixture run, so it may make journal
/// entries for them.
struct Cases<'a> {
    f: &'a HostFixture,
    driver: DriverId,
}

impl Cases<'_> {
    fn entry(&self, run: &Run, seq: i64, op: &str, args: &Value) -> Result<CallEntry, String> {
        self.f
            .store
            .call_entry(&run.id, &self.driver, STEP, 1, seq, op, &args_hash(args))
            .map_err(|e| e.to_string())
    }

    async fn refused_read(&self, run: &Run, seq: i64, op: &str, args: Value) -> Result<(), String> {
        let audit = crate::run::bot_ctx(&run.person);
        let cx = OpCtx::new(run, &audit, STEP, 1, seq);
        let args = args.as_array().cloned().unwrap_or_default();
        match self.f.host.read(&cx, op, &args).await {
            Ok(v) => Err(format!("{op} answered {v} instead of refusing")),
            Err(_) => Ok(()),
        }
    }

    async fn read(&self, run: &Run, op: &str, args: Value) -> Result<Value, String> {
        let audit = crate::run::bot_ctx(&run.person);
        let cx = OpCtx::new(run, &audit, STEP, 1, 999);
        let args = args.as_array().cloned().unwrap_or_default();
        self.f.host.read(&cx, op, &args).await.map_err(|e| e.code)
    }

    async fn plan<'s>(
        &'s self,
        run: &Run,
        seq: i64,
        op: &str,
        args: &Value,
    ) -> Result<WritePlan<'s>, String> {
        let audit = crate::run::bot_ctx(&run.person);
        let cx = OpCtx::new(run, &audit, STEP, 1, seq);
        let list = args.as_array().cloned().unwrap_or_default();
        if op == "draft.publish" {
            return self
                .f
                .host
                .plan_publish(&cx, publish_draft(&list))
                .await
                .map_err(|e| e.code);
        }
        self.f
            .host
            .plan_write(&cx, op, &list)
            .await
            .map_err(|e| e.code)
    }

    async fn write(&self, run: &Run, seq: i64, op: &str, args: Value) -> Result<Value, String> {
        let plan = self.plan(run, seq, op, &args).await?;
        run_plan(plan, self.entry(run, seq, op, &args)?)
            .await
            .map_err(|e| e.code)
    }

    /// Write `op` at `seq` crashes between the write and its journal
    /// entry, then runs again twice: `state` (what the write changes) must
    /// be as before after the crash, and change once.
    async fn crash_once<F, Fut>(
        &self,
        run: &Run,
        seq: i64,
        op: &str,
        args: Value,
        state: F,
    ) -> Result<(), String>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Value, String>>,
    {
        self.crash_then(run, seq, op, args, state, false).await
    }

    /// [`Cases::crash_once`] for a write that lands in the artifact's own
    /// store (a document's), which no instance transaction holds: the write
    /// may stand after the crash, and then the call made again must answer
    /// without writing it again, so it is applied exactly once.
    async fn crash_stands<F, Fut>(
        &self,
        run: &Run,
        seq: i64,
        op: &str,
        args: Value,
        state: F,
    ) -> Result<(), String>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Value, String>>,
    {
        self.crash_then(run, seq, op, args, state, true).await
    }

    async fn crash_then<F, Fut>(
        &self,
        run: &Run,
        seq: i64,
        op: &str,
        args: Value,
        state: F,
        stands: bool,
    ) -> Result<(), String>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Value, String>>,
    {
        let before = state().await?;
        let (step, at) = (STEP, seq);
        self.f
            .store
            .set_journal_fault(Some(Arc::new(move |e: &CallEntry| {
                e.step_n() == step && e.seq() == at
            })));
        let crashed = self.write(run, seq, op, args.clone()).await;
        self.f.store.set_journal_fault(None);
        if crashed.is_ok() {
            return Err(format!("{op}: the injected crash did not fail the write"));
        }
        let crashed = state().await?;
        if !stands && crashed != before {
            return Err(format!("{op}: the write stood without its journal entry"));
        }
        let first = self
            .write(run, seq, op, args.clone())
            .await
            .map_err(|e| format!("{op}: the resumed write failed: {e}"))?;
        let after = state().await?;
        if after == before {
            return Err(format!("{op}: the resumed write changed nothing"));
        }
        if crashed != before && after != crashed {
            return Err(format!(
                "{op}: the write stood after the crash, and the resumed call wrote it again"
            ));
        }
        let again = self
            .write(run, seq, op, args)
            .await
            .map_err(|e| format!("{op}: the replayed write failed: {e}"))?;
        if again != first {
            return Err(format!(
                "{op}: the replay answered {again}, the write {first}"
            ));
        }
        if state().await? != after {
            return Err(format!("{op}: the replay wrote again"));
        }
        Ok(())
    }
}

/// A one-page draft for `draft.publish` from the arguments `[version]`.
fn publish_draft(args: &[Value]) -> PublishDraft {
    PublishDraft {
        files: vec![PublishFile {
            path: "index.html".into(),
            content: b"<!doctype html><title>Conformance</title><p>published</p>".to_vec(),
            content_type: "text/html".into(),
        }],
        if_version: args
            .first()
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0),
        note: Some("conformance".into()),
        addresses: Vec::new(),
    }
}

fn refused(op: &str, r: Result<Value, String>) -> Result<(), String> {
    match r {
        Ok(v) => Err(format!("{op} answered {v} instead of refusing")),
        Err(_) => Ok(()),
    }
}

/// How many comments thread `tid` has whose text is `body`.
fn comments_saying(f: &HostFixture, tid: &str, body: &str) -> Result<usize, String> {
    let t = f
        .store
        .get_thread(tid)
        .map_err(|e| e.to_string())?
        .ok_or("the run's thread is gone")?;
    Ok(t.comments.iter().filter(|c| c.body == body).count())
}

/// Runs every case against `f`; each case's result by name.
pub async fn check_host(f: &HostFixture) -> Vec<(&'static str, Result<(), String>)> {
    let c = Cases {
        f,
        driver: DriverId::mint(),
    };
    for r in [&f.run, &f.brief_run, &f.revoked_run]
        .into_iter()
        .chain(f.reader_run.as_ref())
    {
        if let Err(e) = f.store.claim_run(&r.id, &c.driver, 0) {
            return vec![("the fixture runs can be claimed", Err(e.to_string()))];
        }
    }
    let run = &f.run;
    let inside = run.thread_ids.first().cloned().unwrap_or_default();
    let mut out: Vec<(&'static str, Result<(), String>)> = Vec::new();
    out.push((
        "a_draft_operation_is_not_the_hosts",
        async {
            c.refused_read(run, 0, "files.read", json!(["index.html"]))
                .await?;
            refused(
                "files.write",
                c.write(run, 1, "files.write", json!(["a.html", "x"])).await,
            )
        }
        .await,
    ));
    out.push((
        "an_unknown_operation_is_refused",
        async {
            c.refused_read(run, 2, "artifacts.list", json!([null]))
                .await?;
            refused(
                "shell.run",
                c.write(run, 3, "shell.run", json!(["rm"])).await,
            )
        }
        .await,
    ));
    out.push((
        "a_thread_outside_the_run_is_refused",
        async {
            c.refused_read(run, 4, "threads.read", json!([f.thread_outside_run]))
                .await?;
            refused(
                "threads.reply",
                c.write(run, 5, "threads.reply", json!([f.thread_outside_run, "hi"]))
                    .await,
            )?;
            refused(
                "threads.resolve",
                c.write(run, 6, "threads.resolve", json!([f.thread_outside_run]))
                    .await,
            )
        }
        .await,
    ));
    out.push((
        "a_thread_of_another_artifact_is_refused",
        async {
            c.refused_read(run, 7, "threads.read", json!([f.foreign_thread]))
                .await?;
            refused(
                "threads.reply",
                c.write(run, 8, "threads.reply", json!([f.foreign_thread, "hi"]))
                    .await,
            )
        }
        .await,
    ));
    out.push((
        "a_thread_request_creates_no_artifact",
        refused(
            "artifacts.create",
            c.write(run, 9, "artifacts.create", json!([{"title": "X"}]))
                .await,
        ),
    ));
    out.push((
        "a_repeated_key_answers_once_and_a_reused_one_is_refused",
        async {
            let body = "Conformance: once.";
            let args = json!([inside, body]);
            let first = c
                .write(run, 10, "threads.reply", args.clone())
                .await
                .map_err(|e| format!("the reply failed: {e}"))?;
            let again = c
                .write(run, 10, "threads.reply", args)
                .await
                .map_err(|e| format!("the repeated reply failed: {e}"))?;
            if first != again {
                return Err(format!("the repeat answered {again}, the first {first}"));
            }
            if comments_saying(f, &inside, body)? != 1 {
                return Err("the repeated key wrote again".into());
            }
            if !f
                .store
                .bot_calls(&run.id, STEP)
                .map_err(|e| e.to_string())?
                .iter()
                .any(|e| e.seq() == 10 && e.ok())
            {
                return Err("the reply is not journaled".into());
            }
            match c
                .write(run, 10, "threads.reply", json!([inside, "Twice?"]))
                .await
            {
                Err(code) if code == "key_reused" => Ok(()),
                other => Err(format!("a reused key answered {other:?}, not key_reused")),
            }
        }
        .await,
    ));
    out.push((
        "a_crash_between_a_write_and_its_journal_entry_writes_once",
        async {
            let body = "Conformance: crash.";
            c.crash_once(run, 20, "threads.reply", json!([inside, body]), || async {
                Ok(json!(comments_saying(f, &inside, body)?))
            })
            .await?;
            let doc = |id: &'static str| {
                let c = &c;
                move || async move { c.read(run, "db.get", json!(["conformance", id])).await }
            };
            c.crash_stands(
                run,
                21,
                "db.set",
                json!(["conformance", "a", {"n": 1}]),
                doc("a"),
            )
            .await?;
            let version =
                c.read(run, "db.get", json!(["conformance", "a"])).await?["version"].clone();
            c.crash_stands(
                run,
                22,
                "db.update",
                json!(["conformance", "a", {"n": 2}, {"if_version": version}]),
                doc("a"),
            )
            .await?;
            let version =
                c.read(run, "db.get", json!(["conformance", "a"])).await?["version"].clone();
            c.crash_stands(
                run,
                23,
                "db.delete",
                json!(["conformance", "a", {"if_version": version}]),
                doc("a"),
            )
            .await?;
            let current =
                || async { Ok(c.read(run, "artifact.get", json!([])).await?["version"].clone()) };
            let v = current().await?;
            c.crash_once(run, 24, "draft.publish", json!([v]), current)
                .await?;
            let title = "Conformance: crash create";
            let made = || async {
                Ok(json!(
                    f.store
                        .list_artifacts()
                        .map_err(|e| e.to_string())?
                        .iter()
                        .filter(|a| a.title == title)
                        .count()
                ))
            };
            c.crash_once(
                &f.brief_run,
                25,
                "artifacts.create",
                json!([{"title": title}]),
                made,
            )
            .await?;
            let resolved = || async {
                let t = f
                    .store
                    .get_thread(&inside)
                    .map_err(|e| e.to_string())?
                    .ok_or("the run's thread is gone")?;
                Ok(json!(t.status))
            };
            c.crash_once(run, 26, "threads.resolve", json!([inside]), resolved)
                .await
        }
        .await,
    ));
    out.push((
        "a_person_revoked_between_plan_and_write_writes_nothing",
        async {
            let r = &f.revoked_run;
            let tid = r.thread_ids.first().cloned().unwrap_or_default();
            let body = "Conformance: revoked.";
            let args = json!([tid, body]);
            let plan = c
                .plan(r, 30, "threads.reply", &args)
                .await
                .map_err(|e| format!("the reply was refused before the revoke: {e}"))?;
            (f.revoke)(&r.person);
            let entry = c.entry(r, 30, "threads.reply", &args)?;
            if let Ok(v) = run_plan(plan, entry).await {
                return Err(format!("the revoked person's reply answered {v}"));
            }
            match comments_saying(f, &tid, body)? {
                0 => Ok(()),
                n => Err(format!("the revoked person's reply made {n} comments")),
            }
        }
        .await,
    ));
    if let Some(reader) = &f.reader_run {
        let thread = reader.thread_ids.first().cloned().unwrap_or_default();
        out.push((
            "a_write_without_rights_is_refused",
            refused(
                "threads.reply",
                c.write(reader, 12, "threads.reply", json!([thread, "hi"]))
                    .await,
            ),
        ));
    }
    for r in [&f.run, &f.brief_run, &f.revoked_run]
        .into_iter()
        .chain(f.reader_run.as_ref())
    {
        let _ = f.store.release_run(&r.id, &c.driver);
    }
    out
}
