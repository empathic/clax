//! ClaxBot runs end to end on a daemon (spec
//! docs/superpowers/specs/2026-10-21-claxbot-design.md §4, §10): a thread
//! sent to ClaxBot runs on the daemon's driver against a scripted
//! Messages API, its steps show in `GET /api/bot/runs/<id>`, and its
//! events stream with IDs `<step>.<attempt>:<seq>`, replayed after a
//! `Last-Event-ID`.

use clax_core::audit::AuditCtx;
use clax_store::bot_settings::Setting;
use clax_testkit::TestServer;
use clax_testkit::fake_messages::{FakeMessages, Turn};
use futures::StreamExt;
use serde_json::{Value, json};

const KEY: &str = "sk-ant-api03-FAKEKEYFORTESTS-wxyz";

/// A daemon driving runs against `fake`, its instance key set.
async fn daemon(fake: &FakeMessages) -> TestServer {
    let ts = TestServer::spawn_with(|s| {
        let d = clax_server::bot::driver(s);
        clax_server::bot::start_driver(&d);
    })
    .await;
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::BaseUrl, Some(json!(fake.base)))],
        )
        .unwrap();
    let res = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    ts
}

/// The events of run `rid` after `after`, read until `run_end`: (ID,
/// event, data).
async fn events(ts: &TestServer, rid: &str, after: Option<&str>) -> Vec<(String, String, Value)> {
    let mut req = ts
        .client
        .get(format!("{}/api/bot/runs/{rid}/events", ts.base))
        .header("cookie", ts.owner_cookie());
    if let Some(a) = after {
        req = req.header("last-event-id", a);
    }
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut body = res.bytes_stream();
    let (mut buf, mut out) = (String::new(), Vec::new());
    let read = async {
        while let Some(chunk) = body.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            while let Some(end) = buf.find("\n\n") {
                let frame = buf[..end].to_string();
                buf.drain(..end + 2);
                let (mut id, mut ev, mut data) = (String::new(), String::new(), Value::Null);
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("id: ") {
                        id = v.to_string();
                    } else if let Some(v) = line.strip_prefix("event: ") {
                        ev = v.to_string();
                    } else if let Some(v) = line.strip_prefix("data: ") {
                        data = serde_json::from_str(v).unwrap();
                    }
                }
                if ev.is_empty() {
                    continue;
                }
                let end = ev == "run_end";
                out.push((id, ev, data));
                if end {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(clax_testkit::HANG, read)
        .await
        .expect("the run ended");
    out
}

/// `(step, attempt, seq)` of an event ID.
fn id_of(id: &str) -> (i64, i64, i64) {
    let (sa, seq) = id.split_once(':').unwrap();
    let (s, a) = sa.split_once('.').unwrap();
    (s.parse().unwrap(), a.parse().unwrap(), seq.parse().unwrap())
}

#[tokio::test]
async fn a_thread_sent_to_claxbot_runs_streams_and_replays() {
    let fake = FakeMessages::start(vec![
        Turn::script(
            "tu_1",
            "Look at the page",
            "const a = await clax.artifact.get(); console.log(a.title); return a.version;",
        ),
        Turn::finish("The page is titled Notes, at version 1."),
    ])
    .await;
    let ts = daemon(&fake).await;
    let aid = ts.publish("Notes", &[("index.html", "<p>hi</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread(&aid, 1, "What is this page?").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .client
        .post(format!("{}/api/bot/runs", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let run: Value = res.json().await.unwrap();
    let rid = run["run"]["id"].as_str().unwrap().to_string();

    let all = events(&ts, &rid, None).await;
    let names: Vec<&str> = all.iter().map(|(_, e, _)| e.as_str()).collect();
    assert_eq!(names.first(), Some(&"step_started"), "{names:?}");
    assert_eq!(names.last(), Some(&"run_end"), "{names:?}");
    assert!(
        names.contains(&"script") && names.contains(&"call"),
        "{names:?}"
    );
    // IDs never repeat and order by (step, attempt, seq).
    let ids: Vec<(i64, i64, i64)> = all.iter().map(|(i, _, _)| id_of(i)).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(ids, sorted);
    // The shapes the run panel reads.
    let started = &all[0].2;
    assert_eq!(
        (&started["step"]["n"], &started["step"]["status"]),
        (&json!(1), &json!("running"))
    );
    let call = all.iter().find(|(_, e, _)| e == "call").unwrap();
    assert_eq!(call.2["call"]["op"], "artifact.get");
    assert_eq!(call.2["call"]["ok"], true);
    assert!(call.2["call"]["ms"].is_u64(), "{}", call.2);
    let done: Vec<&Value> = all
        .iter()
        .filter(|(_, e, _)| e == "step_done")
        .map(|(_, _, d)| d)
        .collect();
    let script = done.iter().find(|d| d["step"]["kind"] == "script").unwrap();
    assert_eq!(script["step"]["title"], "Look at the page");
    assert_eq!(script["step"]["value"], 1);
    assert_eq!(script["step"]["logs"], json!(["Notes"]));
    let end = &all.last().unwrap().2;
    assert_eq!(end["run"]["status"], "finished", "{end}");

    // The run as GET shows it.
    let r: Value = ts
        .client
        .get(format!("{}/api/bot/runs/{rid}", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let kinds: Vec<&str> = r["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["turn", "script", "turn", "finish"], "{r}");
    assert_eq!(r["steps"][0]["text"], "Running Look at the page.");
    assert_eq!(r["steps"][0]["model"], "claude-sonnet-5-5");
    assert_eq!(
        r["steps"][3]["summary"],
        "The page is titled Notes, at version 1."
    );
    assert!(r["credits_milli"].as_i64().unwrap() > 0, "{r}");
    assert!(r["tokens"]["input"].as_i64().unwrap() > 0, "{r}");
    assert_eq!(
        r["last_event_id"].as_str(),
        Some(all.last().unwrap().0.as_str())
    );
    assert_eq!(fake.left(), 0);
    // The model was sent the key, and only to the configured URL.
    assert_eq!(fake.requests().len(), 2);

    // A reader that reconnects after the first step's end hears the rest once.
    let first_done = all
        .iter()
        .find(|(_, e, _)| e == "step_done")
        .unwrap()
        .0
        .clone();
    let rest = events(&ts, &rid, Some(&first_done)).await;
    assert!(
        rest.iter().all(|(i, _, _)| id_of(i) > id_of(&first_done)),
        "{rest:?}"
    );
    assert_eq!(rest.last().unwrap().1, "run_end");
    let kinds_done: Vec<&str> = rest
        .iter()
        .filter(|(_, e, _)| e == "step_done")
        .map(|(_, _, d)| d["step"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds_done, ["script", "turn", "finish"]);

    // Its audit trail: the run, its turns, its script and its call.
    let kinds: Vec<String> = ts
        .store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("bot."))
        .collect();
    for k in [
        "bot.run_start",
        "bot.turn",
        "bot.script",
        "bot.call",
        "bot.run_end",
    ] {
        assert!(kinds.iter().any(|x| x == k), "{k} missing from {kinds:?}");
    }

    // While a run works its threads show ClaxBot working; once it ends, not.
    assert!(ts.working.for_artifact(&aid).is_empty());
    let row = ts.store.run(&rid).unwrap().unwrap();
    clax_server::bot::show_working(&ts.state, &row, true);
    let w = ts.working.for_artifact(&aid);
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].agent, "claxbot");
    clax_server::bot::show_working(&ts.state, &row, false);
    assert!(ts.working.for_artifact(&aid).is_empty());
}

#[tokio::test]
async fn the_servers_host_passes_the_bot_host_conformance_suite() {
    use clax_bot::host_conformance::{HostFixture, check_host};
    use clax_store::bot_runs::{NewRun, RequestKind};
    use clax_store::credits::KeySource;
    use clax_store::flags::Flag;
    let ts = TestServer::spawn().await;
    let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
    let aid = id_of(&ts.publish("Host", &[("index.html", "<p>hi</p>")]).await["artifact"]);
    let other = id_of(&ts.publish("Other", &[("index.html", "<p>x</p>")]).await["artifact"]);
    let inside = id_of(&ts.thread(&aid, 1, "Please fix the title.").await);
    let outside = id_of(&ts.thread(&aid, 1, "Not sent to ClaxBot.").await);
    let foreign = id_of(&ts.thread(&other, 1, "Another artifact's.").await);
    let rae = ts.person("Rae").await.public_id;
    let vic = ts.person("Vic").await.public_id;
    let st = ts.store.clone();
    let (a, i, r, v) = (aid.clone(), inside.clone(), rae.clone(), vic.clone());
    let (run, brief_run, reader_run, revoked_run) = tokio::task::spawn_blocking(move || {
        let read_only = clax_core::rights::Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read"], "locations": ["*"]},
        ]))
        .unwrap();
        st.set_person_rights(&AuditCtx::DAEMON, &r, Some(read_only))
            .unwrap();
        for p in [&r, &v] {
            st.set_flags(&AuditCtx::DAEMON, p, &[(Flag::Claxbot, true)])
                .unwrap();
        }
        let new = |person: &str, request| {
            st.create_run(
                &clax_bot::run::bot_ctx(person),
                NewRun {
                    artifact_id: a.clone(),
                    person: person.to_string(),
                    request,
                    thread_ids: vec![i.clone()],
                    model: "claude-sonnet-5-5".into(),
                    key_source: KeySource::Instance,
                    base_version: 1,
                    agent_grant: None,
                },
            )
            .unwrap()
        };
        (
            new("*owner", RequestKind::Thread),
            new("*owner", RequestKind::Brief),
            new(&r, RequestKind::Thread),
            new(&v, RequestKind::Thread),
        )
    })
    .await
    .unwrap();
    let st = ts.store.clone();
    let fixture = HostFixture {
        host: std::sync::Arc::new(clax_server::bot::ServerHost::new(ts.state.clone())),
        store: ts.store.clone(),
        run,
        brief_run,
        thread_outside_run: outside,
        foreign_thread: foreign,
        reader_run: Some(reader_run),
        revoked_run,
        // Between plan and run: the person loses ClaxBot.
        revoke: Box::new(move |person| {
            st.set_flags(&AuditCtx::DAEMON, person, &[(Flag::Claxbot, false)])
                .unwrap();
        }),
    };
    let results = check_host(&fixture).await;
    assert!(results.len() >= 9, "{}", results.len());
    for (name, result) in results {
        result.unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // The replies are ClaxBot's, acting for the owner, on the run's thread.
    let t = ts.store.get_thread(&inside).unwrap().unwrap();
    let reply = t
        .comments
        .iter()
        .find(|c| c.author_kind == "agent")
        .expect("a reply");
    assert!(
        reply.author_name.starts_with("ClaxBot"),
        "{}",
        reply.author_name
    );
}

#[tokio::test]
async fn a_run_edits_publishes_and_replies_as_claxbot_for_its_person() {
    let fake = FakeMessages::start(vec![]).await;
    let ts = daemon(&fake).await;
    let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
    let aid = id_of(&ts.publish("Greeting", &[("index.html", "<p>hi</p>")]).await["artifact"]);
    let tid = id_of(&ts.thread(&aid, 1, "Say hello instead.").await);
    let code = format!(
        "await clax.files.replace('index.html', 'hi', 'hello');
         const p = await clax.draft.publish({{note: 'Says hello', addresses: ['{tid}']}});
         await clax.threads.reply('{tid}', 'Changed it to say hello.');
         return p.version;"
    );
    // The fake answers in order: the edit, then the finish.
    let fake = FakeMessages::start(vec![
        Turn::script("tu_1", "Say hello", &code),
        Turn::finish("Changed the greeting."),
    ])
    .await;
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::BaseUrl, Some(json!(fake.base)))],
        )
        .unwrap();
    // A new base URL drops the stored key: the admin enters it again.
    let res = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/send",
            ts.base
        ))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"to": "claxbot"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    let rid = v["run"]["id"].as_str().unwrap().to_string();
    let all = events(&ts, &rid, None).await;
    let end = &all.last().unwrap().2;
    assert_eq!(end["run"]["status"], "finished", "{end}");
    // The artifact is at version 2, with the edit.
    let a = ts
        .store
        .get_artifact(&clax_core::ArtifactId::parse(&aid).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(a.current_version, 2);
    let (bytes, _) = ts
        .store
        .read_file_async(
            &clax_core::ArtifactId::parse(&aid).unwrap(),
            2,
            "index.html",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "<p>hello</p>");
    // The reply is ClaxBot's, acting for the owner, once.
    let t = ts.store.get_thread(&tid).unwrap().unwrap();
    let replies: Vec<_> = t
        .comments
        .iter()
        .filter(|c| c.author_kind == "agent")
        .collect();
    assert_eq!(replies.len(), 1, "{:?}", t.comments);
    assert_eq!(replies[0].body, "Changed it to say hello.");
    assert!(replies[0].author_name.starts_with("ClaxBot"));
    // Recorded as ClaxBot acting for the owner.
    let publish = ts
        .store
        .events_after(0, 10_000)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "version.publish" && e.actor.contains("\"bot\""))
        .expect("the publish is recorded");
    assert!(publish.actor.contains("\"bot\""), "{}", publish.actor);
    assert!(ts.working.for_artifact(&aid).is_empty());
}

/// A LAN daemon whose driver starts only when the test says so, against
/// `fake`, its instance key set; and that driver.
async fn held_daemon(fake: &FakeMessages) -> (TestServer, std::sync::Arc<clax_bot::Driver>) {
    let held = std::sync::Arc::new(std::sync::Mutex::new(None));
    let h = held.clone();
    // On the LAN, so people connect by invite.
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), move |s| {
        *h.lock().unwrap() = Some(clax_server::bot::driver(s));
    })
    .await;
    ts.store
        .set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::BaseUrl, Some(json!(fake.base)))],
        )
        .unwrap();
    let res = ts
        .client
        .put(format!("{}/api/admin/provider/key", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"key": KEY}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let d = held.lock().unwrap().take().unwrap();
    (ts, d)
}

