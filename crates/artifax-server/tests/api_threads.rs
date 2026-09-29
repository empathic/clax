mod common;
use artifax_server::testing::{FAKE_PNG, element_anchor};
use common::TestServer;
use serde_json::{Value, json};

async fn setup(ts: &TestServer) -> (String, String) {
    let s = ts.register_session("claude", "h1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts
        .publish_as(
            &sid,
            "Quarterly Review",
            "<main><h2>Quarterly goals</h2></main>",
        )
        .await;
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn create_stores_the_clip_and_serves_it_sandboxed() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let res = ts
        .create_thread(&aid, 1, "Make this two columns.", Some(FAKE_PNG))
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    let t = &v["thread"];
    assert_eq!(t["has_clip"], true);
    assert_eq!(t["comments"][0]["author_name"], "Viewer");
    assert_eq!(t["clip_path"], Value::Null, "no token, no path");
    let clip = ts.get(t["clip_url"].as_str().unwrap()).await;
    assert_eq!(clip.headers()["content-type"], "image/png");
    assert_eq!(clip.headers()["content-security-policy"], "sandbox");
    assert_eq!(clip.headers()["x-content-type-options"], "nosniff");
    assert_eq!(clip.bytes().await.unwrap().as_ref(), FAKE_PNG);
    let tid = t["id"].as_str().unwrap();
    let authed: Value = ts
        .authed(
            ts.client
                .get(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base)),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let path = authed["thread"]["clip_path"].as_str().unwrap();
    assert!(std::path::Path::new(path).is_absolute() && std::path::Path::new(path).exists());
}

#[tokio::test]
async fn bad_clip_saves_the_thread_without_it() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let res = ts
        .create_thread(&aid, 1, "hostile clip", Some(b"GIF89a-not-a-png"))
        .await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["thread"]["has_clip"], false);
    assert_eq!(v["clip_error"], "the clip is not a PNG image");
    let mut big = FAKE_PNG.to_vec();
    big.resize(artifax_core::store::threads::MAX_CLIP_BYTES + 1, 0);
    let v: Value = ts
        .create_thread(&aid, 1, "huge clip", Some(&big))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["thread"]["has_clip"], false);
    assert!(v["clip_error"].as_str().unwrap().contains("exceeds"));
    let tid = v["thread"]["id"].as_str().unwrap();
    ts.send_thread(&aid, tid).await;
    let fb: Value = ts
        .authed(ts.client.get(format!(
            "{}/api/sessions/{sid}/feedback?tier=piggyback",
            ts.base
        )))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        fb["text"]
            .as_str()
            .unwrap()
            .contains("\nClip: none (no screenshot was captured for this comment)\n")
    );
}

#[tokio::test]
async fn bad_input_is_refused() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let res = ts.create_thread(&aid, 7, "x", None).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_version"
    );
    let form = reqwest::multipart::Form::new()
        .text("anchor", "{\"kind\":\"element\"}")
        .text("body", "x")
        .text("version", "1");
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_anchor"
    );
    let res = ts.create_thread(&aid, 1, "   ", None).await;
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_comment"
    );
    assert_eq!(
        ts.create_thread("zzzzzzzzzzzz", 1, "x", None)
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn viewer_cookie_names_the_author() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let res = ts.get("/api/viewers/me").await;
    let cookie = res.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(
        cookie.starts_with("artifax_viewer=")
            && cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Lax")
            && !cookie.contains("Domain"),
        "{cookie}"
    );
    let pair = cookie.split(';').next().unwrap().to_string();
    let v: Value = ts
        .client
        .put(format!("{}/api/viewers/me", ts.base))
        .header("cookie", &pair)
        .json(&json!({"display_name": "Alex"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["viewer"]["display_name"], "Alex");
    let again = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", &pair)
        .send()
        .await
        .unwrap();
    assert!(
        again.headers().get("set-cookie").is_none(),
        "a valid cookie is kept"
    );
    let form = reqwest::multipart::Form::new()
        .text("anchor", element_anchor().to_string())
        .text("body", "hi")
        .text("version", "1");
    let t: Value = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("cookie", &pair)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(t["thread"]["comments"][0]["author_name"], "Alex");
    let forged = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", "artifax_viewer=../../etc")
        .send()
        .await
        .unwrap();
    assert!(
        forged.headers().get("set-cookie").is_some(),
        "a malformed cookie is replaced"
    );
}

#[tokio::test]
async fn agent_replies_need_the_token_and_a_sent_thread() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let t = ts.thread(&aid, 1, "plain").await;
    let tid = t["id"].as_str().unwrap();
    let url = format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base);
    let body = json!({"body": "done", "author_kind": "agent"});
    assert_eq!(
        ts.client
            .post(&url)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let res = ts
        .authed(ts.client.post(&url))
        .header("x-artifax-session", &sid)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert!(v["guidance"].as_str().unwrap().contains("not sent to you"));
    let after: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        after["thread"]["comments"].as_array().unwrap().len(),
        1,
        "guidance writes nothing"
    );
    ts.send_thread(&aid, tid).await;
    let res = ts
        .authed(ts.client.post(&url))
        .header("x-artifax-session", &sid)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["comment"]["author_kind"], "agent");
    assert_eq!(v["comment"]["author_name"], "claude");
    assert_eq!(v["comment"]["via_harness"], "claude");
    assert_eq!(
        v["thread"]["feedback_state"]["state"], "acknowledged",
        "an agent reply acknowledges"
    );
}

