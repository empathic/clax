//! The owner's inbox over HTTP (spec 2026-10-06-agent-questions-and-inbox
//! §8): the routes, the `inbox` stream topic, finished work and read marks.

mod common;
use common::TestServer;
use serde_json::{Value, json};

/// The owner comments on a thread of a fresh agent artifact and sends it;
/// returns (session, artifact, thread).
async fn owner_thread(ts: &TestServer) -> (String, String, String) {
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = ts.publish_as(&sid, "Board", "<h1>Board</h1>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread_as_owner(&aid, "make it blue").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn get(ts: &TestServer, path: &str) -> Value {
    let res = ts.get_authed(path).await;
    assert_eq!(res.status(), 200, "{path}");
    res.json().await.unwrap()
}

#[tokio::test]
async fn reply_item_arrives_on_the_topic_and_is_read_by_a_look() {
    let ts = TestServer::spawn().await;
    let mut ev = ts.stream_as_owner(&["inbox"]).await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "Done: blue").await;
    let e = loop {
        let e = ev.next_named("inbox_item").await;
        if e["item"]["kind"] == "reply" {
            break e;
        }
    };
    assert_eq!(e["topic"], "inbox");
    assert_eq!(
        (e["item"]["read"].as_bool(), e["unread"].as_u64()),
        (Some(false), Some(2)),
        "published + reply"
    );
    assert_eq!(e["item"]["reply"]["body"], "Done: blue");
    assert_eq!(e["item"]["reply"]["addressed"], false);
    assert_eq!(e["item"]["thread"]["id"], tid.as_str());
    assert_eq!(e["item"]["thread"]["status"], "open");
    assert_eq!(e["item"]["artifact"]["title"], "Board");
    assert_eq!(e["item"]["agent"]["harness"], "claude");
    assert_eq!(e["item"]["agent"]["project"], "w");
    assert_eq!(e["item"]["url"], format!("/a/{aid}?thread={tid}").as_str());
    assert!(e["item"].get("session_id").is_none());
    ts.look_as_owner(&aid, &[&tid]).await;
    let e = ev.next_named("inbox_item").await;
    assert_eq!(
        (
            e["item"]["kind"].as_str(),
            e["item"]["read"].as_bool(),
            e["unread"].as_u64()
        ),
        (Some("reply"), Some(true), Some(1))
    );
}

