//! Joins over HTTP (`docs/contract.md`, "ClaxBot surfaces", "Joins";
//! `clax_server::routes::bot_joins`): a host joins the runs of Clax's own
//! surface on its person's artifacts, and a thread run started there
//! while it is connected has its surface too, calls it through the
//! daemon's hub, tells the model of it, and shows a reader nothing of
//! its calls. With the host away, or for another person's run, the run
//! is made as a run with no join is.

use clax_core::audit::AuditCtx;
use clax_core::working::ManualClock;
use clax_server::routes::bot::Sender;
use clax_store::bot_joins::Join;
use clax_store::bot_runs::{Bindings, StoredBinding};
use clax_surface::wire::Notation;
use clax_surface::wire::Outcome;
use clax_testkit::fake_host::{Behaviour, FakeHost, HttpTransport, Served, When};
use clax_testkit::fake_messages::{FakeMessages, Turn};
use clax_testkit::remotes::json_of;
use clax_testkit::{ManualSleeper, TestServer};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FIXTURE: &str = include_str!("../../../docs/claxbot-surfaces/contract.json");
const KEY: &str = "sk-ant-api03-TOPSECRETKEYVALUE-abcd";
const START: &str = "2026-11-11T00:00:00Z";
/// The time a driver is given to become idle; no test comes near it.
const BUDGET: Duration = Duration::from_secs(3600);

fn desk_file() -> String {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    fixture["files"]["desk.d.ts"].as_str().unwrap().to_string()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A daemon on a clock the test moves, its driver ticked by the test,
/// its model `fake`'s, its instance key set.
struct Daemon {
    ts: TestServer,
    clock: Arc<ManualClock>,
    sleeper: Arc<ManualSleeper>,
    driver: Arc<clax_bot::Driver>,
}

async fn daemon(fake: &FakeMessages) -> Daemon {
    let clock = Arc::new(ManualClock::at(START));
    let sleeper = Arc::new(ManualSleeper::default());
    let slot = Arc::new(Mutex::new(None));
    let (s, kept) = (sleeper.clone(), slot.clone());
    let ts = TestServer::spawn_with_clock_and(clock.clone(), move |state| {
        state.bot_surfaces.set_sleeper(s);
        *kept.lock().unwrap() = Some(clax_server::bot::driver(state));
    })
    .await;
    let driver = slot.lock().unwrap().take().expect("a driver");
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(
                clax_store::bot_settings::Setting::BaseUrl,
                Some(json!(fake.base)),
            )],
        )
        .unwrap();
    let r = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}));
    assert_eq!(r.send().await.unwrap().status(), 200);
    Daemon {
        ts,
        clock,
        sleeper,
        driver,
    }
}

impl Daemon {
    /// The desk's host, registered, answering `lamps` with two lamps.
    async fn desk(&self) -> FakeHost<HttpTransport> {
        let mut host = FakeHost::new(
            HttpTransport::of(&self.ts),
            "desk",
            Notation::Dts,
            &desk_file(),
        )
        .on(
            "lamps",
            Behaviour::result(json!([{"id": "l1", "on": true}, {"id": "l2", "on": false}])),
        )
        .wait_ms(5_000);
        host.register().await.expect("desk registers");
        host
    }

