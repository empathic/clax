use crate::common;
use clax_server::testing::{FAKE_PNG, element_anchor};
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
    big.resize(clax_core::store::threads::MAX_CLIP_BYTES + 1, 0);
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
        cookie.starts_with("clax_viewer=")
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
        .header("cookie", "clax_viewer=../../etc")
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
        .header("x-clax-session", &sid)
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
        .header("x-clax-session", &sid)
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
        assert_eq!(clax_server::routes::threads::mentions_agent(s), want, "{s}");
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
        .header("x-clax-session", &sid)
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
        .header("x-clax-session", &sid)
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
        .header("x-clax-session", &sid)
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
            .header("x-clax-session", &sid)
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
    let st = clax_core::Store::open(&ts.home).unwrap();
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
async fn malformed_viewer_cookies_are_skipped_and_disagreeing_ones_name_no_one() {
    let ts = TestServer::spawn().await;
    let a = clax_core::new_ulid();
    let b = clax_core::new_ulid();
    let me = |cookies: String| {
        let ts = &ts;
        async move {
            ts.client
                .get(format!("{}/api/viewers/me", ts.base))
                .header("cookie", cookies)
                .send()
                .await
                .unwrap()
        }
    };
    let res = me(format!(
        "clax_viewer=junk; clax_viewer={a}; clax_viewer={a}"
    ))
    .await;
    assert!(res.headers().get("set-cookie").is_none());
    let got = res.json::<Value>().await.unwrap()["viewer"]["public_id"].clone();
    let only_a: Value = me(format!("clax_viewer={a}")).await.json().await.unwrap();
    assert_eq!(got, only_a["viewer"]["public_id"]);
    // Two values: one was planted by another page (cookies ignore ports, and
    // a longer path is sent first). Neither is trusted, so neither wins.
    let res = me(format!("clax_viewer={b}; clax_viewer={a}")).await;
    let fresh = res.headers().get("set-cookie").is_some();
    let got = res.json::<Value>().await.unwrap()["viewer"]["public_id"].clone();
    assert!(fresh, "a new viewer is minted");
    assert_ne!(got, only_a["viewer"]["public_id"]);
}

/// The viewer cookie is a credential and the session ID names a live agent:
/// neither may appear in anything the daemon hands to others.
#[tokio::test]
async fn the_viewer_cookie_and_session_ids_never_leave_the_daemon() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let cookie = clax_core::new_ulid();
    let pair = format!("clax_viewer={cookie}");
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
    assert!(clax_core::is_public_id(&public_id), "{me}");
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
        .header("x-clax-session", &sid)
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
        .header("x-clax-session", &sid)
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

#[tokio::test]
async fn resolving_as_a_viewer_records_the_public_id() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let v = ts.viewer(Some("Alex")).await;
    let t = ts.thread(&aid, 1, "plain").await;
    let mut events = ts.events(&format!("?artifact={aid}")).await;
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{}/resolve",
            ts.base,
            t["id"].as_str().unwrap()
        ))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["thread"]["resolved_by"],
        format!("viewer:{}", v.public_id)
    );
    let ev = events.next_named("thread_resolved").await;
    assert_eq!(ev["resolved_by"], format!("viewer:{}", v.public_id));
    assert!(
        !ev.to_string().contains(&v.cookie),
        "the cookie never reaches SSE"
    );
}