#[tokio::test]
async fn inbox_is_owner_only_and_lan_looks_mark_nothing() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "x").await;
    let (lan, base) = ts.lan();
    for path in ["/api/inbox", "/api/inbox/summary"] {
        let res = lan.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(res.status(), 403, "{path}");
    }
    let res = lan
        .post(format!("{base}/api/inbox/read"))
        .json(&json!({"all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    let mia = ts.viewer(Some("Mia")).await;
    mia.look(&aid, &[&tid]).await;
    let s = get(&ts, "/api/inbox/summary").await;
    assert_eq!(s["unread"], 2, "a viewer's look reads nothing: {s}");
    assert_eq!(mia.subscribe_status(&["inbox"]).await, 403);
    let res = ts
        .client
        .post(format!("{}/api/inbox/read", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", "http://localhost:5173")
        .json(&json!({"all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "a foreign origin");
    let res = ts
        .client
        .post(format!("{}/api/inbox/read", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"all": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "a browser of the owner's");
}

#[tokio::test]
async fn search_filters_paging_and_marks() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    for i in 0..60 {
        ts.reply_as_agent(&sid, &aid, &tid, &format!("note {i} about the header"))
            .await;
    }
    ts.reply_as_agent(&sid, &aid, &tid, "the footer is green")
        .await;
    let p = get(&ts, "/api/inbox?q=foot&read=all").await;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    assert_eq!(p["total"], 1);
    let p = get(&ts, "/api/inbox?kind=reply&read=unread").await;
    assert_eq!(p["items"].as_array().unwrap().len(), 50);
    assert_eq!(p["total"], 61);
    assert_eq!(p["unread"], 62, "61 replies and the published item");
    let next = p["next_cursor"].as_str().unwrap();
    let p2 = get(
        &ts,
        &format!("/api/inbox?kind=reply&read=unread&before={next}"),
    )
    .await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 11);
    assert!(p2["next_cursor"].is_null());
    let all = get(&ts, "/api/inbox").await;
    assert!(all.get("total").is_none(), "no filter, no total");
    assert_eq!(all["items"][0]["kind"], "reply");
    assert_eq!(all["items"][0]["reply"]["body"], "the footer is green");
    let r: Value = ts
        .post_json(
            "/api/inbox/read",
            json!({"all": true, "filter": {"q": "header"}}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["marked"].as_u64(), r["unread"].as_u64()),
        (Some(60), Some(2))
    );
    for bad in [
        "/api/inbox?since=yesterday",
        "/api/inbox?until=2026-13-01",
        "/api/inbox?kind=nope",
        "/api/inbox?artifact=x",
        "/api/inbox?before=abc",
        "/api/inbox?read=some",
    ] {
        let res = ts.get_authed(bad).await;
        assert_eq!(res.status(), 400, "{bad}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "invalid_query", "{bad}");
    }
    assert_eq!(
        ts.get_authed("/api/inbox?q=%22unclosed%20NEAR(")
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn dates_filter_in_the_stored_form() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "now").await;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let p = get(&ts, &format!("/api/inbox?kind=reply&since={today}")).await;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    let p = get(&ts, &format!("/api/inbox?kind=reply&until={today}")).await;
    assert_eq!(
        p["items"].as_array().unwrap().len(),
        1,
        "until a date takes in the day"
    );
    let p = get(&ts, "/api/inbox?kind=reply&until=2020-01-01").await;
    assert_eq!(p["items"].as_array().unwrap().len(), 0);
    // An offset time is compared as the same instant in UTC.
    let later = (chrono::Utc::now() + chrono::Duration::hours(1))
        .with_timezone(&chrono::FixedOffset::east_opt(5 * 3600).unwrap())
        .to_rfc3339();
    let p = get(
        &ts,
        &format!("/api/inbox?kind=reply&until={}", later.replace('+', "%2B")),
    )
    .await;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    let p = get(
        &ts,
        &format!("/api/inbox?kind=reply&since={}", later.replace('+', "%2B")),
    )
    .await;
    assert_eq!(p["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn one_item_reads_unreads_and_the_summary_counts() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "Done").await;
    let q = ts
        .ask(
            &sid,
            json!({"source": "ask", "questions": [{"question": "Blue or green?", "header": "Colour",
                "options": [{"label": "Blue"}, {"label": "Green"}]}]}),
        )
        .await;
    let s = get(&ts, "/api/inbox/summary").await;
    assert_eq!(s["unread"], 3);
    assert_eq!(s["questions"].as_array().unwrap().len(), 1);
    assert_eq!(s["questions"][0]["id"], q["question"]["id"]);
    let latest = s["latest"].as_array().unwrap();
    assert_eq!(
        latest
            .iter()
            .map(|i| i["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["reply", "published"],
        "newest first, no questions"
    );
    let id = latest[0]["id"].as_str().unwrap();
    let one = get(&ts, &format!("/api/inbox/{id}")).await;
    assert_eq!(one["item"]["id"], id);
    let r: Value = ts
        .post_json(&format!("/api/inbox/{id}/read"), json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["item"]["read"].as_bool(), r["unread"].as_u64()),
        (Some(true), Some(2))
    );
    let r: Value = ts
        .post_json(&format!("/api/inbox/{id}/unread"), json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["item"]["read"].as_bool(), r["unread"].as_u64()),
        (Some(false), Some(3))
    );
    let qi = get(&ts, "/api/inbox?kind=question").await["items"][0].clone();
    assert_eq!(qi["question"]["status"], "open");
    assert_eq!(
        qi["url"],
        format!("/inbox?q={}", q["question"]["id"].as_str().unwrap()).as_str()
    );
    for missing in ["/api/inbox/01ARZ3NDEKTSV4RRFFQ69G5FAV", "/api/inbox/nope"] {
        assert_eq!(ts.get_authed(missing).await.status(), 404, "{missing}");
    }
    let res = ts
        .post_json("/api/inbox/read", json!({"ids": ["nope"]}))
        .await;
    assert_eq!(res.status(), 400);
    let res = ts.post_json("/api/inbox/read", json!({})).await;
    assert_eq!(res.status(), 400, "neither ids nor all");
    let ids: Vec<String> = (0..501).map(|_| clax_core::new_ulid()).collect();
    let res = ts.post_json("/api/inbox/read", json!({"ids": ids})).await;
    assert_eq!(res.status(), 400, "at most 500 ids");
    let r: Value = ts
        .post_json("/api/inbox/read", json!({"ids": [id]}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["marked"].as_u64(), r["unread"].as_u64()),
        (Some(1), Some(2))
    );
}

#[tokio::test]
async fn mark_all_stops_at_the_newest_item_shown_and_a_large_one_says_refetch() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    for i in 0..55 {
        ts.reply_as_agent(&sid, &aid, &tid, &format!("r{i}")).await;
    }
    let shown = get(&ts, "/api/inbox?read=unread&limit=1").await["items"][0]["seq"]
        .as_i64()
        .unwrap();
    ts.reply_as_agent(&sid, &aid, &tid, "made after the page")
        .await;
    let mut ev = ts.stream_as_owner(&["inbox"]).await;
    let r: Value = ts
        .post_json("/api/inbox/read", json!({"all": true, "upto": shown}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["marked"].as_u64(), r["unread"].as_u64()),
        (Some(56), Some(1))
    );
    let e = ev.next_named("inbox_read").await;
    assert_eq!(e["ids"], Value::Null);
    assert_eq!(
        (e["read"].as_bool(), e["unread"].as_u64()),
        (Some(true), Some(1))
    );
    let left = get(&ts, "/api/inbox?read=unread").await;
    assert_eq!(left["items"][0]["reply"]["body"], "made after the page");
}

#[tokio::test]
async fn finished_work_is_an_item_only_when_the_agent_says_so() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.put_working(
        &sid,
        &aid,
        json!({"message": "Recolouring", "thread_ids": [tid]}),
    )
    .await;
    ts.delete_working(&sid, &aid).await;
    ts.put_working(&sid, &aid, json!({"message": "Second pass"}))
        .await;
    ts.skew_working(200).await;
    let p = get(&ts, "/api/inbox?kind=finished&read=all").await;
    let items = p["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{p}");
    assert_eq!(items[0]["work"]["message"], "Recolouring");
    assert_eq!(items[0]["work"]["threads"][0]["id"], tid.as_str());
    assert!(items[0]["work"]["threads"][0]["summary"].is_string());
    // The turn ending is finished work too.
    ts.put_working(&sid, &aid, json!({"message": "Third pass"}))
        .await;
    let res = ts
        .post_json(&format!("/api/sessions/{sid}/working/end"), json!({}))
        .await;
    assert_eq!(res.status(), 200);
    let p = get(&ts, "/api/inbox?kind=finished").await;
    assert_eq!(p["items"][0]["work"]["message"], "Third pass");
    assert_eq!(p["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_deleted_source_leaves_a_gone_item() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "x").await;
    ts.delete_thread_as_owner(&aid, &tid).await;
    let p = get(&ts, "/api/inbox?kind=reply&read=all").await;
    assert_eq!(p["items"][0]["gone"], true);
    assert_eq!(p["items"][0]["thread"]["id"], tid.as_str());
    assert_eq!(p["items"][0]["artifact"]["title"], "Board");
    let res = ts
        .authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base)))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let p = get(&ts, "/api/inbox?kind=published").await;
    assert_eq!(p["items"][0]["gone"], true);
    assert_eq!(p["items"][0]["artifact"]["id"], aid.as_str());
    assert_eq!(p["items"][0]["artifact"]["title"], Value::Null);
}

#[tokio::test]
async fn a_version_item_names_the_owners_threads_it_addressed() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    let res = ts
        .authed(
            ts.client
                .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
        )
        .header("x-clax-session", &sid)
        .json(
            &json!({"if_version": 1, "note": "Blue now", "addresses": [tid],
            "files": {"index.html": {"content": "<h1>Blue</h1>", "encoding": "utf8"}}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    let p = get(&ts, "/api/inbox?kind=version").await;
    let v = &p["items"][0];
    assert_eq!(v["version"]["n"], 2);
    assert_eq!(v["version"]["note"], "Blue now");
    assert_eq!(v["version"]["addressed"][0]["id"], tid.as_str());
    assert_eq!(v["url"], format!("/a/{aid}/v/2").as_str());
}

#[tokio::test]
async fn the_inbox_page_is_the_gallery() {
    let ts = TestServer::spawn().await;
    let a = ts.get("/").await;
    let b = ts.get("/inbox").await;
    assert_eq!(a.status(), b.status());
    assert_eq!(a.text().await.unwrap(), b.text().await.unwrap());
}

#[tokio::test]
async fn inbox_routes_refuse_lan_owner_cookies_artifact_origins_and_viewers() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "x").await;
    let id = get(&ts, "/api/inbox?kind=reply").await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (lan, base) = ts.lan();
    let mia = ts.viewer(Some("Mia")).await;
    let viewer = format!("clax_viewer={}", mia.cookie);
    let page = format!("http://{aid}.localhost:{}", ts.addr.port());
    let routes: Vec<(reqwest::Method, String, Option<Value>)> = vec![
        (reqwest::Method::GET, "/api/inbox".into(), None),
        (reqwest::Method::GET, "/api/inbox/summary".into(), None),
        (reqwest::Method::GET, format!("/api/inbox/{id}"), None),
        (
            reqwest::Method::POST,
            format!("/api/inbox/{id}/read"),
            Some(json!({})),
        ),
        (
            reqwest::Method::POST,
            format!("/api/inbox/{id}/unread"),
            Some(json!({})),
        ),
        (
            reqwest::Method::POST,
            "/api/inbox/read".into(),
            Some(json!({"all": true})),
        ),
    ];
    for (m, path, b) in &routes {
        let with_body = |r: reqwest::RequestBuilder| match b {
            Some(b) => r.json(b),
            None => r,
        };
        // A LAN peer presenting the owner cookie: the cookie needs a local peer.
        let res = with_body(lan.request(m.clone(), format!("{base}{path}")))
            .header("cookie", ts.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "LAN owner cookie, {m} {path}");
        // An artifact's own origin, with the owner cookie.
        let res = with_body(ts.client.request(m.clone(), format!("{}{path}", ts.base)))
            .header("cookie", ts.owner_cookie())
            .header("origin", &page)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "artifact origin, {m} {path}");
        // A local viewer cookie alone.
        let res = with_body(ts.client.request(m.clone(), format!("{}{path}", ts.base)))
            .header("cookie", &viewer)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "viewer cookie, {m} {path}");
    }
    let s = get(&ts, "/api/inbox/summary").await;
    assert_eq!(s["unread"], 2, "nothing was marked: {s}");
}

#[tokio::test]
async fn the_extension_may_list_read_and_follow_the_inbox() {
    let ts = TestServer::spawn().await;
    let ext = ts.extension().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "Done").await;
    let list: Value = ext.get("/api/inbox?kind=reply").await.json().await.unwrap();
    let id = list["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(ext.get("/api/inbox/summary").await.status(), 200);
    assert_eq!(ext.get(&format!("/api/inbox/{id}")).await.status(), 200);
    let mut events = clax_server::testing::EventReader::from_response(ext.get("/api/stream").await);
    let sid_stream = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ext
        .post(
            &format!("/api/stream/{sid_stream}"),
            json!({"subscribe": ["inbox"]}),
        )
        .await;
    assert_eq!(r.status(), 200);
    let r: Value = ext
        .post(&format!("/api/inbox/{id}/read"), json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(r["item"]["read"], true);
    let e = events.next_named("inbox_item").await;
    assert_eq!(
        (e["item"]["id"].as_str(), e["item"]["read"].as_bool()),
        (Some(id.as_str()), Some(true))
    );
    let r = ext.post("/api/inbox/read", json!({"all": true})).await;
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn the_hooks_timer_leaves_a_question_unread_and_the_owner_moving_it_reads_it() {
    let ts = TestServer::spawn().await;
    // An owner surface is open, so hook questions wait rather than go to
    // the terminal at once.
    let _surface = ts.stream_as_owner(&["questions"]).await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let hook = |t: &str| {
        json!({"source": "hook", "tool_use_id": t, "questions": [{"question": "Which?",
            "header": "Pick", "multiSelect": false, "options": [{"label": "A", "description": "a"},
            {"label": "B", "description": "b"}]}]})
    };
    let timer = ts.ask(&sid, hook("t1")).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let moved = ts.ask(&sid, hook("t2")).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = ts
        .post_json(
            &format!("/api/sessions/{sid}/questions/{timer}/release"),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let res = ts
        .client
        .post(format!("{}/api/questions/{moved}/release", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let items = get(&ts, "/api/inbox?kind=question").await;
    let read_of = |qid: &str| {
        items["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["question"]["id"] == qid)
            .unwrap()["read"]
            .as_bool()
    };
    assert_eq!(read_of(&timer), Some(false), "the hook's timer");
    assert_eq!(read_of(&moved), Some(true), "Answer in the terminal");
}
