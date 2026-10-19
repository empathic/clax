//! The owner's inbox over HTTP (spec 2026-10-06-agent-questions-and-inbox
//! §8): the routes, the `inbox` stream topic, finished work and read marks.

use clax_testkit::TestServer;
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
    let mut events = clax_testkit::EventReader::from_response(ext.get("/api/stream").await);
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
    // An owner surface is open and the session has a page, so hook
    // questions wait rather than go to the terminal at once.
    let _surface = ts.stream_as_owner(&["questions"]).await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.publish_as(&sid, "Board", "<p>b").await;
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
    assert_eq!(
        read_of(&moved),
        Some(true),
        "the owner moving it to the terminal"
    );
}

/// A daemon on the LAN with Sam connected (`clax:collaborator`): Sam's
/// machine, his viewer cookie and public ID, and his agent's session.
struct Sam {
    ts: TestServer,
    sam: clax_testkit::TestClient,
    cookie: String,
    public_id: String,
    session: String,
}

async fn with_sam() -> Sam {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sam = ts.connect_person("Sam", "clax:collaborator").await;
    let v = ts.store.viewer_of_grant(&sam.grant_id()).unwrap();
    let session = sam.register("claude", "sam-h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cookie = ts.browser_cookie(&v.id);
    Sam {
        ts,
        sam,
        cookie,
        public_id: v.public_id,
        session,
    }
}

impl Sam {
    /// `req` from the browser whose viewer cookie is `cookie`.
    fn browser(cookie: &str, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("cookie", format!("clax_viewer={cookie}"))
    }

    /// `GET path` from the browser of `cookie`: (status, body).
    async fn get_as(&self, cookie: &str, path: &str) -> (u16, Value) {
        let r = Sam::browser(
            cookie,
            self.ts.client.get(format!("{}{path}", self.ts.base)),
        )
        .send()
        .await
        .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    /// `POST path` with `body` from the browser of `cookie`: (status, body).
    async fn post_as(&self, cookie: &str, path: &str, body: Value) -> (u16, Value) {
        let r = Sam::browser(
            cookie,
            self.ts.client.post(format!("{}{path}", self.ts.base)),
        )
        .json(&body)
        .send()
        .await
        .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    /// Sam's agent asks one question (about `artifact`, when given); its ID.
    async fn ask(&self, text: &str, artifact: Option<&str>) -> String {
        let mut body = json!({"source": "ask", "questions": [{"question": text, "header": "Colour",
            "options": [{"label": "Teal"}, {"label": "Amber"}]}]});
        if let Some(a) = artifact {
            body["artifact_id"] = json!(a);
        }
        let r = self
            .sam
            .agent(
                "POST",
                &format!("/api/sessions/{}/questions", self.session),
                Some(&self.session),
                Some(body),
            )
            .await;
        assert_eq!(r.status(), 201);
        r.json::<Value>().await.unwrap()["question"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Sam's browser opens `/api/stream` and follows `topics`.
    async fn stream(&self, topics: &[&str]) -> clax_testkit::EventReader {
        let res = Sam::browser(
            &self.cookie,
            self.ts.client.get(format!("{}/api/stream", self.ts.base)),
        )
        .send()
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
        let mut ev = clax_testkit::EventReader::from_response(res);
        let id = ev.next_named("ready").await["stream"]
            .as_str()
            .unwrap()
            .to_string();
        let (status, body) = self
            .post_as(
                &self.cookie,
                &format!("/api/stream/{id}"),
                json!({"subscribe": topics}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        ev
    }
}

/// The IDs of `v`'s items.
fn item_ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_person_sees_only_their_inbox() {
    let w = with_sam().await;
    let mia = w.ts.connect_person("Mia", "clax:collaborator").await;
    let mia_cookie =
        w.ts.browser_cookie(&w.ts.store.viewer_of_grant(&mia.grant_id()).unwrap().id);
    let qid = w.ask("Teal or amber?", None).await;

    let (status, sams) = w.get_as(&w.cookie, "/api/inbox").await;
    assert_eq!(status, 200, "{sams}");
    let items = sams["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{sams}");
    assert_eq!(items[0]["kind"], "question");
    assert_eq!(items[0]["question"]["id"], qid.as_str());
    assert_eq!(sams["unread"], 1);
    let item = items[0]["id"].as_str().unwrap().to_string();

    // Mia's inbox and the owner's hold none of it.
    let (status, mias) = w.get_as(&mia_cookie, "/api/inbox").await;
    assert_eq!((status, item_ids(&mias).len()), (200, 0), "{mias}");
    let owners = get(&w.ts, "/api/inbox").await;
    assert!(
        owners["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["kind"] == "connection"),
        "the owner hears of the machines that connected alone: {owners}"
    );
    let path = format!("/api/inbox/{item}");
    assert_eq!(w.ts.get_authed(&path).await.status(), 404);
    assert_eq!(w.get_as(&mia_cookie, &path).await.0, 404);
    let (status, one) = w.get_as(&w.cookie, &path).await;
    assert_eq!(
        (status, one["item"]["id"].as_str()),
        (200, Some(item.as_str()))
    );

    // The summary and the questions are Sam's own.
    let (_, summary) = w.get_as(&w.cookie, "/api/inbox/summary").await;
    assert_eq!(summary["unread"], 1);
    assert_eq!(summary["questions"][0]["id"], qid.as_str());
    let (_, qs) = w.get_as(&w.cookie, "/api/questions").await;
    assert_eq!(
        (qs["open"].as_u64(), qs["questions"][0]["id"].as_str()),
        (Some(1), Some(qid.as_str()))
    );
    let owners_qs = get(&w.ts, "/api/questions?status=all").await;
    assert_eq!(owners_qs["questions"], json!([]));

    // Marks reach the caller's own items only.
    let r =
        w.ts.authed(w.ts.client.post(format!("{}{path}/read", w.ts.base)))
            .send()
            .await
            .unwrap();
    assert_eq!(r.status(), 404);
    let marked =
        w.ts.post_json("/api/inbox/read", json!({"ids": [item]}))
            .await
            .json::<Value>()
            .await
            .unwrap();
    assert_eq!(marked["marked"], 0, "{marked}");
    let (status, mine) = w
        .post_as(&w.cookie, &format!("{path}/read"), json!({}))
        .await;
    assert_eq!((status, mine["unread"].as_u64()), (200, Some(0)), "{mine}");
    let (status, all) = w
        .post_as(&w.cookie, "/api/inbox/read", json!({"all": true}))
        .await;
    assert_eq!((status, all["unread"].as_u64()), (200, Some(0)), "{all}");
    assert_eq!(
        get(&w.ts, "/api/inbox/summary").await["unread"],
        2,
        "the owner's stay unread"
    );

    // Sam answers his agent's question; the owner cannot.
    let answer = json!({"answers": [{"selected": ["Teal"]}]});
    let r =
        w.ts.post_json(&format!("/api/questions/{qid}/answer"), answer.clone())
            .await;
    assert_eq!(r.status(), 404);
    let (status, q) = w
        .post_as(&w.cookie, &format!("/api/questions/{qid}/answer"), answer)
        .await;
    assert_eq!(status, 200, "{q}");
    assert_eq!(
        (
            q["question"]["status"].as_str(),
            q["question"]["answered_via"].as_str()
        ),
        (Some("answered"), Some("shell"))
    );
}

#[tokio::test]
async fn the_owner_inbox_is_unchanged() {
    let w = with_sam().await;
    let ts = &w.ts;
    // The owner's agent publishes and replies in the owner's thread; Sam's
    // agent asks.
    let (sid, aid, tid) = owner_thread(ts).await;
    ts.reply_as_agent(&sid, &aid, &tid, "Done: blue").await;
    w.ask("Teal or amber?", None).await;
    let owners = get(ts, "/api/inbox").await;
    let kinds: Vec<&str> = owners["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["reply", "published", "connection"]);
    let reply = &owners["items"][0];
    let mut keys: Vec<&str> = reply
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "agent",
            "artifact",
            "created_at",
            "gone",
            "id",
            "kind",
            "published",
            "question",
            "read",
            "reply",
            "seq",
            "thread",
            "url",
            "version",
            "work"
        ],
        "an item's view keeps its fields"
    );
    assert_eq!(reply["reply"]["body"], "Done: blue");
    assert_eq!(owners["unread"], 3);
    // The connection item names the machine and its person.
    let c = &owners["items"][2];
    assert_eq!(c["connection"]["client"]["name"], "test-machine", "{c}");
    assert_eq!(c["connection"]["person"]["display_name"], "Sam");
    assert_eq!(c["url"], "/people");
    // The owner's look reads the owner's reply alone.
    ts.look_as_owner(&aid, &[&tid]).await;
    assert_eq!(get(ts, "/api/inbox/summary").await["unread"], 2);
    let (_, sams) = w.get_as(&w.cookie, "/api/inbox/summary").await;
    assert_eq!(sams["unread"], 1);
}

#[tokio::test]
async fn a_persons_stream_carries_their_own_items() {
    let w = with_sam().await;
    let mut sams = w.stream(&["inbox", "questions"]).await;
    let mut owners = w.ts.stream_as_owner(&["inbox", "questions"]).await;
    let qid = w.ask("Teal or amber?", None).await;
    // The question's event and its item's arrive in either order.
    let (mut question, mut item) = (None, None);
    while question.is_none() || item.is_none() {
        match sams.next().await {
            (n, e) if n == "question" => question = Some(e),
            (n, e) if n == "inbox_item" => item = Some(e),
            _ => {}
        }
    }
    assert_eq!(question.unwrap()["question"]["id"], qid.as_str());
    let e = item.unwrap();
    assert_eq!(
        (e["item"]["question"]["id"].as_str(), e["unread"].as_u64()),
        (Some(qid.as_str()), Some(1))
    );
    // The owner's stream hears its own next item, never Sam's.
    let (sid, aid, tid) = owner_thread(&w.ts).await;
    w.ts.reply_as_agent(&sid, &aid, &tid, "Done").await;
    let e = owners.next_named("inbox_item").await;
    assert_eq!(e["item"]["kind"], "published", "{e}");
    // A person's browser follows only their own topics; a LAN viewer none.
    let lan = w.ts.viewer(Some("Lan")).await;
    let (status, _) = w.get_as(&lan.cookie, "/api/inbox").await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn a_revoked_person_loses_the_inbox() {
    let w = with_sam().await;
    w.ask("Teal or amber?", None).await;
    let mut sams = w.stream(&["inbox", "questions"]).await;
    assert_eq!(w.get_as(&w.cookie, "/api/inbox").await.0, 200);
    w.ts.store
        .revoke_person(&clax_core::audit::AuditCtx::DAEMON, &w.public_id)
        .unwrap();
    // The stream ends, and the routes refuse the browser.
    sams.rest().await;
    for path in ["/api/inbox", "/api/inbox/summary", "/api/questions"] {
        assert_eq!(w.get_as(&w.cookie, path).await.0, 403, "{path}");
    }
    let res = Sam::browser(
        &w.cookie,
        w.ts.client.get(format!("{}/api/stream", w.ts.base)),
    )
    .send()
    .await
    .unwrap();
    let mut ev = clax_testkit::EventReader::from_response(res);
    let id = ev.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = w
        .post_as(
            &w.cookie,
            &format!("/api/stream/{id}"),
            json!({"subscribe": ["inbox"]}),
        )
        .await;
    assert_eq!(status, 403);
    assert_eq!(
        get(&w.ts, "/api/inbox").await["unread"],
        1,
        "the owner's inbox stays"
    );
}

#[tokio::test]
async fn another_persons_agents_project_never_reaches_a_persons_inbox() {
    let w = with_sam().await;
    let mut sams = w.stream(&["inbox"]).await;
    // The owner's agent works in /home/alex/secret-project and replies in
    // Sam's thread.
    let res = w
        .ts
        .post_json(
            "/api/sessions",
            json!({"harness": "claude", "harness_session_id": "own2", "cwd": "/home/alex/secret-project"}),
        )
        .await;
    let sid = res.json::<Value>().await.unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let a = w.ts.publish_as(&sid, "Board", "<h1>Board</h1>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = w.ts.thread_as(&aid, &w.cookie, "Sam asks").await;
    let tid = t["id"].as_str().unwrap().to_string();
    w.ts.send_thread(&aid, &tid).await;
    w.ts.reply_as_agent(&sid, &aid, &tid, "done").await;
    let e = sams.next_named("inbox_item").await;
    assert_eq!(e["item"]["kind"], "reply");
    assert!(!e.to_string().contains("secret-project"), "{e}");
    let (_, inbox) = w.get_as(&w.cookie, "/api/inbox").await;
    assert!(!inbox.to_string().contains("secret-project"), "{inbox}");
    assert!(inbox["items"][0]["agent"]["project"].is_null());
    // The owner's inbox names it; Sam's own agent's project reaches Sam.
    let owners = get(&w.ts, "/api/inbox?kind=published").await;
    assert_eq!(owners["items"][0]["agent"]["project"], "secret-project");
    w.ask("Teal or amber?", None).await;
    let (_, inbox) = w.get_as(&w.cookie, "/api/inbox?kind=question").await;
    assert_eq!(inbox["items"][0]["agent"]["project"], "w");
}

#[tokio::test]
async fn a_persons_agents_have_their_own_question_cap() {
    let w = with_sam().await;
    let ask = json!({"source": "ask", "questions": [{"question": "q", "header": "H",
        "options": [{"label": "A"}, {"label": "B"}]}]});
    let mut n = 0;
    'outer: for s in 0..20 {
        let sid = w.sam.register("claude", &format!("sam-f{s}")).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        for _ in 0..8 {
            let r = w
                .sam
                .agent(
                    "POST",
                    &format!("/api/sessions/{sid}/questions"),
                    Some(&sid),
                    Some(ask.clone()),
                )
                .await;
            if r.status() != 201 {
                assert_eq!(r.status(), 429);
                break 'outer;
            }
            n += 1;
        }
    }
    assert_eq!(n, 100, "Sam's agents reach their own cap");
    let sid = w.ts.register_session("claude", "own-cap").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r =
        w.ts.post_json(&format!("/api/sessions/{sid}/questions"), ask)
            .await;
    assert_eq!(r.status(), 201, "the owner's agents keep theirs");
}

#[tokio::test]
async fn the_agent_strip_groups_the_callers_inbox_by_agent() {
    let w = with_sam().await;
    let qid = w.ask("Teal or amber?", None).await;
    let (status, strip) = w.get_as(&w.cookie, "/api/inbox?group=agent").await;
    assert_eq!(status, 200, "{strip}");
    let agents = strip["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 1, "{strip}");
    let a = &agents[0];
    assert_eq!(
        (
            a["harness"].as_str(),
            a["live"].as_bool(),
            a["unread"].as_u64()
        ),
        (Some("claude"), Some(true), Some(1))
    );
    assert_eq!(a["blocked"]["questions"], 1);
    assert_eq!(a["blocked"]["approvals"], 0);
    assert!(a["blocked"]["since"].is_string());
    assert!(a["handle"].as_str().unwrap().starts_with("a_"));
    assert_eq!(strip["unread"], 1);
    // The chip filters the inbox by the agent's handle.
    let (_, items) = w
        .get_as(
            &w.cookie,
            &format!("/api/inbox?agent={}", a["handle"].as_str().unwrap()),
        )
        .await;
    assert_eq!(items["items"][0]["question"]["id"], qid.as_str());
    // The owner's strip holds none of Sam's agents.
    assert_eq!(
        get(&w.ts, "/api/inbox?group=agent").await["agents"],
        json!([])
    );
    // Another grouping, or other parameters beside it, are refused.
    for q in ["group=thread", "group=agent&read=unread", "group="] {
        let (status, body) = w.get_as(&w.cookie, &format!("/api/inbox?{q}")).await;
        assert_eq!(
            (status, body["error"]["code"].as_str()),
            (400, Some("invalid_query")),
            "{q}"
        );
    }
}

#[tokio::test]
async fn the_summary_lists_a_waiting_connection_request() {
    let w = with_sam().await;
    let summary = get(&w.ts, "/api/inbox/summary").await;
    let latest = summary["latest"].as_array().unwrap();
    assert_eq!(latest.len(), 1, "{summary}");
    assert_eq!(latest[0]["kind"], "connection");
}

#[tokio::test]
async fn group_by_artifact_counts_every_match_and_pages_each_group() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = owner_thread(&ts).await;
    for i in 0..5 {
        ts.reply_as_agent(&sid, &aid, &tid, &format!("note {i}"))
            .await;
    }
    let other = ts.publish_as(&sid, "Pricing", "<h1>Pricing</h1>").await;
    let other = other["artifact"]["id"].as_str().unwrap().to_string();
    // A question about no page: the group without an artifact.
    let q = ts
        .ask(
            &sid,
            json!({"source": "ask", "questions": [{"question": "Which?", "header": "Pick",
                "options": [{"label": "A"}, {"label": "B"}]}]}),
        )
        .await;
    assert!(q["question"]["artifact"].is_null(), "{q}");
    ts.reply_as_agent(&sid, &aid, &tid, "the newest note").await;

    let g = get(&ts, "/api/inbox?group=artifact&read=unread&rows=2").await;
    let groups = g["groups"].as_array().unwrap();
    let shape: Vec<_> = groups
        .iter()
        .map(|g| {
            (
                g["artifact"]["id"].as_str().map(str::to_string),
                g["total"].as_u64().unwrap(),
                g["unread"].as_u64().unwrap(),
                g["items"].as_array().unwrap().len(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (Some(aid.clone()), 7, 7, 2),
            (None, 1, 1, 1),
            (Some(other.clone()), 1, 1, 1),
        ],
        "Board: its publication and six replies; newest group first: {g}"
    );
    assert_eq!(groups[0]["artifact"]["title"], "Board");
    assert_eq!(groups[0]["items"][0]["reply"]["body"], "the newest note");
    assert_eq!(groups[0]["newest_at"], groups[0]["items"][0]["created_at"]);
    assert_eq!(groups[1]["items"][0]["kind"], "question");
    assert_eq!(
        (g["total"].as_u64(), g["unread"].as_u64()),
        (Some(9), Some(9))
    );
    assert!(g["next_cursor"].is_null());

    // The rest of a group: the plain list with its artifact and cursor.
    let next = groups[0]["next_cursor"].as_str().unwrap();
    let rest = get(
        &ts,
        &format!("/api/inbox?read=unread&artifact={aid}&before={next}"),
    )
    .await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 5);
    let none = get(&ts, "/api/inbox?artifact=none").await;
    assert_eq!(none["total"], 1);
    assert_eq!(none["items"][0]["kind"], "question");

    // Groups page by their newest match.
    let one = get(&ts, "/api/inbox?group=artifact&limit=2").await;
    assert_eq!(one["groups"].as_array().unwrap().len(), 2);
    let next = one["next_cursor"].as_str().unwrap();
    let two = get(
        &ts,
        &format!("/api/inbox?group=artifact&limit=2&before={next}"),
    )
    .await;
    assert_eq!(two["groups"][0]["artifact"]["id"], other.as_str());
    assert!(two["next_cursor"].is_null());

    // A group's mark-all reads that group alone; the page filter gives one group.
    let marked = |filter: Value| {
        let ts = &ts;
        async move {
            ts.post_json("/api/inbox/read", json!({"all": true, "filter": filter}))
                .await
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let r = marked(json!({"artifact": aid})).await;
    assert_eq!(
        (r["marked"].as_u64(), r["unread"].as_u64()),
        (Some(7), Some(2))
    );
    let r = marked(json!({"artifact": "none"})).await;
    assert_eq!(
        (r["marked"].as_u64(), r["unread"].as_u64()),
        (Some(1), Some(1))
    );
    let read = get(
        &ts,
        &format!("/api/inbox?group=artifact&read=read&artifact={aid}"),
    )
    .await;
    let groups = read["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        (groups[0]["total"].as_u64(), groups[0]["unread"].as_u64()),
        (Some(7), Some(0))
    );
    assert_eq!(
        groups[0]["items"].as_array().unwrap().len(),
        3,
        "three rows by default"
    );

    for bad in [
        "/api/inbox?rows=2",
        "/api/inbox?group=artifact&rows=x",
        "/api/inbox?group=agent&rows=2",
        "/api/inbox?group=page",
        "/api/inbox?artifact=nothing",
    ] {
        assert_eq!(ts.get_authed(bad).await.status(), 400, "{bad}");
    }
}