#[tokio::test]
async fn a_resolved_thread_names_its_resolver_when_the_viewer_has_a_name() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let named = ts.viewer(Some("Mia")).await;
    let unnamed = ts.viewer(None).await;
    let resolve = |tid: String, cookie: Option<String>| {
        let mut req = ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        ));
        if let Some(c) = cookie {
            req = req.header("cookie", format!("clax_viewer={c}"));
        }
        async move { req.send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let id = |t: &Value| t["id"].as_str().unwrap().to_string();
    let a = ts.thread(&aid, 1, "by Mia").await;
    let b = ts.thread(&aid, 1, "by someone").await;
    let c = ts.thread(&aid, 1, "anonymous").await;
    let open = ts.thread(&aid, 1, "still open").await;
    assert_eq!(
        resolve(id(&a), Some(named.cookie.clone())).await["thread"]["resolved_by_name"],
        "Mia"
    );
    assert!(
        resolve(id(&b), Some(unnamed.cookie.clone())).await["thread"]["resolved_by_name"].is_null()
    );
    assert!(resolve(id(&c), None).await["thread"]["resolved_by_name"].is_null());
    let listed: Value = ts
        .get(&format!(
            "/api/artifacts/{aid}/threads?include_resolved=true"
        ))
        .await
        .json()
        .await
        .unwrap();
    let named_in_list = |tid: String| {
        listed["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == tid.as_str())
            .unwrap()["resolved_by_name"]
            .clone()
    };
    assert_eq!(named_in_list(id(&a)), "Mia");
    assert!(named_in_list(id(&open)).is_null());
}

#[tokio::test]
async fn threads_are_anchored_on_any_file_of_their_version() {
    let ts = TestServer::spawn().await;
    let a = ts
        .publish(
            "Two pages",
            &[
                ("index.html", "<a href=\"about.html\">about</a>"),
                ("about.html", "<h2>About</h2>"),
            ],
        )
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut ev = ts.events(&format!("?artifact={aid}")).await;
    let post = |anchor: Value| {
        let form = reqwest::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", "Tighten this heading")
            .text("version", "1");
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
            .multipart(form)
            .send()
    };
    let mut on_about = element_anchor();
    on_about["file"] = json!("about.html");
    let res = post(on_about).await.unwrap();
    assert_eq!(res.status(), 201);
    let t = res.json::<Value>().await.unwrap()["thread"].clone();
    assert_eq!(t["anchor"]["file"], "about.html");
    let e = ev.next_named("thread").await;
    assert_eq!(e["thread"]["anchor"]["file"], "about.html");
    let listed: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(listed["threads"][0]["anchor"]["file"], "about.html");

    let res = post(element_anchor()).await.unwrap();
    assert_eq!(
        res.status(),
        201,
        "an anchor without a file is on the index"
    );
    let t = res.json::<Value>().await.unwrap()["thread"].clone();
    assert_eq!(t["anchor"]["file"], "index.html");

    for file in ["missing.html", "../about.html", ""] {
        let mut bad = element_anchor();
        bad["file"] = json!(file);
        let res = post(bad).await.unwrap();
        assert_eq!(res.status(), 400, "{file:?}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "invalid_anchor",
            "{file:?}"
        );
    }
}

#[tokio::test]
async fn area_threads_keep_their_drawn_rectangle_and_bad_areas_are_refused() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let post = |anchor: Value| {
        let form = reqwest::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", "What is this gap for?")
            .text("version", "1");
        ts.client
            .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
            .multipart(form)
            .send()
    };
    let area = json!({
        "kind": "area", "selector": "body > main", "quote": null, "prefix": null, "suffix": null,
        "html_hash": null, "custom_name": null, "file": "index.html",
        "area": {"x": 0.25, "y": 0.5, "w": 0.5, "h": 0.125},
        "rect": {"x": 40.0, "y": 60.0, "w": 200.0, "h": 50.0, "scrollX": 0.0, "scrollY": 120.0, "viewportW": 800.0}
    });
    let res = post(area.clone()).await.unwrap();
    assert_eq!(res.status(), 201);
    let t = res.json::<Value>().await.unwrap()["thread"].clone();
    assert_eq!(t["anchor"]["kind"], "area");
    assert_eq!(t["anchor"]["area"], area["area"]);
    assert_eq!(t["anchor"]["rect"]["scrollY"], 120.0);
    for (name, patch) in [
        ("no area", json!(null)),
        (
            "past the edge",
            json!({"x": 0.75, "y": 0.0, "w": 0.5, "h": 0.5}),
        ),
        ("negative", json!({"x": -0.1, "y": 0.0, "w": 0.5, "h": 0.5})),
    ] {
        let mut bad = area.clone();
        bad["area"] = patch;
        let res = post(bad).await.unwrap();
        assert_eq!(res.status(), 400, "{name}");
        assert_eq!(
            res.json::<Value>().await.unwrap()["error"]["code"],
            "invalid_anchor",
            "{name}"
        );
    }
}