#[tokio::test]
async fn at_agent_sends_but_an_address_does_not() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let a = ts.thread(&aid, 1, "write to me@agent.dev").await;
    assert_eq!(a["sent_to_agent"], false);
    let b = ts.thread(&aid, 1, "@agent please fix the spacing").await;
    assert_eq!(b["sent_to_agent"], true);
    let tid = a["id"].as_str().unwrap();
    let res: Value = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        ))
        .json(&json!({"body": "over to you (@agent)."}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["thread"]["sent_to_agent"], true);
    for (s, want) in [
        ("@agent", true),
        ("hey @agent.", true),
        ("me@agent.dev", false),
        ("@agents", false),
        ("x@agent", false),
    ] {
        assert_eq!(
            artifax_server::routes::threads::mentions_agent(s),
            want,
            "{s}"
        );
    }
}

#[tokio::test]
async fn resolve_by_viewer_and_agent() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let plain = ts.thread(&aid, 1, "plain").await;
    let pid = plain["id"].as_str().unwrap();
    let agent = json!({"as": "agent"});
    let url = |tid: &str| format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base);
    let g: Value = ts
        .authed(ts.client.post(url(pid)))
        .header("x-artifax-session", &sid)
        .json(&agent)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(g["guidance"].is_string());
    let v: Value = ts
        .client
        .post(url(pid))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["thread"]["status"], "resolved");
    assert!(
        v["thread"]["resolved_by"]
            .as_str()
            .unwrap()
            .starts_with("viewer:")
    );
    let sent = ts.thread(&aid, 1, "@agent fix").await;
    let sid2 = sent["id"].as_str().unwrap();
    assert_eq!(
        ts.client
            .post(url(sid2))
            .json(&agent)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let v: Value = ts
        .authed(ts.client.post(url(sid2)))
        .header("x-artifax-session", &sid)
        .json(&agent)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["thread"]["resolved_by"], "agent:claude");
    let listed: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        listed["threads"].as_array().unwrap().is_empty(),
        "resolved threads are hidden by default"
    );
    let all: Value = ts
        .get(&format!(
            "/api/artifacts/{aid}/threads?include_resolved=true"
        ))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(all["threads"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn events_announce_threads_comments_resolutions_and_feedback_state() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let t = ts.thread(&aid, 1, "first").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let e = ev.next_named("thread").await;
    assert_eq!(
        (e["artifact_id"].as_str(), e["thread"]["id"].as_str()),
        (Some(aid.as_str()), Some(tid.as_str()))
    );
    ts.client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        ))
        .json(&json!({"body": "second"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ev.next_named("comment").await["comment"]["body"], "second");
    ts.send_thread(&aid, &tid).await;
    let fs = ev.next_named("feedback_state").await;
    assert_eq!(
        fs,
        json!({
            "type": "feedback_state", "artifact_id": aid, "thread_id": tid, "state": "sent",
            "tier": "stop_hook", "since": fs["since"], "resends": 0, "exhausted": false
        })
    );
    ts.client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        ))
        .send()
        .await
        .unwrap();
    let r = ev.next_named("thread_resolved").await;
    assert_eq!(r["thread_id"], tid);
}

