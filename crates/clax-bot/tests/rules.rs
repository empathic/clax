//! What a run may reach (its own threads and artifact; one artifact
//! created, and only for a brief), the host conformance suite, a draft
//! rebased over a version someone else published, and a script run again
//! from its journal.

use crate::common::*;
use crate::resume::crash_once;
use clax_bot::host::args_hash;
use clax_bot::host_conformance::{HostFixture, check_host};
use clax_bot::run::CrashPoint;
use clax_bot::{Next, Wait};
use clax_core::ArtifactId;
use clax_core::audit::AuditCtx;
use clax_store::bot_runs::{DriverId, NewRun, RequestKind, RunStatus, StepKind};
use clax_store::credits::KeySource;
use serde_json::{Value, json};
use std::sync::Arc;

/// The value a committed script step returned.
fn script_value(env: &Env, run: &str, n: usize) -> Value {
    let (_, steps) = env.store.run_with_steps(run).unwrap().unwrap();
    let p = steps[n - 1].payload.clone().unwrap();
    let content: Value = serde_json::from_str(p["result"]["content"].as_str().unwrap()).unwrap();
    content
}

#[tokio::test]
async fn a_run_reaches_only_its_threads_and_a_thread_request_creates_nothing() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Reach out",
        "const codes = [];
         for (const f of [() => clax.threads.read('T2'), () => clax.threads.reply('T2', 'x'),
                          () => clax.threads.resolve('T2'),
                          () => clax.artifacts.create({title: 'New'})]) {
           try { await f(); codes.push('ok'); } catch (e) { codes.push(e.code); }
         }
         return codes;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        script_value(&env, &run, 2)["value"],
        json!(["forbidden", "forbidden", "forbidden", "forbidden"])
    );
    let st = env.host.lock();
    assert!(
        st.calls
            .iter()
            .all(|(_, op)| !op.starts_with("threads.") && op != "artifacts.create"),
        "{:?}",
        st.calls
    );
}