    /// An artifact of the owner's with a thread on it that the owner
    /// wrote in their browser: their IDs.
    async fn page(&self) -> (String, String) {
        let aid = self
            .ts
            .publish("Desk notes", &[("index.html", "<p>hi</p>")])
            .await["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let form = reqwest::multipart::Form::new()
            .text("anchor", clax_testkit::element_anchor().to_string())
            .text("body", "Which lamps are on?")
            .text("version", "1");
        let r = self
            .ts
            .client
            .post(format!("{}/api/artifacts/{aid}/threads", self.ts.base))
            .header("cookie", self.ts.owner_cookie())
            .multipart(form)
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(r).await;
        assert_eq!(s, 201, "{v}");
        let tid = v["thread"]["id"].as_str().unwrap().to_string();
        (aid, tid)
    }

    /// The owner comments `body` on thread `tid` of `aid` in their
    /// browser: the answer, with the follow-up run it started, if any.
    async fn owner_says(&self, aid: &str, tid: &str, body: &str) -> Value {
        let r = self
            .ts
            .client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                self.ts.base
            ))
            .header("cookie", self.ts.owner_cookie())
            .json(&json!({"body": body}))
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(r).await;
        assert_eq!(s, 201, "{v}");
        v
    }

    /// The owner sends `tid` of `aid` to ClaxBot: the run's ID.
    async fn send(&self, aid: &str, tid: &str) -> String {
        let r = self
            .ts
            .client
            .post(format!("{}/api/bot/runs", self.ts.base))
            .header("cookie", self.ts.owner_cookie())
            .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(r).await;
        assert_eq!(s, 201, "{v}");
        v["run"]["id"].as_str().unwrap().to_string()
    }

    /// The bindings run `id` was made with.
    fn bindings(&self, id: &str) -> Bindings {
        let (_, made, _) = self.ts.store.run_with_steps_as_made(id).unwrap().unwrap();
        made.bindings().unwrap()
    }

    async fn get(&self, path: &str, cookie: &str) -> (u16, Value) {
        let r = self
            .ts
            .client
            .get(format!("{}{path}", self.ts.base))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        json_of(r).await
    }

    /// `GET /api/bot/surfaces`, with the token.
    async fn listed(&self) -> Value {
        let r = self
            .ts
            .client
            .get(format!("{}/api/bot/surfaces", self.ts.base))
            .bearer_auth(&self.ts.token)
            .send()
            .await
            .unwrap();
        let (s, v) = json_of(r).await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

/// What `GET /api/bot/runs/<id>/events` sends `cookie` from the run's
/// first event, read until it holds `until` (a stream of a run that has
/// not ended does not end).
async fn events_until(d: &Daemon, run: &str, cookie: &str, until: &str) -> String {
    let mut r =
        d.ts.client
            .get(format!("{}/api/bot/runs/{run}/events", d.ts.base))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
    let mut body = String::new();
    while !body.contains(until) {
        let chunk = tokio::time::timeout(clax_testkit::HANG, r.chunk())
            .await
            .expect("the stream goes on")
            .unwrap()
            .unwrap_or_else(|| panic!("the stream ended without {until:?}: {body}"));
        body.push_str(&String::from_utf8_lossy(&chunk));
    }
    body
}

/// The text of every system prompt `fake` was sent.
fn systems(fake: &FakeMessages) -> Vec<String> {
    fake.requests()
        .iter()
        .map(|r| match &r["system"] {
            Value::String(text) => text.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<String>(),
            other => panic!("no system prompt: {other}"),
        })
        .collect()
}

/// With a join and its host connected, a thread run on the artifact has
/// the host's surface: its script calls the host's operation through the
/// daemon's hub and gets the host's answer, its model is told the
/// surface's declaration and rules, its host is told how it ended, and a
/// reader of the artifact sees nothing of the host's calls.
#[tokio::test]
async fn a_thread_run_on_a_joined_artifact_calls_its_host_and_gets_its_answer() {
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Count the lamps that are on",
            "const lamps = await clax.desk.lamps(); return lamps.filter((l) => l.on).length;",
        ),
        Turn::finish("One lamp is on."),
    ])
    .await;
    let d = daemon(&fake).await;
    let (aid, tid) = d.page().await;
    let mut host = d.desk().await;
    let joined = host
        .join(&[aid.as_str()], None)
        .await
        .expect("the desk joins");
    assert_eq!(
        joined,
        json!({"registration": host.registration().unwrap(), "clax": [{"artifact": aid}]})
    );
    let listed = d.listed().await;
    assert_eq!(
        listed["surfaces"][0]["joins"],
        json!({"clax": [{"artifact": aid}]})
    );

    let run = d.send(&aid, &tid).await;
    let bindings = d.bindings(&run);
    let names: Vec<&str> = bindings.all().iter().map(StoredBinding::surface).collect();
    assert_eq!(names, ["clax", "desk"]);
    let desk = bindings.of("desk").unwrap();
    assert_eq!(desk.registration(), host.registration());
    // A join that names no operation binds the surface's reads alone:
    // its image too, on a daemon that keeps content.
    assert_eq!(desk.operations().unwrap(), ["desk.lamps", "desk.snapshot"]);
    // Clax's own binding has what the run has with no join.
    let web = d.ts.store.run(&run).unwrap().unwrap().web;
    assert_eq!(
        bindings.of("clax").unwrap(),
        &clax_bot::joins::clax_binding(&aid, &web)
    );

    let ticking = {
        let driver = d.driver.clone();
        tokio::spawn(async move { driver.run_until_idle(BUDGET).await })
    };
    // The call, then its answer and the run's end.
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.len() == 1));
    host.serve_until(|h| !h.ended().is_empty(), 6)
        .await
        .expect("the host is told the run ended");
    ticking.await.unwrap();
    let calls = host.calls_of("lamps");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].run, run);
    // The host is told which artifact, and which threads, the call is for.
    let on = json!({"clax": {"artifact": aid, "thread_ids": [tid]}});
    assert_eq!(calls[0].joined, Some(on.clone()));
    assert_eq!(d.bindings(&run).of("desk").unwrap().scope(), &on);
    assert_eq!(host.ended()[0].run, run);
    assert_eq!(fake.left(), 0);

    // The script was answered the host's lamps.
    let (_, steps) = d.ts.store.run_with_steps(&run).unwrap().unwrap();
    let script = steps
        .iter()
        .find(|s| s.kind == clax_store::bot_runs::StepKind::Script)
        .unwrap();
    assert_eq!(
        script.payload.as_ref().unwrap()["outcome"]["value"],
        json!(1)
    );
    // The model was told the desk's declaration and rules, at the end.
    let system = &systems(&fake)[0];
    assert!(system.contains("desk: {"), "{system}");
    assert!(
        system.ends_with("\n\nA desk and what is on it. Ask before you move anything.\n"),
        "{system}"
    );
    assert!(
        system.contains("# desk\n\nThe operations on `clax.desk` act on the person's own"),
        "{system}"
    );

    // The owner sees the host's call; a reader of the artifact sees the
    // run's steps bare.
    let (s, mine) = d
        .get(&format!("/api/bot/runs/{run}"), &d.ts.owner_cookie())
        .await;
    assert_eq!(s, 200, "{mine}");
    assert_eq!(mine["status"], "finished");
    assert!(mine.to_string().contains("desk.lamps"), "{mine}");
    let rae = d.ts.person("Rae").await;
    let (s, theirs) = d
        .get(
            &format!("/api/bot/runs/{run}"),
            &format!("clax_viewer={}", rae.cookie),
        )
        .await;
    assert_eq!(s, 200, "{theirs}");
    let text = theirs.to_string();
    assert!(
        !text.contains("desk.lamps") && !text.contains("Count the lamps"),
        "{theirs}"
    );
    assert_eq!(theirs["status"], "finished");
    let finish = theirs["steps"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(finish["kind"], "finish");
    assert_eq!(finish["summary"], "One lamp is on.");
    // Over the events too.
    let r =
        d.ts.client
            .get(format!("{}/api/bot/runs/{run}/events", d.ts.base))
            .header("cookie", format!("clax_viewer={}", rae.cookie))
            .send()
            .await
            .unwrap();
    let body = tokio::time::timeout(clax_testkit::HANG, r.text())
        .await
        .unwrap()
        .unwrap();
    assert!(body.contains("event: run_end"), "{body}");
    assert!(
        !body.contains("desk.lamps")
            && !body.contains("Count the lamps")
            && !body.contains("event: call"),
        "{body}"
    );
    // With its host away, the reader sees the run as bare: the run's
    // bindings decide, not the registrations the hub holds now.
    d.clock.advance(20);
    let (s, later) = d
        .get(
            &format!("/api/bot/runs/{run}"),
            &format!("clax_viewer={}", rae.cookie),
        )
        .await;
    assert_eq!(s, 200, "{later}");
    assert_eq!(later["steps"], theirs["steps"]);
}