#[tokio::test]
async fn thread_events_never_carry_the_clip_path() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let res: Value = ts
        .create_thread(&aid, 1, "@agent with a clip", Some(FAKE_PNG))
        .await
        .json()
        .await
        .unwrap();
    let tid = res["thread"]["id"].as_str().unwrap().to_string();
    let created = ev.next_named("thread").await;
    assert_eq!(created["thread"]["has_clip"], true);
    assert_eq!(created["thread"]["clip_path"], Value::Null);
    let reply = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        )))
        .header("x-artifax-session", &sid)
        .json(&json!({"body": "done", "author_kind": "agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 201);
    let body: Value = reply.json().await.unwrap();
    assert!(
        body["thread"]["clip_path"].is_string(),
        "the authenticated response carries the path"
    );
    let e = ev.next_named("thread").await;
    assert_eq!(e["thread"]["comments"][1]["author_kind"], "agent");
    assert_eq!(
        e["thread"]["clip_path"],
        Value::Null,
        "the agent's token never reaches SSE subscribers"
    );
}

#[tokio::test]
async fn agent_actions_need_a_live_session_header() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let t = ts.thread(&aid, 1, "@agent fix").await;
    let tid = t["id"].as_str().unwrap();
    let reply = format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base);
    let resolve = format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base);
    let body = json!({"body": "done", "author_kind": "agent"});
    let as_agent = json!({"as": "agent"});
    for (url, b) in [(&reply, &body), (&resolve, &as_agent)] {
        let res = ts.authed(ts.client.post(url)).json(b).send().await.unwrap();
        assert_eq!(res.status(), 400, "{url}: no session header");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "unknown_session"
        );
    }
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    for (url, b) in [(&reply, &body), (&resolve, &as_agent)] {
        let res = ts
            .authed(ts.client.post(url))
            .header("x-artifax-session", &sid)
            .json(b)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "{url}: ended session");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "unknown_session"
        );
    }
    let after: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(after["thread"]["status"], "open");
    assert_eq!(after["thread"]["comments"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn viewer_resolve_withdraws_undelivered_feedback() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let t = ts.thread(&aid, 1, "@agent fix").await;
    let tid = t["id"].as_str().unwrap();
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let v: Value = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["thread"]["feedback_state"], Value::Null, "no rows remain");
    ev.next_named("thread_resolved").await;
    assert_eq!(
        ev.next_named("thread").await["thread"]["feedback_state"],
        Value::Null
    );
    let fb: Value = ts
        .authed(ts.client.get(format!(
            "{}/api/sessions/{sid}/feedback?tier=piggyback",
            ts.base
        )))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(fb["feedback"].as_array().unwrap().is_empty());
    let st = artifax_core::Store::open(&ts.home).unwrap();
    assert!(
        st.feedback_rows(tid).unwrap().is_empty(),
        "the rows are gone"
    );
}

#[tokio::test]
async fn viewer_routes_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let port = ts.addr.port();
    let t = ts.thread(&aid, 1, "plain").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let form = || {
        reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", "hi")
            .text("version", "1")
    };
    let foreign = [
        format!("http://{aid}.localhost:{port}"),
        "https://evil.example".to_string(),
        "null".to_string(),
    ];
    for origin in &foreign {
        let reqs = [
            ts.client
                .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
                .multipart(form()),
            ts.client
                .post(format!(
                    "{}/api/artifacts/{aid}/threads/{tid}/comments",
                    ts.base
                ))
                .json(&json!({"body": "x"})),
            ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                ts.base
            )),
            ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/resolve",
                ts.base
            )),
            ts.client.get(format!("{}/api/viewers/me", ts.base)),
            ts.client
                .put(format!("{}/api/viewers/me", ts.base))
                .json(&json!({"display_name": "x"})),
        ];
        for r in reqs {
            let res = r.header("origin", origin).send().await.unwrap();
            assert_eq!(res.status(), 403, "{origin}");
            assert_eq!(
                res.json::<Value>().await.unwrap()["error"]["code"],
                "forbidden_origin"
            );
        }
    }
    let same = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("host", format!("localhost:{port}"))
        .header("origin", format!("http://localhost:{port}"))
        .multipart(form())
        .send()
        .await
        .unwrap();
    assert_eq!(same.status(), 201, "the shell's own origin");
    let none = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .multipart(form())
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 201, "no Origin: scripts and curl");
    let after: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        after["thread"]["status"], "open",
        "refused requests changed nothing"
    );
    assert_eq!(after["thread"]["sent_to_agent"], false);
}