#[tokio::test]
async fn viewers_reopen_and_delete_threads_with_events() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let t: Value = ts
        .create_thread(&aid, 1, "tidy this", Some(FAKE_PNG))
        .await
        .json()
        .await
        .unwrap();
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let url = |tail: &str| format!("{}/api/artifacts/{aid}/threads/{tid}{tail}", ts.base);
    let named = ts.viewer(Some("Sam")).await;
    let as_named =
        |r: reqwest::RequestBuilder| r.header("cookie", format!("clax_viewer={}", named.cookie));
    assert_eq!(
        ts.client
            .post(url("/resolve"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut events = ts.events(&format!("?artifact={aid}")).await;
    let res = as_named(ts.client.post(url("/reopen")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (
            v["thread"]["status"].as_str(),
            v["thread"]["resolved_by"].clone()
        ),
        (Some("open"), Value::Null)
    );
    assert_eq!(
        events.next_named("thread").await["thread"]["status"],
        "open"
    );
    let clip = ts
        .home
        .clip_path(&clax_core::ArtifactId::parse(&aid).unwrap(), &tid);
    assert!(clip.exists());
    let res = as_named(ts.client.delete(url(""))).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap(),
        json!({"deleted": true, "thread_id": tid})
    );
    assert_eq!(
        events.next_named("thread_deleted").await,
        json!({"type": "thread_deleted", "artifact_id": aid, "thread_id": tid})
    );
    assert!(!clip.exists());
    assert_eq!(
        ts.get(&format!("/api/artifacts/{aid}/threads/{tid}"))
            .await
            .status(),
        404
    );
    assert_eq!(
        as_named(ts.client.delete(url("")))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn reopening_and_deleting_need_a_name_or_the_token() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let tid = ts.thread(&aid, 1, "x").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let url = |tail: &str| format!("{}/api/artifacts/{aid}/threads/{tid}{tail}", ts.base);
    let unnamed = ts.viewer(None).await;
    let cookie = format!("clax_viewer={}", unnamed.cookie);
    for res in [
        ts.client.post(url("/reopen")).send().await.unwrap(),
        ts.client
            .post(url("/reopen"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
        ts.client
            .delete(url(""))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(res.status(), 403);
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "forbidden");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("name"),
            "{v}"
        );
    }
    assert_eq!(
        ts.authed(ts.client.post(url("/reopen")))
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "the owner shell (token) may"
    );
    assert_eq!(
        ts.authed(ts.client.delete(url("")))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn agents_reopen_and_delete_only_sent_threads_from_a_live_session() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let plain = ts.thread(&aid, 1, "plain").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sent = ts.thread(&aid, 1, "@agent fix").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let agent = |req: reqwest::RequestBuilder, session: bool| {
        let r = ts.authed(req);
        if session {
            r.header("x-clax-session", &sid)
        } else {
            r
        }
    };
    let reopen = |tid: &str| {
        ts.client
            .post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/reopen",
                ts.base
            ))
            .json(&json!({"as": "agent"}))
    };
    let res = agent(reopen(&sent), false).send().await.unwrap();
    assert_eq!(res.status(), 400, "an agent needs a live session");
    let v: Value = agent(reopen(&plain), true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v["guidance"].is_string(), "{v}");
    assert_eq!(
        agent(reopen(&sent), true).send().await.unwrap().status(),
        200
    );
    let del = |tid: &str| {
        ts.client.delete(format!(
            "{}/api/artifacts/{aid}/threads/{tid}?as=agent",
            ts.base
        ))
    };
    assert_eq!(
        ts.client
            .delete(format!(
                "{}/api/artifacts/{aid}/threads/{sent}?as=agent",
                ts.base
            ))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let v: Value = agent(del(&plain), true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v["guidance"].is_string(), "{v}");
    let v: Value = agent(del(&sent), true)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["deleted"], true);
}

#[tokio::test]
async fn reopen_and_delete_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = setup(&ts).await;
    let tid = ts.thread(&aid, 1, "x").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let port = ts.addr.port();
    let origin = format!("http://{aid}.localhost:{port}");
    let r = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/reopen",
            ts.base
        ))
        .header("origin", &origin)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = ts
        .client
        .delete(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base))
        .header("origin", &origin)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
}