#[tokio::test]
async fn an_agents_run_never_exceeds_the_agents_grant() {
    use clax_bot::BotHost;
    use clax_store::flags::Flag;
    let fake = FakeMessages::start(vec![]).await;
    let (ts, _driver) = held_daemon(&fake).await;
    let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
    let aid = id_of(&ts.publish("Shared", &[("index.html", "<p>hi</p>")]).await["artifact"]);
    let tid = id_of(&ts.thread(&aid, 1, "Fix the title.").await);
    // Pat's agent of another machine may read and comment and send, and
    // nothing more, whatever Pat may.
    let c = ts.connect_person("Pat", "clax:full").await;
    let grant = c.grant_id();
    let pat = ts.store.viewer_of_grant(&grant).unwrap().public_id;
    let st = ts.store.clone();
    let (g, p) = (grant.clone(), pat.clone());
    tokio::task::spawn_blocking(move || {
        st.set_flags(&AuditCtx::DAEMON, &p, &[(Flag::Claxbot, true)])
            .unwrap();
        let narrow = clax_core::rights::Rights::parse(&json!([
            {"type": "clax-artifacts", "actions": ["read"], "locations": ["*"]},
            {"type": "clax-comments", "actions": ["read", "write", "reply", "send"], "locations": ["*"]},
        ]))
        .unwrap();
        st.narrow_grant(&AuditCtx::DAEMON, &g, Some(narrow)).unwrap();
    })
    .await
    .unwrap();
    let sid = id_of(&c.register("claude", "pat-1").await);
    let res = c
        .agent(
            "POST",
            &format!("/api/artifacts/{aid}/threads/{tid}/send"),
            Some(&sid),
            Some(json!({"to": "claxbot"})),
        )
        .await;
    let (s, v) = clax_testkit::remotes::json_of(res).await;
    assert_eq!(s, 200, "{v}");
    let run = ts
        .store
        .run(v["run"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(run.person, pat);
    assert_eq!(run.agent_grant.as_deref(), Some(grant.as_str()));
    // Its writes are bounded by the grant: no publish, no document write,
    // no resolve; a reply it may make.
    let host = clax_server::bot::ServerHost::new(ts.state.clone());
    let audit = clax_bot::run::bot_ctx(&pat);
    let cx = clax_bot::OpCtx::new(&run, &audit, 900_000, 1, 0);
    let draft = clax_bot::host::PublishDraft {
        files: vec![clax_bot::host::PublishFile {
            path: "index.html".into(),
            content: b"<p>owned</p>".to_vec(),
            content_type: "text/html".into(),
        }],
        if_version: 1,
        note: None,
        addresses: vec![],
    };
    let code = |r: Result<clax_bot::host::WritePlan<'_>, clax_bot_sandbox::HostError>| match r {
        Ok(_) => "planned".to_string(),
        Err(e) => e.code,
    };
    assert_eq!(code(host.plan_publish(&cx, draft).await), "forbidden");
    assert_eq!(
        code(
            host.plan_write(
                &cx,
                "db.set",
                &[json!("notes"), json!("a"), json!({"x": 1}), json!({})]
            )
            .await
        ),
        "forbidden"
    );
    assert_eq!(
        code(host.plan_write(&cx, "threads.resolve", &[json!(tid)]).await),
        "forbidden"
    );
    assert_eq!(code(host.plan_reply(&cx, &tid, "On it.").await), "planned");
}

/// What `find` sees once it sees something: checked now and again at
/// each event the daemon publishes (a run starting or ending shows on its
/// threads as working), bounded by [`clax_testkit::HANG`]. `rx` is
/// subscribed before the action `find` waits on.
async fn when<T>(
    rx: &mut tokio::sync::broadcast::Receiver<clax_core::events::Stamped>,
    mut find: impl FnMut() -> Option<T>,
) -> T {
    use tokio::sync::broadcast::error::RecvError;
    tokio::time::timeout(clax_testkit::HANG, async {
        loop {
            if let Some(v) = find() {
                return v;
            }
            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the daemon's events ended"),
            }
        }
    })
    .await
    .expect("the condition held")
}

