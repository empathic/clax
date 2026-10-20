//! The app's jobs over HTTP (spec 2026-10-19-in-process-core-design §7.4,
//! §7.5, §8.1): one queue owner per client, tied to its event stream, and
//! publish and reply steps that commit with what they write.

use clax_testkit::{EventReader, TestServer};
use reqwest::Method;
use serde_json::{Value, json};

const OWNER: &str = "x-clax-jobs-owner";

async fn send(r: reqwest::RequestBuilder) -> (u16, Value) {
    let res = r.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

/// A request with the token.
fn authed(ts: &TestServer, m: Method, path: &str) -> reqwest::RequestBuilder {
    ts.authed(ts.client.request(m, format!("{}{path}", ts.base)))
}

/// A job write with the token and the queue owner's `token`.
fn owned(ts: &TestServer, m: Method, path: &str, token: &str) -> reqwest::RequestBuilder {
    authed(ts, m, path).header(OWNER, token)
}

/// Opens `/api/stream` with the token: the reader and the stream's ID.
async fn stream(ts: &TestServer) -> (EventReader, String) {
    let res = authed(ts, Method::GET, "/api/stream").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut events = EventReader::from_response(res);
    let id = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    (events, id)
}

/// Subscribes stream `sid` to `topics`.
async fn subscribe(ts: &TestServer, sid: &str, topics: &[&str]) {
    let (s, v) = send(
        authed(ts, Method::POST, &format!("/api/stream/{sid}")).json(&json!({"subscribe": topics})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

/// `POST /api/jobs/owner` for the app with stream `sid`.
async fn claim(ts: &TestServer, sid: &str) -> (u16, Value) {
    send(authed(ts, Method::POST, "/api/jobs/owner").json(&json!({"client": "app", "stream": sid})))
        .await
}

/// The app's queue owner: its stream (kept open) and token.
async fn owner(ts: &TestServer) -> (EventReader, String) {
    let (events, sid) = stream(ts).await;
    let (s, v) = claim(ts, &sid).await;
    assert_eq!(s, 200, "{v}");
    let token = v["owner"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 32);
    (events, token)
}

/// Creates job `id` of `kind` on `artifact` and moves it to `running`; its
/// row.
async fn running(
    ts: &TestServer,
    token: &str,
    id: &str,
    kind: &str,
    artifact: Option<&str>,
) -> Value {
    let (s, v) = send(owned(ts, Method::POST, "/api/jobs", token).json(&json!({
        "id": id, "client": "app", "kind": kind, "artifact_id": artifact,
        "title": "Budget", "input": {"request": "a budget"},
    })))
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["job"]["job"]["state"], "queued");
    let (s, v) = send(
        owned(ts, Method::PATCH, &format!("/api/jobs/{id}"), token)
            .json(&json!({"if_rev": 1, "state": "running"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    v["job"]["job"].clone()
}

/// A publish step's body: `html` as the version of `artifact` at `base`.
fn publish_body(artifact: Option<&str>, base: u32, html: &str, done: bool) -> Value {
    let mut b = json!({"publish": {
        "artifact_id": artifact, "base": base,
        "request": {"title": "Budget", "files": {"index.html": {"content": html, "encoding": "utf8"}}},
    }});
    if done {
        b["state"] = json!("done");
    }
    b
}

/// `POST /api/jobs/{id}/steps/{key}` with `body`.
async fn step(ts: &TestServer, token: &str, id: &str, key: &str, body: Value) -> (u16, Value) {
    send(
        owned(
            ts,
            Method::POST,
            &format!("/api/jobs/{id}/steps/{key}"),
            token,
        )
        .json(&body),
    )
    .await
}

async fn job(ts: &TestServer, id: &str) -> Value {
    let (s, v) = send(authed(ts, Method::GET, &format!("/api/jobs/{id}"))).await;
    assert_eq!(s, 200, "{v}");
    v["job"].clone()
}

async fn versions(ts: &TestServer, aid: &str) -> u64 {
    let (_, v) = send(authed(ts, Method::GET, &format!("/api/artifacts/{aid}"))).await;
    v["artifact"]["current_version"].as_u64().unwrap()
}

#[tokio::test]
async fn a_replayed_publish_step_publishes_once() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    running(&ts, &token, "j1", "make", None).await;
    let (s, first) = step(
        &ts,
        &token,
        "j1",
        "publish:0",
        publish_body(None, 0, "<p>a", true),
    )
    .await;
    assert_eq!(s, 200, "{first}");
    assert_eq!(
        (first["replayed"].as_bool(), first["version"].as_u64()),
        (Some(false), Some(1))
    );
    let (s, again) = step(
        &ts,
        &token,
        "j1",
        "publish:0",
        publish_body(None, 0, "<p>a", true),
    )
    .await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["replayed"], true);
    assert_eq!(again["version"], first["version"]);
    assert_eq!(again["step"], first["step"]);
    let (_, list) = send(authed(&ts, Method::GET, "/api/artifacts")).await;
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1, "{list}");
}

#[tokio::test]
async fn a_done_publish_step_ends_the_job_with_the_version() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    running(&ts, &token, "j1", "make", None).await;
    let (s, v) = step(
        &ts,
        &token,
        "j1",
        "publish:0",
        publish_body(None, 0, "<p>a", true),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let row = &v["job"]["job"];
    assert_eq!(
        (row["state"].as_str(), row["step"].as_str()),
        (Some("done"), Some("publish:0"))
    );
    let aid = v["step"]["result"]["artifact_id"].as_str().unwrap();
    assert_eq!(row["artifact_id"], aid);
    assert_eq!(versions(&ts, aid).await, 1);
    let (s, open) = send(authed(&ts, Method::GET, "/api/jobs?open=1&client=app")).await;
    assert_eq!(s, 200, "{open}");
    assert_eq!(open["jobs"], json!([]));
}

#[tokio::test]
async fn a_step_on_a_moved_base_records_nothing() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    let aid = ts.publish("App", &[("index.html", "<p>v1")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    running(&ts, &token, "j1", "change", Some(&aid)).await;
    let (s, v) = send(
        authed(&ts, Method::POST, &format!("/api/artifacts/{aid}/versions")).json(&json!({
            "if_version": 1, "files": {"index.html": {"content": "<p>v2"}}
        })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = step(
        &ts,
        &token,
        "j1",
        "publish:1",
        publish_body(Some(&aid), 1, "<p>x", false),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("version_moved")),
        "{v}"
    );
    assert_eq!(v["error"]["current"], 2);
    assert_eq!(job(&ts, "j1").await["steps"], json!([]));
    assert_eq!(versions(&ts, &aid).await, 2);
}

#[tokio::test]
async fn a_moved_document_refuses_the_whole_step() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    let (s, v) = send(authed(&ts, Method::POST, "/api/artifacts").json(&json!({
        "title": "Tracker", "capabilities": {"db": {}},
        "files": {"index.html": {"content": "<p>v1"}}
    })))
    .await;
    assert_eq!(s, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let put = |data: Value| {
        authed(
            &ts,
            Method::PUT,
            &format!("/api/artifacts/{aid}/docs/notes/n1"),
        )
        .json(&json!({"data": data, "lww": true}))
    };
    let (_, w) = send(put(json!({"t": 1}))).await;
    let planned = w["doc"]["version"].as_u64().unwrap();
    send(put(json!({"t": 2}))).await;
    running(&ts, &token, "j1", "change", Some(&aid)).await;
    let mut body = publish_body(Some(&aid), 1, "<p>v2", false);
    body["publish"]["docs"] =
        json!([{"op": "set", "path": "notes/n1", "data": {"t": 3}, "if_version": planned}]);
    let (s, v) = step(&ts, &token, "j1", "publish:1", body).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("doc_moved")),
        "{v}"
    );
    assert_eq!(v["error"]["path"], "notes/n1");
    assert_eq!(versions(&ts, &aid).await, 1);
    assert_eq!(job(&ts, "j1").await["steps"], json!([]));
}

#[tokio::test]
async fn a_reply_step_replies_resolves_and_acks_once() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    let sess = ts.register_session("claude", "h1").await;
    let sid = sess["id"].as_str().unwrap();
    let aid = ts
        .publish_as(sid, "App", "<main><h2>Quarterly goals</h2></main>")
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread(&aid, 1, "make it green").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.send_thread(&aid, &tid).await;
    running(&ts, &token, "j1", "change", Some(&aid)).await;
    let body = json!({"reply": {"session_id": sid, "threads": [tid], "body": "Made it green.",
        "resolve": true}, "state": "done"});
    let (s, v) = step(&ts, &token, "j1", "reply:0", body.clone()).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["replayed"].as_bool(), v["job"]["job"]["state"].as_str()),
        (Some(false), Some("done"))
    );
    let (s, again) = step(&ts, &token, "j1", "reply:0", body).await;
    assert_eq!(
        (s, again["replayed"].as_bool()),
        (200, Some(true)),
        "{again}"
    );
    let (_, t) = send(authed(
        &ts,
        Method::GET,
        &format!("/api/artifacts/{aid}/threads/{tid}"),
    ))
    .await;
    let t = &t["thread"];
    assert_eq!(t["status"], "resolved", "{t}");
    let agent = t["comments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["author_kind"] == "agent")
        .count();
    assert_eq!(agent, 1, "{t}");
}

#[tokio::test]
async fn jobs_are_owner_only() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    running(&ts, &token, "j1", "make", None).await;
    let viewer = ts.viewer(Some("Sam")).await;
    let routes = [
        (Method::POST, "/api/jobs/owner"),
        (Method::DELETE, "/api/jobs/owner"),
        (Method::POST, "/api/jobs"),
        (Method::GET, "/api/jobs?open=1&client=app"),
        (Method::GET, "/api/jobs/j1"),
        (Method::PATCH, "/api/jobs/j1"),
        (Method::PUT, "/api/jobs/j1/threads/t1"),
        (Method::DELETE, "/api/jobs/j1/threads/t1"),
        (Method::PUT, "/api/jobs/j1/questions/q1"),
        (Method::POST, "/api/jobs/j1/steps/turn:1"),
        (Method::GET, "/api/agent-read-consents"),
        (Method::PUT, "/api/agent-read-consents/a/b"),
        (Method::DELETE, "/api/agent-read-consents/a/b"),
    ];
    for (m, p) in routes {
        let url = format!("{}{p}", ts.base);
        // The owner's agent: the token with a session.
        let (s, v) = send(
            ts.authed(ts.client.request(m.clone(), &url))
                .header("x-clax-session", "s1")
                .header(OWNER, &token)
                .json(&json!({})),
        )
        .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (403, Some("not_for_agents")),
            "{m} {p}"
        );
        // A viewer, and no credential.
        let (s, _) = send(
            ts.client
                .request(m.clone(), &url)
                .header("cookie", viewer.cookie.clone())
                .json(&json!({})),
        )
        .await;
        assert!(matches!(s, 401 | 403 | 404), "{m} {p}: {s}");
        let (s, _) = send(ts.client.request(m.clone(), &url).json(&json!({}))).await;
        assert!(matches!(s, 401 | 403 | 404), "{m} {p}: {s}");
    }
    assert_eq!(job(&ts, "j1").await["job"]["rev"], 2, "nothing changed");
}