/// A reader of a run whose bindings this build cannot read (a later
/// build's version) sees it as a run a host's surface is bound into is
/// shown: bare in `GET /api/bot/runs/<id>`, the listing and the events,
/// and with no `give_up` reason, the model's own words. With bindings it
/// reads and no host, the same reader sees the run as before.
#[tokio::test]
async fn a_reader_of_a_run_whose_bindings_are_unreadable_sees_it_bare() {
    let fake = FakeMessages::start(vec![
        Turn::script("tu_1", "Count the notes", "return 2;"),
        Turn::give_up("The notes say the lamp code is 4512."),
    ])
    .await;
    let d = daemon(&fake).await;
    let (aid, tid) = d.page().await;
    let run = d.send(&aid, &tid).await;
    d.driver.run_until_idle(BUDGET).await;
    let rae = d.ts.person("Rae").await;
    let theirs = format!("clax_viewer={}", rae.cookie);
    let (s, before) = d.get(&format!("/api/bot/runs/{run}"), &theirs).await;
    assert_eq!(s, 200, "{before}");
    assert_eq!(before["status"], "gave_up", "{before}");
    let text = before.to_string();
    assert!(
        text.contains("Count the notes") && text.contains("lamp code"),
        "{before}"
    );

    let c = rusqlite::Connection::open(d.ts.home.db_path()).unwrap();
    c.busy_timeout(Duration::from_secs(10)).unwrap();
    let changed = c
        .execute(
            "UPDATE bot_runs SET bindings_json = ?2 WHERE id = ?1",
            rusqlite::params![run, json!({"v": 99, "bindings": []}).to_string()],
        )
        .unwrap();
    assert_eq!(changed, 1);
    assert!(
        d.ts.store
            .run_with_steps_as_made(&run)
            .unwrap()
            .unwrap()
            .1
            .bindings()
            .is_err()
    );

    let (s, after) = d.get(&format!("/api/bot/runs/{run}"), &theirs).await;
    assert_eq!(s, 200, "{after}");
    let text = after.to_string();
    assert!(
        !text.contains("Count the notes") && !text.contains("lamp code"),
        "{after}"
    );
    let gave_up = after["steps"].as_array().unwrap().last().unwrap();
    assert_eq!(gave_up["kind"], "give_up", "{after}");
    assert!(gave_up.get("reason").is_none(), "{after}");
    let script = after["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "script")
        .unwrap();
    assert!(
        script.get("calls").is_none() && script.get("title").is_none(),
        "{after}"
    );
    let (s, listed) = d
        .get(&format!("/api/bot/runs?artifact={aid}"), &theirs)
        .await;
    assert_eq!(s, 200, "{listed}");
    assert_eq!(listed["runs"][0]["id"], run.as_str());
    let body = events_until(&d, &run, &theirs, "event: run_end").await;
    assert!(
        !body.contains("Count the notes")
            && !body.contains("lamp code")
            && !body.contains("event: script"),
        "{body}"
    );
    // The owner sees it all.
    let (_, mine) = d
        .get(&format!("/api/bot/runs/{run}"), &d.ts.owner_cookie())
        .await;
    assert!(mine.to_string().contains("Count the notes"), "{mine}");
}

/// With its host not connected, a join binds nothing: the run is made
/// with Clax's own surface alone, as a run with no join, and is sent
/// Clax's prompt as it was.
#[tokio::test]
async fn with_its_host_away_a_join_binds_nothing() {
    let fake = FakeMessages::start(vec![Turn::finish("Nothing to do.")]).await;
    let d = daemon(&fake).await;
    let (aid, tid) = d.page().await;
    let mut host = d.desk().await;
    host.join(&[aid.as_str()], None)
        .await
        .expect("the desk joins");
    // The host has not served for longer than a registration stays
    // connected.
    d.clock.advance(20);
    let run = d.send(&aid, &tid).await;
    let row = d.ts.store.run(&run).unwrap().unwrap();
    assert_eq!(d.bindings(&run), Bindings::clax(&aid, &row.web));
    d.driver.run_until_idle(BUDGET).await;
    let system = &systems(&fake)[0];
    assert!(!system.contains("desk"), "{system}");
    assert!(system.ends_with("```\n"), "{system}");
}

/// A host's joins are replaced whole by each `PUT`, kept when it goes on
/// as its registration, listed under it, and gone with their artifact.
#[tokio::test]
async fn joins_are_replaced_whole_kept_on_registering_again_and_gone_with_their_artifact() {
    let fake = FakeMessages::start(vec![]).await;
    let d = daemon(&fake).await;
    let (a, _) = d.page().await;
    let (b, _) = d.page().await;
    let (c, _) = d.page().await;
    let mut host = d.desk().await;
    let sorted = |mut v: Vec<&str>| {
        v.sort_unstable();
        v.into_iter()
            .map(|aid| json!({"artifact": aid}))
            .collect::<Vec<_>>()
    };
    let set = host.join(&[a.as_str(), b.as_str()], None).await.unwrap();
    assert_eq!(set["clax"], json!(sorted(vec![a.as_str(), b.as_str()])));
    let set = host
        .join(&[c.as_str()], Some(vec!["desk.lamps".into()]))
        .await
        .unwrap();
    assert_eq!(
        set["clax"],
        json!([{"artifact": c, "allow": ["desk.lamps"]}])
    );
    let only_c = json!({"clax": [{"artifact": c, "allow": ["desk.lamps"]}]});
    assert_eq!(d.listed().await["surfaces"][0]["joins"], only_c);
    // Going on as its registration keeps them.
    let reg = host.registration().unwrap().to_string();
    host.register().await.expect("it goes on");
    assert_eq!(host.registration(), Some(reg.as_str()));
    assert_eq!(d.listed().await["surfaces"][0]["joins"], only_c);
    let kept: Vec<Join> = d.ts.store.joins_of("desk").unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].registration_id, reg);
    // Deleting the artifact removes its join.
    let r =
        d.ts.client
            .delete(format!("{}/api/artifacts/{c}", d.ts.base))
            .bearer_auth(&d.ts.token)
            .send()
            .await
            .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    assert_eq!(
        d.listed().await["surfaces"][0]["joins"],
        json!({"clax": []})
    );
    // An empty set leaves none.
    host.join(&[a.as_str()], None).await.unwrap();
    assert_eq!(host.join(&[], None).await.unwrap()["clax"], json!([]));
    assert!(d.ts.store.joins_of("desk").unwrap().is_empty());
}