#[tokio::test]
async fn comments_on_a_claxbot_thread_follow_up_on_their_authors_credits_and_rights() {
    use clax_bot::BotHost;
    use clax_store::flags::Flag;
    let fake = FakeMessages::start(vec![
        Turn::finish("First answer."),
        Turn::finish("Answer to the owner's comment."),
        Turn::finish("Answer to Sam."),
        Turn::finish("Answer to Sam again."),
    ])
    .await;
    let (ts, driver) = held_daemon(&fake).await;
    let mut bus = ts.state.events.subscribe();
    let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
    let aid = id_of(&ts.publish("Thread", &[("index.html", "<p>hi</p>")]).await["artifact"]);
    let owner = ts.owner_cookie();
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("cookie", &owner)
        .multipart(
            reqwest::multipart::Form::new()
                .text("anchor", clax_testkit::element_anchor().to_string())
                .text("body", "Make it blue.")
                .text("version", "1"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let tid = id_of(&res.json::<Value>().await.unwrap()["thread"]);
    let comment = |cookie: String, body: &'static str| {
        let (client, url) = (
            ts.client.clone(),
            format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base),
        );
        async move {
            let res = client
                .post(url)
                .header("cookie", cookie)
                .json(&json!({"body": body}))
                .send()
                .await
                .unwrap();
            let (s, v) = clax_testkit::remotes::json_of(res).await;
            assert_eq!(s, 201, "{v}");
            v
        }
    };
    let sam = ts.person("Sam").await;
    let st = ts.store.clone();
    let p = sam.public_id.clone();
    tokio::task::spawn_blocking(move || {
        st.set_flags(&AuditCtx::DAEMON, &p, &[(Flag::Claxbot, true)])
            .unwrap()
    })
    .await
    .unwrap();
    let sam_cookie = format!("clax_viewer={}", sam.cookie);
    let lou = ts.viewer(Some("Lou")).await;
    // The send makes a run.
    let res = ts
        .client
        .post(format!("{}/api/bot/runs", ts.base))
        .header("cookie", &owner)
        .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let first = res.json::<Value>().await.unwrap()["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A second send of the thread while it works: 409 run_active, no run.
    let res = ts
        .client
        .post(format!("{}/api/bot/runs", ts.base))
        .header("cookie", &owner)
        .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
        .send()
        .await
        .unwrap();
    let (s, v) = clax_testkit::remotes::json_of(res).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("run_active")),
        "{v}"
    );
    assert_eq!(v["error"]["run"]["id"], first);
    // While it works: the owner, Sam, a LAN guest and the owner again.
    // The two people are queued once each, in arrival order; the guest,
    // who may not send, is not.
    let queued = json!({"id": first, "queued": true});
    assert_eq!(
        comment(owner.clone(), "And bold, please.").await["run"],
        queued
    );
    assert_eq!(
        comment(sam_cookie.clone(), "Sam here: also italic.").await["run"],
        queued
    );
    let v = comment(format!("clax_viewer={}", lou.cookie), "Lou: nice.").await;
    assert!(v.get("run").is_none(), "{v}");
    assert_eq!(comment(owner.clone(), "Bolder still.").await["run"], queued);
    assert_eq!(
        ts.store.follow_ups(&tid).unwrap(),
        vec!["*owner".to_string(), sam.public_id.clone()]
    );
    assert_eq!(ts.store.runs_for_thread(&tid, 10).unwrap().len(), 1);
    // Each run's end starts the next queued person's run, acting for them
    // alone, on their own credits.
    clax_server::bot::start_driver(&driver);
    let all = events(&ts, &first, None).await;
    assert_eq!(all.last().unwrap().2["run"]["status"], "finished");
    let runs_after = |n: usize| {
        let st = ts.store.clone();
        let tid = tid.clone();
        move || {
            let mut runs = st.runs_for_thread(&tid, 10).unwrap();
            (runs.len() == n).then(|| runs.remove(0))
        }
    };
    let second = when(&mut bus, runs_after(2)).await;
    assert_eq!(second.person, "*owner");
    let all = events(&ts, &second.id, None).await;
    assert_eq!(all.last().unwrap().2["run"]["status"], "finished");
    let third = when(&mut bus, runs_after(3)).await;
    assert_eq!(third.person, sam.public_id);
    let all = events(&ts, &third.id, None).await;
    assert_eq!(all.last().unwrap().2["run"]["status"], "finished");
    assert!(ts.store.follow_ups(&tid).unwrap().is_empty());
    // Only the person a run acts for speaks in it.
    let host = clax_server::bot::ServerHost::new(ts.state.clone());
    let words = |run: &clax_store::bot_runs::Run| {
        let (host, run) = (&host, run.clone());
        async move {
            let ctx = host.context(&run).await.unwrap();
            ctx.threads[0]
                .comments
                .iter()
                .filter(|c| c.from_sender)
                .map(|c| c.text.clone())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        words(&second).await,
        ["Make it blue.", "And bold, please.", "Bolder still."]
    );
    assert_eq!(words(&third).await, ["Sam here: also italic."]);
    // Each charged its own person.
    let spent = |p: &str| ts.store.week_usage_now(p).unwrap().spent_milli;
    assert!(spent(&sam.public_id) > 0);
    assert!(spent("*owner") > spent(&sam.public_id));
    // Sam comments once no run works on the thread: a run at once.
    let v = comment(sam_cookie, "One more, Sam.").await;
    let fourth = v["run"]["id"].as_str().expect("a follow-up run");
    assert_eq!(ts.store.run(fourth).unwrap().unwrap().person, sam.public_id);
    let all = events(&ts, fourth, None).await;
    assert_eq!(all.last().unwrap().2["run"]["status"], "finished");
}