#[tokio::test]
async fn a_second_claimant_is_refused_while_the_first_stream_lives() {
    let ts = TestServer::spawn().await;
    let (_a, first) = stream(&ts).await;
    let (_b, second) = stream(&ts).await;
    let (s, v) = claim(&ts, &first).await;
    assert_eq!(s, 200, "{v}");
    let (s, again) = claim(&ts, &first).await;
    assert_eq!(
        (s, &again["owner"]),
        (200, &v["owner"]),
        "the holder's own stream"
    );
    let (s, v) = claim(&ts, &second).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("owner_held")),
        "{v}"
    );
}

#[tokio::test]
async fn the_claim_is_released_when_its_stream_ends() {
    let ts = TestServer::spawn().await;
    let (a, first) = stream(&ts).await;
    let (_b, second) = stream(&ts).await;
    let (mut c, third) = stream(&ts).await;
    subscribe(&ts, &third, &["jobs"]).await;
    let (s, v) = claim(&ts, &first).await;
    assert_eq!(s, 200, "{v}");
    let old = v["owner"].as_str().unwrap().to_string();
    assert_eq!(
        c.next_named("jobs_owner").await,
        json!({"topic": "jobs", "client": "app", "held": true})
    );
    running(&ts, &old, "j1", "make", None).await;
    drop(a);
    // The claim finds the holder and nudges its connection, whose write
    // fails: the stream ends and the claim is released.
    let (s, v) = claim(&ts, &second).await;
    assert!(s == 200 || v["error"]["code"] == "owner_held", "{s} {v}");
    assert_eq!(c.next_named("jobs_owner").await["held"], false);
    let (s, v) = claim(&ts, &second).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = send(
        owned(&ts, Method::PATCH, "/api/jobs/j1", &old)
            .json(&json!({"if_rev": 2, "state": "paused"})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "{v}"
    );
}

#[tokio::test]
async fn a_write_without_the_owners_token_is_refused() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    running(&ts, &token, "j1", "make", None).await;
    let patch = json!({"if_rev": 2, "state": "paused"});
    let (s, v) = send(authed(&ts, Method::PATCH, "/api/jobs/j1").json(&patch)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "{v}"
    );
    let (s, v) = send(
        owned(
            &ts,
            Method::PATCH,
            "/api/jobs/j1",
            "0123456789abcdef0123456789abcdef",
        )
        .json(&patch),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "{v}"
    );
    // Another client's token holds only that client's queue.
    let (_x, sid) = stream(&ts).await;
    let (s, v) = send(
        authed(&ts, Method::POST, "/api/jobs/owner").json(&json!({"client": "cli", "stream": sid})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let theirs = v["owner"].as_str().unwrap();
    let (s, v) = send(owned(&ts, Method::PATCH, "/api/jobs/j1", theirs).json(&patch)).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "{v}"
    );
    let (s, v) = send(owned(&ts, Method::POST, "/api/jobs", theirs).json(&json!({
        "id": "j2", "client": "app", "kind": "make", "input": {}
    })))
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "{v}"
    );
    assert_eq!(
        job(&ts, "j1").await["job"]["state"],
        "running",
        "nothing changed"
    );
    let (s, _) = send(authed(&ts, Method::GET, "/api/jobs/j2")).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn a_claim_naming_no_live_stream_is_refused() {
    let ts = TestServer::spawn().await;
    let (s, v) = claim(&ts, "0123456789abcdef0123456789abcdef").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("no_such_stream")),
        "{v}"
    );
}