/// What a `PUT …/joins` refuses, each with its code and status, and
/// changes nothing for: another person's artifact, one that is not here,
/// an ID that is none, an artifact named twice, more than the limit, an
/// allow pattern that is none, a registration the hub does not hold, and
/// any credential but the daemon's token.
#[tokio::test]
async fn a_join_is_refused_for_another_persons_artifact_and_what_is_not_one() {
    let ts = TestServer::spawn().await;
    let rae = ts.person("Rae").await;
    let r = ts
        .client
        .post(format!("{}/api/artifacts:new", ts.base))
        .header("cookie", format!("clax_viewer={}", rae.cookie))
        .json(&json!({"title": "Rae's chart", "brief": "A chart of Q3 revenue by region"}))
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(r).await;
    assert_eq!(s, 201, "{v}");
    let theirs = v["artifact"]["id"].as_str().unwrap().to_string();
    let mine = ts.publish("Mine", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut host = FakeHost::new(HttpTransport::of(&ts), "desk", Notation::Dts, &desk_file());
    host.register().await.unwrap();
    host.join(&[mine.as_str()], None).await.unwrap();
    let reg = host.registration().unwrap().to_string();
    let put = |body: Value| {
        let ts = &ts;
        async move {
            let r = ts
                .client
                .put(format!("{}/api/bot/surfaces/desk/joins", ts.base))
                .bearer_auth(&ts.token)
                .json(&body)
                .send()
                .await
                .unwrap();
            json_of(r).await
        }
    };
    let gone = ts.publish("Gone", &[("index.html", "<p>x</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .client
        .delete(format!("{}/api/artifacts/{gone}", ts.base))
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let scopes = |ids: &[&str]| {
        ids.iter()
            .map(|a| json!({"artifact": a}))
            .collect::<Vec<_>>()
    };
    let many: Vec<Value> = (0..=256)
        .map(|n| json!({"artifact": format!("{n:012}")}))
        .collect();
    for (body, status, want) in [
        (
            json!({"registration": reg, "clax": scopes(&[&mine, &theirs])}),
            403,
            "join_artifact_refused",
        ),
        (
            json!({"registration": reg, "clax": scopes(&[&gone])}),
            404,
            "join_artifact_unknown",
        ),
        (
            json!({"registration": reg, "clax": scopes(&["000000000000"])}),
            404,
            "join_artifact_unknown",
        ),
        (
            json!({"registration": reg, "clax": scopes(&["not an id"])}),
            404,
            "join_artifact_unknown",
        ),
        (
            json!({"registration": reg, "clax": scopes(&[&mine, &mine])}),
            400,
            "invalid_request",
        ),
        (
            json!({"registration": reg, "clax": many}),
            400,
            "invalid_request",
        ),
        (
            json!({"registration": reg, "clax": [{"artifact": mine, "allow": ["desk..x"]}]}),
            400,
            "invalid_request",
        ),
        (
            json!({"registration": reg, "clax": [{"artifact": mine, "allow": ["clax"]}]}),
            400,
            "invalid_request",
        ),
        // `null` is not "left out".
        (
            json!({"registration": reg, "clax": [{"artifact": mine, "allow": null}]}),
            400,
            "invalid_request",
        ),
        (
            json!({"registration": reg, "clax": [], "shell": []}),
            400,
            "invalid_request",
        ),
        (
            json!({"registration": "0".repeat(32), "clax": []}),
            404,
            "unknown_registration",
        ),
    ] {
        let (s, v) = put(body.clone()).await;
        assert_eq!((s, code(&v)), (status, want), "{body}: {v}");
        let kept = ts.store.joins_of("desk").unwrap();
        assert_eq!(kept.len(), 1, "{body}");
        assert_eq!(kept[0].artifact_id, mine);
    }
    // The owner's browser is not the token.
    let r = ts
        .client
        .put(format!("{}/api/bot/surfaces/desk/joins", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"registration": reg, "clax": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(ts.store.joins_of("desk").unwrap().len(), 1);
}

/// Whatever binds, a join binds into the runs of its registrant's person
/// alone, never into one an agent sent under a grant, nothing is asked
/// of a host that keeps no hub, and none for an artifact with no join.
#[tokio::test]
async fn a_join_binds_into_its_registrants_runs_alone() {
    struct Always;
    impl clax_http::bot_joins::JoinedSurfaces for Always {
        fn joined(&self, joins: &[Join], _: &Value) -> Vec<StoredBinding> {
            joins
                .iter()
                .map(|j| {
                    StoredBinding::bound(
                        j.surface.as_str(),
                        Some("h".into()),
                        Some(j.registration_id.clone()),
                        None,
                        Value::Null,
                        vec!["desk.lamps".into()],
                    )
                })
                .collect()
        }
    }
    let ts = TestServer::spawn().await;
    let mine = ts.publish("Mine", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other = ts.publish("Other", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut host = FakeHost::new(HttpTransport::of(&ts), "desk", Notation::Dts, &desk_file());
    host.register().await.unwrap();
    host.join(&[mine.as_str()], None).await.unwrap();
    let with = clax_http::bot::Bot {
        joins: Some(Arc::new(Always)),
        ..Default::default()
    };
    let without = clax_http::bot::Bot::default();
    let as_sent = |bot: &clax_http::bot::Bot, sender: &Sender, aid: &str| {
        clax_server::routes::bot_joins::joined_bindings(&ts.store, bot, sender, aid, "", &[])
            .unwrap()
    };
    let by = |person: &str| Sender {
        person: person.to_string(),
        agent_grant: None,
    };
    let bindings =
        |bot: &clax_http::bot::Bot, person: &str, aid: &str| as_sent(bot, &by(person), aid);
    let owner = clax_store::roles::OWNER;
    let made = bindings(&with, owner, &mine).expect("the owner's run is joined");
    let names: Vec<&str> = made.all().iter().map(StoredBinding::surface).collect();
    assert_eq!(names, ["clax", "desk"]);
    assert_eq!(bindings(&with, "p_someone", &mine), None);
    assert_eq!(bindings(&with, owner, &other), None);
    assert_eq!(bindings(&without, owner, &mine), None);
    // A run an agent of another machine sent for the owner, under a
    // grant, is joined by none.
    let granted = Sender {
        person: owner.to_string(),
        agent_grant: Some("g_1".into()),
    };
    assert_eq!(as_sent(&with, &granted, &mine), None);
}

/// A join binds into a run whose threads hold the comments of the person
/// it acts for (and ClaxBot's replies) alone: a thread where anyone else
/// commented, before the run or before a follow-up, gives a run with no
/// join, made as one with none is.
#[tokio::test]
async fn a_join_binds_into_no_run_whose_threads_hold_anyone_elses_comment() {
    let fake = FakeMessages::start(vec![Turn::finish("Done."), Turn::finish("Done.")]).await;
    let d = daemon(&fake).await;
    let mut host = d.desk().await;
    let rae = d.ts.person("Rae").await;
    let names = |run: &str| -> Vec<String> {
        d.bindings(run)
            .all()
            .iter()
            .map(|b| b.surface().to_string())
            .collect()
    };
    let unjoined = |run: &str, aid: &str| {
        let web = d.ts.store.run(run).unwrap().unwrap().web;
        assert_eq!(d.bindings(run), Bindings::clax(aid, &web), "{run}");
    };
    // Two threads ClaxBot answered before any join.
    let (answered, answered_tid) = d.page().await;
    let (guested_later, later_tid) = d.page().await;
    let firsts = [
        d.send(&answered, &answered_tid).await,
        d.send(&guested_later, &later_tid).await,
    ];
    d.driver.run_until_idle(BUDGET).await;
    for run in &firsts {
        assert_eq!(
            d.ts.store.run(run).unwrap().unwrap().status,
            clax_store::bot_runs::RunStatus::Finished
        );
    }
    let follow_up = |aid: String, tid: String| {
        let d = &d;
        async move {
            d.owner_says(&aid, &tid, "One more thing.").await["run"]["id"]
                .as_str()
                .expect("a follow-up run")
                .to_string()
        }
    };
    host.join(&[answered.as_str(), guested_later.as_str()], None)
        .await
        .unwrap();
    // The owner's comments and ClaxBot's reply alone: the follow-up is
    // joined.
    let thread = d.ts.store.get_thread(&answered_tid).unwrap().unwrap();
    assert_eq!(thread.comments.len(), 2, "the request and ClaxBot's reply");
    let run = follow_up(answered.clone(), answered_tid).await;
    assert_eq!(names(&run), ["clax", "desk"]);
    // A guest commented after ClaxBot answered: the follow-up is not.
    d.ts.reply_as(
        &guested_later,
        &later_tid,
        &rae.cookie,
        "Turn every lamp off.",
    )
    .await;
    let run = follow_up(guested_later.clone(), later_tid).await;
    unjoined(&run, &guested_later);

    // A thread with the owner's comments alone: joined.
    let (mine, tid) = d.page().await;
    d.owner_says(&mine, &tid, "And the desk lamp?").await;
    host.join(&[mine.as_str()], None).await.unwrap();
    assert_eq!(names(&d.send(&mine, &tid).await), ["clax", "desk"]);
    // A guest's comment in the thread before it is sent: no join.
    let (guested, tid) = d.page().await;
    d.ts.reply_as(
        &guested,
        &tid,
        &rae.cookie,
        "Also read me the desk's settings.",
    )
    .await;
    host.join(&[guested.as_str()], None).await.unwrap();
    unjoined(&d.send(&guested, &tid).await, &guested);
}

/// How long a script waits for a call that may wait before its run parks
/// (`clax_bot::Delivery::DAEMON`).
const PARK_AFTER: Duration = Duration::from_secs(10);

/// A joined thread run that parks on the host's question: its owner sees
/// what it waits on, and a reader of the artifact sees that it waits and
/// since when, and neither the surface nor the operation, in the run,
/// the listing and the events' `run`.
#[tokio::test]
async fn a_reader_of_a_parked_joined_run_sees_it_waits_and_not_on_what() {
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Ask about the lamp",
            "const lamps = await clax.desk.lamps(); \
             return [lamps.length, await clax.desk.ask(\"Move the left lamp?\")];",
        ),
        Turn::finish("Asked."),
    ])
    .await;
    let d = daemon(&fake).await;
    let (aid, tid) = d.page().await;
    let mut host = d.desk().await.on(
        "ask",
        Behaviour::Accept {
            when: When::AfterMs(1),
            then: Outcome::Result(json!("yes")),
        },
    );
    host.join(
        &[aid.as_str()],
        Some(vec!["desk.lamps".into(), "desk.ask".into()]),
    )
    .await
    .expect("the desk joins");
    let run = d.send(&aid, &tid).await;
    let ticking = {
        let driver = d.driver.clone();
        tokio::spawn(async move { driver.run_until_idle(BUDGET).await })
    };
    // The read, answered; then the question, accepted and not answered.
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.len() == 1));
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.len() == 1));
    let mut host = host.wait_ms(0);
    assert!(matches!(host.serve_once().await, Served::Delivered(c) if c.is_empty()));
    d.sleeper.wait_for(PARK_AFTER, 1).await;
    d.clock.advance_millis(10_000);
    d.sleeper.fire(PARK_AFTER);
    ticking.await.unwrap();

    let owner = d.ts.owner_cookie();
    let (s, mine) = d.get(&format!("/api/bot/runs/{run}"), &owner).await;
    assert_eq!(s, 200, "{mine}");
    assert_eq!(mine["status"], "parked", "{mine}");
    assert_eq!(mine["waiting_on"]["surface"], "desk");
    assert_eq!(mine["waiting_on"]["op"], "desk.ask");
    let since = mine["waiting_on"]["since"].clone();
    assert!(since.is_string(), "{mine}");

    let rae = d.ts.person("Rae").await;
    let theirs = format!("clax_viewer={}", rae.cookie);
    let bare = json!({"since": since, "deadline": mine["waiting_on"]["deadline"]});
    let (s, run_v) = d.get(&format!("/api/bot/runs/{run}"), &theirs).await;
    assert_eq!(s, 200, "{run_v}");
    assert_eq!(run_v["status"], "parked");
    assert_eq!(run_v["waiting_on"], bare, "{run_v}");
    let (s, listed) = d
        .get(&format!("/api/bot/runs?artifact={aid}"), &theirs)
        .await;
    assert_eq!(s, 200, "{listed}");
    assert_eq!(listed["runs"][0]["id"], run.as_str());
    assert_eq!(listed["runs"][0]["waiting_on"], bare, "{listed}");
    for (who, v) in [("Rae", &run_v), ("Rae's list", &listed)] {
        let text = v.to_string();
        assert!(!text.contains("desk"), "{who}: {v}");
    }
    // The owner's listing names what it waits on.
    let (_, all) = d
        .get(&format!("/api/bot/runs?artifact={aid}"), &owner)
        .await;
    assert_eq!(all["runs"][0]["waiting_on"]["op"], "desk.ask");
    // The events' `run`, as the run is now: parked.
    let body = events_until(&d, &run, &theirs, "event: step_done").await;
    assert!(body.contains("\"parked\""), "{body}");
    assert!(!body.contains("desk"), "{body}");
    let body = events_until(&d, &run, &owner, "event: step_done").await;
    assert!(body.contains("desk.ask"), "{body}");
}

/// A join with no `allow` binds the surface's reads that do not wait
/// alone; its writes, effects and reads that wait are bound only when the
/// join's own `allow` names them, by name or by a pattern the join holds,
/// and never past the registration's `allow`.
#[tokio::test]
async fn a_join_binds_a_write_or_an_effect_only_when_it_names_it() {
    let fake = FakeMessages::start(vec![]).await;
    let d = daemon(&fake).await;
    let mut host = d.desk().await;
    let ops_of = |run: &str| -> Vec<String> {
        d.bindings(run)
            .of("desk")
            .map(|b| b.operations().unwrap().to_vec())
            .unwrap_or_default()
    };
    for (allow, want) in [
        // The reads, the image among them on a daemon that keeps
        // content.
        (None, vec!["desk.lamps", "desk.snapshot"]),
        (Some(vec!["desk.lamps".into()]), vec!["desk.lamps"]),
        (Some(vec!["desk.notify".into()]), vec!["desk.notify"]),
        (
            Some(vec!["desk.*".into()]),
            vec![
                "desk.lamps",
                "desk.snapshot",
                "desk.settings.set",
                "desk.notify",
                "desk.ask",
            ],
        ),
    ] {
        // A page of its own each time: a thread has one run at a time.
        let (aid, tid) = d.page().await;
        host.join(&[aid.as_str()], allow.clone()).await.unwrap();
        let run = d.send(&aid, &tid).await;
        assert_eq!(ops_of(&run), want, "{allow:?}");
    }
}

/// A daemon's driver joins its runs through its hub; a hosted daemon's,
/// which serves no host route, keeps no join and reads none at a run's
/// start (`clax_http::bot::Bot::joins` is `None`).
#[tokio::test]
async fn a_hosted_daemon_joins_no_run() {
    let local = TestServer::spawn_with(|state| {
        clax_server::bot::driver(state);
    })
    .await;
    assert!(local.state.bot.joins.is_some());
    let hosted = TestServer::spawn_hosted_with(Arc::new(ManualClock::at(START)), None, |state| {
        clax_server::bot::driver(state);
    })
    .await;
    assert!(hosted.state.hosted.is_some());
    assert!(hosted.state.bot.joins.is_none());
}

/// What a test's log subscriber writes.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// The joins a run cannot take are logged, and the run is made all the
/// same: past the seven surfaces a run joins, those left out are named;
/// a set that makes no bindings (two of one surface) is said, and the
/// run is made with no join, as a run with none is.
#[test]
fn joins_a_run_cannot_take_are_logged_and_the_run_is_made_without_them() {
    use tracing_subscriber::util::SubscriberInitExt;
    let log = Log::default();
    let writer = log.clone();
    let _logging = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .set_default();
    let binding = |surface: &str, n: u32| {
        StoredBinding::bound(
            surface,
            None,
            Some(format!("r{n}")),
            None,
            Value::Null,
            vec![],
        )
    };
    let many: Vec<StoredBinding> = (0..9).map(|n| binding(&format!("s{n}"), n)).collect();
    let made = clax_bot::joins::bindings_with("a1", "", many).expect("seven are bound");
    assert_eq!(made.all().len(), 8);
    let text = log.text();
    assert!(
        text.contains("s7 (r7)") && text.contains("s8 (r8)") && !text.contains("s6 (r6)"),
        "{text}"
    );
    let twice = vec![binding("desk", 1), binding("desk", 2)];
    assert_eq!(clax_bot::joins::bindings_with("a1", "", twice), None);
    let text = log.text();
    assert!(
        text.contains("the joins on a1 make no bindings") && text.contains("with no join"),
        "{text}"
    );
}

/// Only the registration that started a run may send it messages or
/// read its state: a host joined into a run of Clax's own is answered
/// 404 `unknown_run` for both, as for a run that is not there, where
/// the starter of an ended run is told `run_ended`. It is delivered no
/// event of the run, only its end.
#[tokio::test]
async fn a_joining_host_may_not_message_or_read_a_run_it_joined_and_is_told_its_end_alone() {
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Count the lamps",
            "return (await clax.desk.lamps()).length;",
        ),
        Turn::finish("Two lamps."),
    ])
    .await;
    let d = daemon(&fake).await;
    let (aid, tid) = d.page().await;
    let mut host = d.desk().await;
    host.join(&[aid.as_str()], None)
        .await
        .expect("the desk joins");
    let run = d.send(&aid, &tid).await;
    let ticking = {
        let driver = d.driver.clone();
        tokio::spawn(async move { driver.run_until_idle(BUDGET).await })
    };
    host.serve_until(|h| !h.ended().is_empty(), 8)
        .await
        .expect("the host is told the run ended");
    ticking.await.unwrap();
    assert_eq!(host.calls_of("lamps").len(), 1, "the run called the host");
    assert_eq!(host.ended()[0].run, run);
    assert!(
        host.events().iter().all(|e| e.run != run),
        "no event of a run it joined: {:?}",
        host.events()
    );
    let (status, body) = host
        .send_message(&run, json!({"say": "Skip the drafts."}), None)
        .await
        .expect_err("a joining host sends no message");
    assert_eq!((status, code(&body)), (404, "unknown_run"), "{body}");
    let (status, body) = host
        .run_state(&run)
        .await
        .expect_err("a joining host reads no run state");
    assert_eq!((status, code(&body)), (404, "unknown_run"), "{body}");
}

