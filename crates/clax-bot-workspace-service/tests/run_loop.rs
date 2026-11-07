//! The real run loop against the real service, through
//! [`clax_bot_remote::RemoteWorkspace`]: what a hosted run does, and what it
//! makes of a service that died under it. A service's death is its runtime
//! shut down, so nothing it was doing goes on.

use crate::common::*;
use crate::fixture::{HOLD, Host, World};
use clax_bot::host::OpCtx;
use clax_bot::run::OffersLapse;
use clax_bot::workspace::{Offer, Proposal};
use clax_bot::{Deps, Next, TickOutcome, Wait, Workspace};
use clax_bot_remote::MAX_SNAPSHOT_AGE_MS;
use clax_bot_sandbox::HostError;
use clax_bot_workspace_service::routes::is_key;
use clax_core::audit::AuditCtx;
use clax_core::queue::{Report, ReportersSee};
use clax_store::bot_runs::{DriverId, RunStatus, StepKind};
use clax_store::queues::{ExternalScope, NewQueue, NewReport, ReporterToken};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;

/// A tick's budget: enough to begin a call, and under what a host waits for
/// one request, so a tick gives up its call rather than the call failing.
const BUDGET: Duration = Duration::from_secs(6);

const FIX: &str = r#"
    await clax.shell.exec("echo two >> app.txt");
    return await clax.change.propose({title: "Add two", summary: "It lacked two."});
"#;