#[tokio::test]
async fn a_restarted_core_holds_no_claim() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    running(&ts, &token, "j1", "make", None).await;
    let again = ts.reopened().await;
    let (_n, sid) = stream(&again).await;
    let (s, v) = claim(&again, &sid).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = send(
        owned(&again, Method::PATCH, "/api/jobs/j1", &token)
            .json(&json!({"if_rev": 2, "state": "paused"})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("not_owner")),
        "the old token: {v}"
    );
}

#[tokio::test]
async fn the_jobs_topic_hears_each_change() {
    let ts = TestServer::spawn().await;
    let (mut events, sid) = stream(&ts).await;
    subscribe(&ts, &sid, &["jobs"]).await;
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(s, 200, "{v}");
    let token = v["owner"].as_str().unwrap().to_string();
    assert_eq!(events.next_named("jobs_owner").await["held"], true);
    running(&ts, &token, "j1", "make", None).await;
    let (s, v) = step(
        &ts,
        &token,
        "j1",
        "turn:1",
        json!({"result": {"calls": []}}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let mut seen = vec![];
    for _ in 0..3 {
        let e = events.next_named("job").await;
        assert_eq!(
            (e["id"].as_str(), e["client"].as_str()),
            (Some("j1"), Some("app")),
            "{e}"
        );
        seen.push((
            e["rev"].as_i64().unwrap(),
            e["state"].as_str().unwrap().to_string(),
        ));
    }
    assert_eq!(
        seen,
        vec![
            (1, "queued".into()),
            (2, "running".into()),
            (3, "running".into())
        ]
    );
}