#[tokio::test]
async fn a_brief_run_creates_one_artifact() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Create",
        "const a = await clax.artifacts.create({title: 'New'});
         try { await clax.artifacts.create({title: 'Again'}); return 'twice'; }
         catch (e) { return [typeof a.id, e.code]; }",
    ));
    env.api.push(env.script_turn(
        "tu_2",
        "Create again",
        "try { await clax.artifacts.create({title: 'Later'}); return 'again'; }
         catch (e) { return e.code; }",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run_as(RequestKind::Brief);
    env.drive(&env.deps(), &run, &DriverId::mint()).await;
    assert_eq!(
        script_value(&env, &run, 2)["value"],
        json!(["string", "forbidden"])
    );
    assert_eq!(script_value(&env, &run, 4)["value"], json!("forbidden"));
    assert_eq!(
        env.host
            .lock()
            .writes
            .iter()
            .filter(|w| w.op == "artifacts.create")
            .count(),
        1
    );
}

#[tokio::test]
async fn the_fake_host_passes_the_host_conformance_suite() {
    let env = Env::new().await;
    let run = env.store.run(&env.run()).unwrap().unwrap();
    let brief_run = env
        .store
        .run(&env.run_as(RequestKind::Brief))
        .unwrap()
        .unwrap();
    let run_for = |person: &str| {
        env.store
            .create_run(
                &clax_bot::run::bot_ctx(person),
                NewRun {
                    artifact_id: env.aid.clone(),
                    person: person.into(),
                    request: RequestKind::Thread,
                    thread_ids: vec![env.t1.clone()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                },
            )
            .unwrap()
    };
    let reader = run_for("u_reader");
    let revoked = run_for("u_revoked");
    env.host.lock().readers.push("u_reader".into());
    let host = env.host.clone();
    let fixture = HostFixture {
        host: env.host.clone(),
        store: env.store.clone(),
        run,
        brief_run,
        thread_outside_run: env.t2.clone(),
        foreign_thread: env.foreign.clone(),
        reader_run: Some(reader),
        revoked_run: revoked,
        revoke: Box::new(move |person| host.lock().readers.push(person.to_string())),
    };
    let results = check_host(&fixture).await;
    assert_eq!(results.len(), 9);
    for (name, r) in results {
        r.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[tokio::test]
async fn a_draft_is_rebased_over_a_version_someone_else_published() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Edit",
        "await clax.files.replace('index.html', '--accent: #c00;', '--accent: #00c;'); return 1;",
    ));
    env.api.push(env.script_turn(
        "tu_2",
        "Publish",
        "let first;
         try { await clax.draft.publish({note: 'blue'}); } catch (e) { first = e.code; }
         const rb = await clax.draft.rebase();
         const theirs = await clax.draft.theirs('index.html');
         const ours = await clax.files.read('index.html');
         const v = await clax.draft.publish({note: 'blue'});
         return {first, rb, theirs: theirs.includes('NEW'), ours: ours.includes('#00c'), v: v.version};",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    env.tick(&deps, &run, &d).await;
    // The person publishes version 2 while the run works.
    let aid = ArtifactId::parse(&env.aid).unwrap();
    let req: clax_core::publish::PublishRequest = serde_json::from_value(json!({
        "files": {"index.html": {"content": PAGE.replace("Quarterly goals", "NEW goals"), "encoding": "utf8"}},
        "if_version": 1
    }))
    .unwrap();
    env.store
        .publish_version(
            &AuditCtx::DAEMON,
            &aid,
            clax_core::publish::validate(req).unwrap(),
            None,
        )
        .unwrap();
    let out = env.drive(&deps, &run, &d).await;
    assert_eq!(out.last().unwrap().next, Next::Done(RunStatus::Finished));
    let v = script_value(&env, &run, 4)["value"].clone();
    assert_eq!(v["first"], "conflict");
    assert_eq!(v["rb"]["version"], 2);
    assert_eq!(v["rb"]["conflicts"], json!(["index.html"]));
    assert_eq!(
        (v["theirs"].as_bool(), v["ours"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(v["v"], 3);
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    assert_eq!(steps[3].payload.as_ref().unwrap()["base"], 3);
    let (bytes, _) = env.store.read_file(&aid, 3, "index.html").unwrap().unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("#00c"));
}

#[tokio::test]
async fn a_script_run_again_that_takes_another_path_stops_where_it_diverged() {
    let env = Env::new().await;
    env.api.push(env.script_turn(
        "tu_1",
        "Read then reply",
        "const a = await clax.db.get('c', 'x'); await clax.threads.reply('T1', 'hi'); return a;",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let deps = env.deps();
    let d = DriverId::mint();
    env.tick(&deps, &run, &d).await;
    // The journal says call 0 of step 2 read another document.
    let entry = env
        .store
        .call_entry(&run, &d, 2, 1, 0, "db.get", &args_hash(&json!(["c", "y"])))
        .unwrap();
    env.store.record_bot_call(&entry).unwrap();
    assert_eq!(env.tick(&deps, &run, &d).await.kind, Some(StepKind::Script));
    let content = script_value(&env, &run, 2);
    assert_eq!(content["error"]["kind"], "host_error");
    assert!(
        content["error"]["message"]
            .as_str()
            .unwrap()
            .contains("replay_diverged"),
        "{content}"
    );
    assert_eq!(content["replay"]["diverged_at"], 0);
    assert!(env.host.lock().calls.is_empty(), "nothing reached the host");
}

#[tokio::test]
async fn a_script_run_again_sees_its_steps_clock_and_journaled_reads() {
    let env = Env::new().await;
    env.set_doc("c/x", json!({"n": 1}));
    env.api.push(env.script_turn(
        "tu_1",
        "Clock",
        "const d = await clax.db.get('c', 'x'); return [Date.now(), Math.random(), d.data.n];",
    ));
    env.api.push(finish_turn("done"));
    let run = env.run();
    let mut a = env.deps();
    crash_once(
        &mut a,
        CrashPoint::BeforeCommit {
            step: 2,
            kind: StepKind::Script,
        },
    );
    let da = DriverId::mint();
    env.tick(&a, &run, &da).await;
    assert_eq!(
        env.tick(&a, &run, &da).await.next,
        Next::Waiting(Wait::Crashed)
    );
    // The document changes and time passes before the step runs again.
    env.set_doc("c/x", json!({"n": 2}));
    env.clock.advance(61);
    let b = env.deps();
    env.drive(&b, &run, &DriverId::mint()).await;
    let (_, steps) = env.store.run_with_steps(&run).unwrap().unwrap();
    let started = chrono::DateTime::parse_from_rfc3339(&steps[1].started_at)
        .unwrap()
        .timestamp_millis();
    assert_eq!(steps[1].attempt, 2);
    let v = script_value(&env, &run, 2)["value"].clone();
    assert_eq!(v[0], started, "the clock is the step's first start");
    assert_eq!(
        v[1],
        clax_bot_sandbox::seeded_random(
            u32::from_str_radix(
                &clax_core::audit::sha256_hex(format!("{run}:2").as_bytes())[..8],
                16
            )
            .unwrap(),
            1
        )[0]
    );
    assert_eq!(v[2], 1, "the read answered from the journal");
    let reads = env
        .host
        .lock()
        .calls
        .iter()
        .filter(|(_, op)| op == "db.get")
        .count();
    assert_eq!(reads, 1);
    let _ = Arc::clone(&env.host);
}