/// A report on a queue: the queue's ID and the report.
fn filed(env: &Env) -> (String, Report) {
    let ctx = AuditCtx::DAEMON;
    let new = NewQueue {
        name: "App".into(),
        origins: vec!["https://app.example".into()],
        reporters_see: ReportersSee::Own,
    };
    let q = env.store.create_queue(&ctx, None, new).unwrap();
    let token = ReporterToken {
        queue_id: q.id.clone(),
        service_grant_id: "g1".into(),
        reporter_id: "reporter-1".into(),
        origin: "https://app.example".into(),
        public_key: String::new(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        profile: None,
    };
    let new = NewReport {
        external_id: None,
        url: "https://app.example/".into(),
        title: "App".into(),
        anchor: serde_json::from_value(json!({"kind": "element", "selector": "#a"})).unwrap(),
        body: "It is broken".into(),
        author_name: "reporter-1".into(),
        metadata: json!({}),
        revision: None,
        clip: None,
        context: None,
        server: None,
        diagnostics: None,
    };
    let scope = ExternalScope::Instance;
    let report = env.store.file_report(&ctx, &q.id, Some(&token), scope, new);
    (q.id, report.unwrap())
}

fn script_value(env: &Env, run: &str) -> Value {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let s = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    let p = s.payload.as_ref().unwrap();
    serde_json::from_str(p["result"]["content"].as_str().unwrap()).unwrap()
}

/// `h`'s workspace in a run's deps, its offers fetched as a driver's start
/// fetches them.
async fn deps(env: &Env, h: &Host, run: &str) -> Deps {
    use clax_bot::Workspace;
    h.remote.reap(&[run.to_string()]).await;
    env.deps().with_lapsing_workspace(h.remote.clone())
}

/// A run of `code` on a report, past its first turn.
async fn at_the_script(env: &Env, code: &str, deps: &Deps, run: &str) -> DriverId {
    env.api.push(script_turn("tu_1", "Fix", code));
    env.api.push(finish_turn("done"));
    let d = DriverId::mint();
    assert_eq!(env.tick(deps, run, &d).await.kind, Some(StepKind::Turn));
    d
}

/// Ticks until `reached`, and lets that tick yield at once: the clock paused
/// here is this runtime's, so the service goes on, and only the tick's budget
/// passes. A tick that yields before `reached` is ticked again, so no result
/// rests on how long anything took.
async fn tick_until(
    env: &Env,
    deps: &Deps,
    run: &str,
    d: &DriverId,
    reached: impl Future<Output = ()>,
) -> TickOutcome {
    let mut reached = std::pin::pin!(reached);
    loop {
        let tick = env.try_tick(deps, run, d, Some(BUDGET));
        let mut tick = std::pin::pin!(tick);
        tokio::select! {
            out = &mut tick => {
                let out = out.unwrap();
                assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");
            }
            () = &mut reached => {
                tokio::time::pause();
                let out = tick.await;
                tokio::time::resume();
                return out.unwrap();
            }
        }
    }
}

fn finished(out: &[TickOutcome]) {
    assert_eq!(
        out.last().unwrap().next,
        Next::Done(RunStatus::Finished),
        "{out:?}"
    );
}

#[test]
fn a_key_the_run_loop_makes_is_one_the_wire_takes() {
    let audit = AuditCtx::DAEMON;
    let id = clax_core::new_ulid();
    let run = crate::fixture::run(&id);
    for (step, seq) in [
        (1, 0),
        (i64::MAX, i64::MAX),
        (1, clax_bot::host::INTENT_SEQ),
    ] {
        let key = OpCtx::new(&run, &audit, step, 1, seq).key;
        assert!(is_key(&key), "{key}");
    }
}

#[tokio::test]
async fn a_hosted_run_changes_the_repository_and_ends_its_container() {
    let env = Env::new().await;
    let (qid, report) = filed(&env);
    let w = World::with_repository_on(&qid);
    let service = w.service();
    let h = w.host(&service);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = deps(&env, &h, &run).await;
    let d = at_the_script(&env, FIX, &deps, &run).await;

    finished(&env.drive(&deps, &run, &d).await);

    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    assert_eq!(w.forge.opened(), 1);
    assert_eq!(w.runtime.live(), 0);
}

#[tokio::test]
async fn a_proposal_replayed_after_the_service_died_mid_push_opens_one_pull_request() {
    let env = Env::new().await;
    let (qid, report) = filed(&env);
    let w = World::with_repository_on(&qid);
    w.forge.gated.store(true, Ordering::SeqCst);
    let tokio = Runtime::new().unwrap();
    let service = w.service_on(&tokio);
    let h = Host::on(&service, &tokio);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let first = deps(&env, &h, &run).await;
    let d = at_the_script(&env, FIX, &first, &run).await;

    let reached = w.forge.opening.notified();
    let out = tick_until(&env, &first, &run, &d, reached).await;
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");
    assert_eq!(w.forge.opened(), 0);

    tokio.shutdown_background();
    w.forge.gated.store(false, Ordering::SeqCst);
    let service = w.service();
    let h = w.host(&service);
    let again = deps(&env, &h, &run).await;
    finished(&env.drive(&again, &run, &d).await);

    assert_eq!(env.run_row(&run).status, RunStatus::Finished);
    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    assert_eq!(w.forge.opened(), 1);
    assert_eq!(w.runtime.ran("echo two >> app.txt"), 1);
}

/// A tick of a moment, for a call that is not going to answer: it yields, at
/// its call or before, however long anything takes.
async fn tick_short(env: &Env, deps: &Deps, run: &str, d: &DriverId) -> TickOutcome {
    let moment = Some(Duration::from_millis(200));
    env.try_tick(deps, run, d, moment).await.unwrap()
}

#[tokio::test]
async fn an_outage_past_the_age_of_the_offers_marks_nothing_and_a_public_fix_follows() {
    let env = Env::new().await;
    let (qid, report) = filed(&env);
    let w = World::with_repository_on(&qid);
    let service = w.service();
    let h = w.host(&service);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = deps(&env, &h, &run).await;
    let code = format!(r#"await clax.shell.exec("{HOLD}"); {FIX}"#);
    let d = at_the_script(&env, &code, &deps, &run).await;

    let reached = w.runtime.entered.notified();
    let out = tick_until(&env, &deps, &run, &d, reached).await;
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");

    h.transport.down.store(true, Ordering::SeqCst);
    h.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 + 1);
    let out = tick_short(&env, &deps, &run, &d).await;
    assert!(!env.run_row(&run).confidential, "nothing is kept");
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");

    // Nothing asks first: what the job answers brings the offers back, in
    // the tick that began without them.
    h.transport.down.store(false, Ordering::SeqCst);
    w.runtime.release();
    finished(&env.drive(&deps, &run, &d).await);

    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    assert_eq!(w.forge.opened(), 1);
    assert!(!env.run_row(&run).confidential);
    let next = env.run_for(OWNER, &report.artifact.id, &report.id);
    assert!(!deps.confidential(&env.run_row(&next)).unwrap());
    assert!(!env.run_row(&next).confidential);
}

#[tokio::test]
async fn a_proposal_joined_as_the_outage_ends_is_the_fix_of_a_public_run() {
    let env = Env::new().await;
    let (qid, report) = filed(&env);
    let w = World::with_repository_on(&qid);
    w.forge.gated.store(true, Ordering::SeqCst);
    let service = w.service();
    let h = w.host(&service);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = deps(&env, &h, &run).await;
    let d = at_the_script(&env, FIX, &deps, &run).await;

    let reached = w.forge.opening.notified();
    let out = tick_until(&env, &deps, &run, &d, reached).await;
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");

    h.transport.down.store(true, Ordering::SeqCst);
    h.clock.advance(MAX_SNAPSHOT_AGE_MS / 1000 + 1);
    let out = tick_short(&env, &deps, &run, &d).await;
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");

    h.transport.down.store(false, Ordering::SeqCst);
    w.forge.release();
    finished(&env.drive(&deps, &run, &d).await);

    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    assert!(!env.run_row(&run).confidential);
}

/// A workspace whose offers have lapsed until a proposal's answer brings them
/// back, and whose preview is up at once. Its first proposal never answers, so
/// the next one is a join.
#[derive(Default)]
struct Flips {
    lapsed: AtomicBool,
    joined: AtomicBool,
}

impl OffersLapse for Flips {
    fn offers_lapsed(&self) -> bool {
        self.lapsed.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Workspace for Flips {
    fn offer(&self, _run: &clax_store::bot_runs::Run, _queue: Option<&str>) -> Offer {
        if self.offers_lapsed() {
            return clax_bot_remote::lapsed_offer();
        }
        Offer::CHANGE
    }

    fn resumable(&self) -> bool {
        true
    }

    async fn exec(
        &self,
        _run: &clax_store::bot_runs::Run,
        _queue: Option<&str>,
        _command: &str,
        _cwd: Option<&str>,
        _timeout_secs: u64,
    ) -> Result<Value, HostError> {
        Ok(json!({"exit_code": 0, "output": ""}))
    }

    async fn propose(
        &self,
        _run: &clax_store::bot_runs::Run,
        _proposal: &Proposal,
    ) -> Result<Value, HostError> {
        if !self.joined.swap(true, Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.lapsed.store(false, Ordering::SeqCst);
        Ok(json!({
            "pr": 1,
            "url": "https://forge.example/pr/1",
            "commit": "c".repeat(40),
            "verify_url": "https://pr-1.preview.example/",
        }))
    }

    async fn finish(&self, _run: &clax_store::bot_runs::Run) {}
}

#[tokio::test]
async fn while_offers_have_lapsed_a_run_reads_as_confidential_and_nothing_is_kept() {
    let env = Env::new().await;
    let (_, report) = filed(&env);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let w = Arc::new(Flips::default());
    w.lapsed.store(true, Ordering::SeqCst);
    let deps = env.deps().with_lapsing_workspace(w.clone());
    let code = r#"
        try { await clax.artifacts.create({title: "t"}); return "left"; }
        catch (e) { return e.code; }
    "#;
    let d = at_the_script(&env, code, &deps, &run).await;
    finished(&env.drive(&deps, &run, &d).await);

    assert_eq!(script_value(&env, &run)["value"], "confidential");
    assert!(!env.run_row(&run).confidential);
}

/// A lapse that reaches a run only on a queue, as a service's copy of its
/// offers says.
struct QueuesOnly;

impl OffersLapse for QueuesOnly {
    fn offers_lapsed(&self) -> bool {
        true
    }

    fn lapse_reaches(&self, _run: &clax_store::bot_runs::Run, queue: Option<&str>) -> bool {
        queue.is_some()
    }
}

#[async_trait::async_trait]
impl Workspace for QueuesOnly {
    fn offer(&self, _run: &clax_store::bot_runs::Run, _queue: Option<&str>) -> Offer {
        clax_bot_remote::lapsed_offer()
    }

    async fn exec(
        &self,
        _run: &clax_store::bot_runs::Run,
        _queue: Option<&str>,
        _command: &str,
        _cwd: Option<&str>,
        _timeout_secs: u64,
    ) -> Result<Value, HostError> {
        Err(HostError::new("shell_unavailable", "out"))
    }

    async fn propose(
        &self,
        _run: &clax_store::bot_runs::Run,
        _proposal: &Proposal,
    ) -> Result<Value, HostError> {
        Err(HostError::new("shell_unavailable", "out"))
    }

    async fn finish(&self, _run: &clax_store::bot_runs::Run) {}
}

#[tokio::test]
async fn a_lapse_leaves_a_plain_artifact_run_as_it_was() {
    let env = Env::new().await;
    let run = env.run();
    let deps = env.deps().with_lapsing_workspace(Arc::new(QueuesOnly));
    let code = r#"
        try { await clax.artifacts.create({title: "t"}); return "left"; }
        catch (e) { return e.code; }
    "#;
    let d = at_the_script(&env, code, &deps, &run).await;
    finished(&env.drive(&deps, &run, &d).await);

    assert_ne!(script_value(&env, &run)["value"], "confidential");
    assert!(!env.run_row(&run).confidential);
}

#[tokio::test]
async fn a_proposal_that_ends_a_lapse_is_fixed_on_what_its_answer_brought() {
    let env = Env::new().await;
    let (_, report) = filed(&env);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let w = Arc::new(Flips::default());
    let deps = env.deps().with_lapsing_workspace(w.clone());
    let code =
        r#"return await clax.change.propose({title: "Add two", summary: "It lacked two."});"#;
    let d = at_the_script(&env, code, &deps, &run).await;

    let out = tick_short(&env, &deps, &run, &d).await;
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");
    w.lapsed.store(true, Ordering::SeqCst);
    finished(&env.drive(&deps, &run, &d).await);

    assert!(!env.run_row(&run).confidential);
    let fixes = env.host.lock().fixes.clone();
    // A public run's fix: its link reaches the reporter and it is not held.
    // Its words stay fixed either way; the model's go to the queue's people.
    assert!(fixes[0].verify_url.is_some(), "{fixes:?}");
    assert!(!fixes[0].held, "{fixes:?}");
    assert_eq!(fixes[0].summary, clax_bot::workspace::FIX_PROPOSED);
    let note = fixes[0].members_note.as_deref().unwrap_or_default();
    assert!(note.contains("It lacked two."), "{fixes:?}");
}

#[tokio::test]
async fn a_command_cut_off_by_the_service_dying_is_interrupted_and_not_run_again() {
    let env = Env::new().await;
    let (qid, report) = filed(&env);
    let w = World::with_repository_on(&qid);
    let tokio = Runtime::new().unwrap();
    let service = w.service_on(&tokio);
    let h = Host::on(&service, &tokio);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let first = deps(&env, &h, &run).await;
    let code = format!(
        r#"try {{ return await clax.shell.exec("{HOLD}"); }} catch (e) {{ return e.code; }}"#
    );
    let d = at_the_script(&env, &code, &first, &run).await;

    let reached = w.runtime.entered.notified();
    let out = tick_until(&env, &first, &run, &d, reached).await;
    assert_eq!(out.next, Next::Waiting(Wait::Budget), "{out:?}");

    tokio.shutdown_background();
    let service = w.service();
    let h = w.host(&service);
    let again = deps(&env, &h, &run).await;
    finished(&env.drive(&again, &run, &d).await);

    assert_eq!(script_value(&env, &run)["value"], "shell_interrupted");
    assert_eq!(w.runtime.ran(HOLD), 1);
}