/// A surface of a camera: one operation answers an image, one takes one.
const LENS: &str = r#"/** A camera and a shelf of pictures. */
declare const lens: {
  /** A picture of what the camera sees. @read */
  look(): Promise<Image>;
  /** Keeps a picture on the shelf, with a note. @write */
  keep(picture: Image, note: string): Promise<null>;
};
"#;

/// The head of a PNG of 2 by 1 pixels: what Clax reads an image's type
/// and size from.
fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    b.extend_from_slice(&2u32.to_be_bytes());
    b.extend_from_slice(&1u32.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut c = 0xffff_ffffu32;
    for &byte in &b[12..29] {
        c ^= u32::from(byte);
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    b.extend_from_slice(&(!c).to_be_bytes());
    b
}

/// A joined surface's operations that hold content are bound on a
/// daemon that keeps content, and its host's content works in a run of
/// Clax's own as in a run it started: the image `look` uploads reaches
/// the script whole, `keep` is sent it, and the host may then read its
/// bytes for the run, a read the run's journal grants.
#[tokio::test]
async fn a_joined_hosts_content_is_passed_and_read_in_a_run_of_claxs_own() {
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Look and keep",
            r#"const p = await clax.lens.look(); await clax.lens.keep(p, "kept"); return p.media_type;"#,
        ),
        Turn::finish("Kept a picture."),
    ])
    .await;
    let d = daemon(&fake).await;
    let (aid, tid) = d.page().await;
    let picture = png();
    let mut host = FakeHost::new(HttpTransport::of(&d.ts), "lens", Notation::Dts, LENS)
        .on("look", Behaviour::upload("image/png", picture.clone()))
        .on("keep", Behaviour::result(Value::Null))
        .wait_ms(5_000);
    let registered = host.register().await.expect("lens registers");
    assert!(registered.withheld.is_empty(), "{registered:?}");
    host.join(&[aid.as_str()], Some(vec!["lens.*".into()]))
        .await
        .expect("the lens joins");
    let run = d.send(&aid, &tid).await;
    assert_eq!(
        d.bindings(&run).of("lens").unwrap().operations().unwrap(),
        ["lens.look", "lens.keep"]
    );
    let ticking = {
        let driver = d.driver.clone();
        tokio::spawn(async move { driver.run_until_idle(BUDGET).await })
    };
    host.serve_until(|h| !h.ended().is_empty(), 16)
        .await
        .expect("the host is told the run ended");
    ticking.await.unwrap();
    let (status, handle) = host.uploads()[0].clone();
    assert_eq!(status, 200, "{handle}");
    let id = handle["id"].as_str().unwrap().to_string();
    let kept = host.calls_of("keep");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].args, vec![handle.clone(), json!("kept")]);
    assert!(kept[0].joined.is_some(), "{:?}", kept[0]);
    let (_, steps) = d.ts.store.run_with_steps(&run).unwrap().unwrap();
    let script = steps
        .iter()
        .find(|s| s.kind == clax_store::bot_runs::StepKind::Script)
        .unwrap();
    assert_eq!(
        script.payload.as_ref().unwrap()["outcome"]["value"],
        json!("image/png")
    );
    let (status, bytes) = host.download(&run, &id).await;
    assert_eq!((status, bytes), (200, picture));
}