#[tokio::test]
async fn the_first_valid_viewer_cookie_wins() {
    let ts = TestServer::spawn().await;
    let a = artifax_core::new_ulid();
    let b = artifax_core::new_ulid();
    let res = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header(
            "cookie",
            format!("artifax_viewer=junk; artifax_viewer={a}; artifax_viewer={b}"),
        )
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("set-cookie").is_none());
    let got = res.json::<Value>().await.unwrap()["viewer"]["public_id"].clone();
    let only_a: Value = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", format!("artifax_viewer={a}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got, only_a["viewer"]["public_id"]);
}

/// The viewer cookie is a credential and the session ID names a live agent:
/// neither may appear in anything the daemon hands to others.
#[tokio::test]
async fn the_viewer_cookie_and_session_ids_never_leave_the_daemon() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let cookie = artifax_core::new_ulid();
    let pair = format!("artifax_viewer={cookie}");
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let me = ts
        .client
        .put(format!("{}/api/viewers/me", ts.base))
        .header("cookie", &pair)
        .json(&json!({"display_name": "Alex"}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let me_v: Value = serde_json::from_str(&me).unwrap();
    let public_id = me_v["viewer"]["public_id"].as_str().unwrap().to_string();
    assert!(artifax_core::is_public_id(&public_id), "{me}");
    assert_eq!(me_v["viewer"]["display_name"], "Alex");
    let get_me = ts
        .client
        .get(format!("{}/api/viewers/me", ts.base))
        .header("cookie", &pair)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .text("anchor", element_anchor().to_string())
        .text("body", "@agent fix this")
        .text("version", "1");
    let created: Value = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .header("cookie", &pair)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tid = created["thread"]["id"].as_str().unwrap().to_string();
    let base = format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base);
    let reply = ts
        .authed(ts.client.post(format!("{base}/comments")))
        .header("x-artifax-session", &sid)
        .json(&json!({"body": "done", "author_kind": "agent"}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap()["comment"]["via_harness"],
        "claude"
    );
    let resolved = ts
        .client
        .post(format!("{base}/resolve"))
        .header("cookie", &pair)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&resolved).unwrap()["thread"]["resolved_by"],
        format!("viewer:{public_id}")
    );
    // A second thread the agent resolves.
    let other = ts.thread(&aid, 1, "@agent and this").await;
    let oid = other["id"].as_str().unwrap();
    let by_agent: Value = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{oid}/resolve",
            ts.base
        )))
        .header("x-artifax-session", &sid)
        .json(&json!({"as": "agent"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(by_agent["thread"]["resolved_by"], "agent:claude");
    let mut seen = Vec::new();
    let mut resolved_events = 0;
    while resolved_events < 2 {
        let (name, data) = ev.next().await;
        if name == "thread_resolved" {
            resolved_events += 1;
        }
        seen.push(format!("{name} {data}"));
    }
    // The `thread` event that follows each resolve.
    seen.push(format!("{}", ev.next_named("thread").await));
    let list = ts
        .get(&format!(
            "/api/artifacts/{aid}/threads?include_resolved=true"
        ))
        .await
        .text()
        .await
        .unwrap();
    let list_authed = ts
        .get_authed(&format!(
            "/api/artifacts/{aid}/threads?include_resolved=true"
        ))
        .await
        .text()
        .await
        .unwrap();
    let one = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .text()
        .await
        .unwrap();
    let all = [
        me,
        get_me,
        created.to_string(),
        reply,
        resolved,
        by_agent.to_string(),
        list,
        list_authed,
        one,
    ]
    .into_iter()
    .chain(seen)
    .collect::<Vec<_>>();
    assert!(all.iter().any(|s| s.contains("thread_resolved")));
    for text in &all {
        assert!(!text.contains(&cookie), "the cookie leaked: {text}");
        assert!(!text.contains(&sid), "the session ID leaked: {text}");
        assert!(!text.contains("via_session_id"), "{text}");
    }
}