/// `POST .../threads` as the shell does for a page's `comments.create`.
async fn page_thread(ts: &TestServer, aid: &str, body: &str) -> Value {
    let form = reqwest::multipart::Form::new()
        .text("anchor", element_anchor().to_string())
        .text("body", body.to_string())
        .text("version", "1")
        .text("via_page", "true");
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    res.json::<Value>().await.unwrap()["thread"].clone()
}

#[tokio::test]
async fn page_written_comments_are_marked_and_their_mentions_send_nothing() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = setup(&ts).await;
    let t = page_thread(&ts, &aid, "@agent please look").await;
    assert_eq!(t["sent_to_agent"], false, "a page's @agent is inert");
    assert_eq!(t["comments"][0]["via_page"], true);
    let plain = ts.thread(&aid, 1, "by hand").await;
    assert_eq!(plain["comments"][0]["via_page"], false);
    let tid = plain["id"].as_str().unwrap();
    let url = format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base);
    let v: Value = ts
        .client
        .post(&url)
        .json(&json!({"body": "@agent go", "via_page": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["comment"]["via_page"], true);
    assert_eq!(v["thread"]["sent_to_agent"], false);
    let res = ts
        .authed(ts.client.post(&url))
        .header("x-clax-session", &sid)
        .json(&json!({"body": "x", "author_kind": "agent", "via_page": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let bad = reqwest::multipart::Form::new()
        .text("anchor", element_anchor().to_string())
        .text("body", "x")
        .text("version", "1")
        .text("via_page", "yes");
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .multipart(bad)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // A page reply into a sent thread is forwarded, and the payload says the
    // page wrote it.
    let sent = ts.thread(&aid, 1, "@agent fix the header").await;
    let stid = sent["id"].as_str().unwrap();
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{stid}/comments",
            ts.base
        ))
        .json(&json!({"body": "and the footer", "via_page": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
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
    let text = fb["text"].as_str().unwrap();
    assert!(
        text.contains("\nViewer: \"@agent fix the header\"\n"),
        "{text}"
    );
    assert!(
        text.contains("\nViewer (written by the page): \"and the footer\"\n"),
        "{text}"
    );
}

#[tokio::test]
async fn every_thread_of_every_artifact_needs_the_token() {
    let ts = TestServer::spawn().await;
    let (_, aid) = setup(&ts).await;
    let other = ts.publish("Empty", &[("index.html", "<p>none</p>")]).await;
    let other = other["artifact"]["id"].as_str().unwrap().to_string();
    let first = ts.thread(&aid, 1, "first").await;
    let res = ts.create_thread(&aid, 1, "second", Some(FAKE_PNG)).await;
    assert_eq!(res.status(), 201);
    let second: Value = res.json().await.unwrap();
    let second = second["thread"].clone();
    ts.client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{}/resolve",
            ts.base,
            first["id"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(ts.get("/api/threads").await.status(), 401);

    let open: Value = ts.get_authed("/api/threads").await.json().await.unwrap();
    let groups = open["artifacts"].as_array().unwrap();
    assert_eq!(
        groups.len(),
        1,
        "an artifact without threads is left out: {open}"
    );
    let g = &groups[0];
    assert_eq!(g["artifact_id"], aid.as_str());
    assert_eq!(g["title"], "Quarterly Review");
    assert_eq!(g["current_version"], 1);
    assert_eq!(g["files"], json!(["index.html"]));
    assert_eq!(g["threads"].as_array().unwrap().len(), 1);
    assert_eq!(g["threads"][0]["id"], second["id"]);
    assert!(
        g["threads"][0]["clip_path"].as_str().is_some(),
        "the token reads clip paths"
    );
    assert!(!open.to_string().contains(&other));

    let all: Value = ts
        .get_authed("/api/threads?include_resolved=true")
        .await
        .json()
        .await
        .unwrap();
    let threads = all["artifacts"][0]["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["id"], first["id"], "oldest first");
    assert_eq!(threads[0]["status"], "resolved");
}