#[tokio::test]
async fn a_reader_of_the_artifact_sees_a_runs_outline_alone_and_loses_it_with_their_rights() {
    use clax_bot::{Event, EventSink};
    let fake = FakeMessages::start(vec![Turn::script(
        "tu_1",
        "Look",
        "const a = await clax.artifact.get(); console.log('secret ' + a.title); return a.title;",
    ), Turn::finish("Done.")])
    .await;
    let ts = daemon(&fake).await;
    let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
    let aid = id_of(
        &ts.publish("Private notes", &[("index.html", "<p>hi</p>")])
            .await["artifact"],
    );
    let tid = id_of(&ts.thread(&aid, 1, "Read it.").await);
    let res = ts
        .client
        .post(format!("{}/api/bot/runs", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"thread_ids": [tid], "artifact_id": aid}))
        .send()
        .await
        .unwrap();
    let rid = res.json::<Value>().await.unwrap()["run"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = events(&ts, &rid, None).await;
    // A connected person who may read the artifact: the outline only.
    let rae = ts.person("Rae").await;
    let r: Value = ts
        .client
        .get(format!("{}/api/bot/runs/{rid}", ts.base))
        .header("cookie", format!("clax_viewer={}", rae.cookie))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = r.to_string();
    assert!(
        !text.contains("secret") && !text.contains("clax.artifact.get"),
        "{r}"
    );
    assert!(r["steps"][0].get("text").is_none(), "{r}");
    assert_eq!(r["steps"][1]["title"], "Look");
    assert_eq!(r["steps"][1]["calls"][0]["op"], "artifact.get");
    // The owner sees it all.
    let o: Value = ts
        .client
        .get(format!("{}/api/bot/runs/{rid}", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(o["steps"][1]["logs"], json!(["secret Private notes"]));
    // Over the events too.
    let res = ts
        .client
        .get(format!("{}/api/bot/runs/{rid}/events", ts.base))
        .header("cookie", format!("clax_viewer={}", rae.cookie))
        .send()
        .await
        .unwrap();
    let body = tokio::time::timeout(clax_testkit::HANG, res.text())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !body.contains("secret") && !body.contains("clax.artifact.get"),
        "{body}"
    );
    assert!(body.contains("event: run_end"), "{body}");
    // A live stream ends at the next step's end once its reader may no
    // longer read the artifact.
    let st = ts.store.clone();
    let run = st
        .create_run(
            &clax_bot::run::bot_ctx("*owner"),
            clax_store::bot_runs::NewRun {
                artifact_id: aid.clone(),
                person: "*owner".into(),
                request: clax_store::bot_runs::RequestKind::Thread,
                thread_ids: vec![],
                model: "claude-sonnet-5-5".into(),
                key_source: clax_store::credits::KeySource::Instance,
                base_version: 1,
                agent_grant: None,
            },
        )
        .unwrap();
    let res = ts
        .client
        .get(format!("{}/api/bot/runs/{}/events", ts.base, run.id))
        .header("cookie", format!("clax_viewer={}", rae.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut body = res.bytes_stream();
    let hub = ts.state.bot.events.clone();
    hub.emit(
        &run.id,
        1,
        1,
        Event::StepStarted {
            kind: clax_store::bot_runs::StepKind::Turn,
        },
    );
    let first = tokio::time::timeout(clax_testkit::HANG, body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("step_started"));
    let p = rae.public_id.clone();
    let st = ts.store.clone();
    tokio::task::spawn_blocking(move || {
        st.set_person_rights(
            &AuditCtx::DAEMON,
            &p,
            Some(clax_core::rights::Rights::parse(&json!([])).unwrap()),
        )
        .unwrap()
    })
    .await
    .unwrap();
    hub.emit(
        &run.id,
        1,
        1,
        Event::StepDone {
            kind: clax_store::bot_runs::StepKind::Turn,
            credits_milli: Some(0),
            run_credits_milli: 0,
            summary: json!({}),
        },
    );
    let rest = tokio::time::timeout(clax_testkit::HANG, async {
        let mut out = Vec::new();
        while let Some(chunk) = body.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    })
    .await
    .expect("the stream ended");
    assert!(!String::from_utf8_lossy(&rest).contains("step_done"));
}

#[tokio::test]
async fn a_follow_up_is_never_stranded_by_a_run_ending_meanwhile_or_a_crash() {
    use clax_store::flags::Flag;
    let fake = FakeMessages::start(vec![Turn::finish("Answer to Sam.")]).await;
    let (ts, driver) = held_daemon(&fake).await;
    let mut bus = ts.state.events.subscribe();
    let owner = ts.owner_cookie();
    let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
    let aid = id_of(&ts.publish("Thread", &[("index.html", "<p>hi</p>")]).await["artifact"]);
    let (tid, t2, t3) = (
        id_of(&ts.thread(&aid, 1, "Make it blue.").await),
        id_of(&ts.thread(&aid, 1, "Make it red.").await),
        id_of(&ts.thread(&aid, 1, "Make it green.").await),
    );
    let sam = ts.person("Sam").await;
    let st = ts.store.clone();
    let p = sam.public_id.clone();
    tokio::task::spawn_blocking(move || {
        st.set_flags(&AuditCtx::DAEMON, &p, &[(Flag::Claxbot, true)])
            .unwrap()
    })
    .await
    .unwrap();
    let db = ts.home.db_path();
    // A run's end committed, as the driver commits it; what follows the
    // commit (its working(false) and follow-ups) is not run.
    let end = |rid: &str| {
        let c = rusqlite::Connection::open(&db).unwrap();
        c.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        c.execute(
            "UPDATE bot_runs SET status = 'finished', ended_at = updated_at WHERE id = ?1",
            rusqlite::params![rid],
        )
        .unwrap();
    };
    let send = |t: &str| {
        let (client, url, cookie, body) = (
            ts.client.clone(),
            format!("{}/api/bot/runs", ts.base),
            owner.clone(),
            json!({"thread_ids": [t], "artifact_id": aid}),
        );
        async move {
            let res = client
                .post(url)
                .header("cookie", cookie)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 201);
            id_of(&res.json::<Value>().await.unwrap()["run"])
        }
    };
    let comment = |cookie: String, t: &str, body: &'static str| {
        let (client, url) = (
            ts.client.clone(),
            format!("{}/api/artifacts/{aid}/threads/{t}/comments", ts.base),
        );
        async move {
            let res = client
                .post(url)
                .header("cookie", cookie)
                .json(&json!({"body": body}))
                .send()
                .await
                .unwrap();
            let (s, v) = clax_testkit::remotes::json_of(res).await;
            assert_eq!(s, 201, "{v}");
            v
        }
    };
    // A run that ended before the comment is checked: the comment starts
    // its run at once, queueing nothing.
    let first = send(&tid).await;
    end(&first);
    let v = comment(owner.clone(), &tid, "And bold, please.").await;
    let second = v["run"]["id"].as_str().expect("a run at once").to_string();
    assert!(v["run"].get("queued").is_none(), "{v}");
    assert!(ts.store.follow_ups(&tid).unwrap().is_empty());
    // Sam comments while it works, and is queued; it ends, and the process
    // stops before its follow-ups start.
    let v = comment(
        format!("clax_viewer={}", sam.cookie),
        &tid,
        "Sam: italic too.",
    )
    .await;
    assert_eq!(v["run"], json!({"id": second, "queued": true}));
    end(&second);
    // Two more threads with someone queued: one deleted, one resolved.
    for t in [&t2, &t3] {
        let r = send(t).await;
        assert!(comment(owner.clone(), t, "Waiting.").await["run"]["queued"] == true);
        end(&r);
    }
    let res = ts
        .authed(
            ts.client
                .delete(format!("{}/api/artifacts/{aid}/threads/{t2}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert!(ts.store.follow_ups(&t2).unwrap().is_empty());
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{t3}/resolve",
            ts.base
        )))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(ts.store.follow_ups(&t3).unwrap(), ["*owner"]);
    // The next driver to start starts Sam's run, and drops the resolved
    // thread's queue.
    clax_server::bot::start_driver(&driver);
    let third = when(&mut bus, || {
        ts.store
            .runs_for_thread(&tid, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.id != first && r.id != second)
    })
    .await;
    assert_eq!(third.person, sam.public_id);
    let all = events(&ts, &third.id, None).await;
    assert_eq!(all.last().unwrap().2["run"]["status"], "finished");
    assert!(ts.store.follow_ups(&tid).unwrap().is_empty());
    assert!(ts.store.follow_ups(&t3).unwrap().is_empty());
    assert_eq!(ts.store.runs_for_thread(&t3, 10).unwrap().len(), 1);
}
