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
    let body = json!({"reply": {"threads": [tid], "body": "Made it green.",
        "resolve": true}, "state": "done"});
    // No publish step yet: no session to reply from.
    let (s, v) = step(&ts, &token, "j1", "reply", body.clone()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_step")),
        "{v}"
    );
    let mut b = publish_body(Some(&aid), 1, "<p>green", false);
    b["publish"]["session_id"] = json!(sid);
    let (s, v) = step(&ts, &token, "j1", "publish:1", b).await;
    assert_eq!(s, 200, "{v}");
    // The app is relaunched: its new session is not the one the reply
    // comes from.
    ts.register_session("codex", "h2").await;
    let mut named = body.clone();
    named["reply"]["session_id"] = json!(sid);
    let (s, v) = step(&ts, &token, "j1", "reply:0", named).await;
    assert_eq!(s, 400, "a reply step names no session: {v}");
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
    let author = t["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["author_kind"] == "agent")
        .map(|c| c["author_name"].clone());
    assert_eq!(
        author,
        Some(json!("claude")),
        "from the publishing session: {t}"
    );
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
    // fails: the stream ends and the claim is released. A claim made once
    // the connection is gone replaces it at once, after the release or
    // before it (which then finds the claim replaced and says nothing).
    let (s, v) = claim(&ts, &second).await;
    if s == 200 {
        let mut held = c.next_named("jobs_owner").await["held"].clone();
        if held == false {
            held = c.next_named("jobs_owner").await["held"].clone();
        }
        assert_eq!(held, true);
    } else {
        assert_eq!(v["error"]["code"], "owner_held", "{s} {v}");
        assert_eq!(c.next_named("jobs_owner").await["held"], false);
    }
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

/// Content in memory whose version writes wait while the gate is closed.
struct Gate {
    inner: clax_core::content::memory::MemContent,
    open: tokio::sync::watch::Sender<bool>,
    entered: tokio::sync::Notify,
}

impl Gate {
    fn new() -> Gate {
        Gate {
            inner: clax_core::content::memory::MemContent::new(),
            open: tokio::sync::watch::channel(true).0,
            entered: tokio::sync::Notify::new(),
        }
    }

    fn set_open(&self, open: bool) {
        self.open.send_replace(open);
    }
}

#[async_trait::async_trait]
impl clax_core::content::ContentStore for Gate {
    async fn read_file(
        &self,
        id: &clax_core::ArtifactId,
        n: u32,
        path: &str,
    ) -> clax_core::Result<Option<Vec<u8>>> {
        self.inner.read_file(id, n, path).await
    }
    async fn write_version(
        &self,
        id: &clax_core::ArtifactId,
        n: u32,
        write: &clax_core::content::VersionWrite<'_>,
    ) -> clax_core::Result<()> {
        let mut open = self.open.subscribe();
        if !*open.borrow() {
            self.entered.notify_one();
            let _ = open.wait_for(|o| *o).await;
        }
        self.inner.write_version(id, n, write).await
    }
    async fn remove_version(&self, id: &clax_core::ArtifactId, n: u32) -> clax_core::Result<()> {
        self.inner.remove_version(id, n).await
    }
    async fn delete_artifact(&self, id: &clax_core::ArtifactId) -> clax_core::Result<()> {
        self.inner.delete_artifact(id).await
    }
    async fn put_asset(
        &self,
        key: &clax_core::content::AssetKey,
        bytes: &[u8],
    ) -> clax_core::Result<()> {
        self.inner.put_asset(key, bytes).await
    }
    async fn read_asset(
        &self,
        key: &clax_core::content::AssetKey,
    ) -> clax_core::Result<Option<Vec<u8>>> {
        self.inner.read_asset(key).await
    }
    async fn delete_asset(&self, key: &clax_core::content::AssetKey) -> clax_core::Result<()> {
        self.inner.delete_asset(key).await
    }
}

/// The next `jobs_owner` event `events` hears: a bound for a hang (the
/// event never coming), not a time budget.
async fn next_owner_event(events: &mut EventReader) -> Value {
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        events.next_named("jobs_owner"),
    )
    .await
    .expect("a jobs_owner event")
}

#[tokio::test]
async fn a_new_claim_drains_at_once_and_holds_the_queue_once_the_old_write_has_landed() {
    let gate = std::sync::Arc::new(Gate::new());
    let ts = TestServer::spawn_on_one_connection(gate.clone()).await;
    let (_a, old) = owner(&ts).await;
    running(&ts, &old, "j1", "make", None).await;
    // A publish step admitted under the old claim, held in its content write.
    gate.set_open(false);
    let write = owned(&ts, Method::POST, "/api/jobs/j1/steps/publish:0", &old)
        .json(&publish_body(None, 0, "<p>a", true));
    let write = tokio::spawn(send(write));
    gate.entered.notified().await;
    let (s, _) = send(owned(&ts, Method::DELETE, "/api/jobs/owner", &old)).await;
    assert_eq!(s, 204);
    // Released at once: the old token's next write is refused.
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
    // The new claim answers at once, draining, and so does its stream's
    // next claim, a retry of the first or one made while it was in flight.
    let (mut b, sid) = stream(&ts).await;
    let first = tokio::spawn({
        let r = authed(&ts, Method::POST, "/api/jobs/owner")
            .json(&json!({"client": "app", "stream": sid}));
        send(r)
    });
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("owner_draining")),
        "{v}"
    );
    let (s, v) = first.await.unwrap();
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("owner_draining")),
        "{v}"
    );
    assert!(!write.is_finished(), "the old write still runs");
    gate.set_open(true);
    let (s, v) = write.await.unwrap();
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["version"], 1);
    // The drain's end is heard on the claimant's stream, and it claims.
    assert_eq!(
        next_owner_event(&mut b).await,
        json!({"topic": "jobs", "client": "app", "held": true})
    );
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(s, 200, "{v}");
    let new = v["owner"].as_str().unwrap();
    assert_ne!(new, old);
    assert_eq!(job(&ts, "j1").await["job"]["state"], "done");
}

/// A job write whose request is dropped while the store runs it, on the
/// daemon's pool: the write goes on on its store thread, and its guard
/// with it, so a new claim drains until it lands.
#[tokio::test]
async fn a_dropped_requests_write_holds_a_new_claim_until_it_lands() {
    use tower::ServiceExt;
    let gate = std::sync::Arc::new(Gate::new());
    let ts = TestServer::spawn_on_pool_with_content(gate.clone()).await;
    let (_a, old) = owner(&ts).await;
    running(&ts, &old, "j1", "make", None).await;
    gate.set_open(false);
    let req = axum::http::Request::post("/api/jobs/j1/steps/publish:0")
        .header("host", format!("127.0.0.1:{}", ts.addr.port()))
        .header("authorization", format!("Bearer {}", ts.token))
        .header("content-type", "application/json")
        .header(OWNER, &old)
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))))
        .body(axum::body::Body::from(
            publish_body(None, 0, "<p>a", true).to_string(),
        ))
        .unwrap();
    let call = clax_server::router::build_router(ts.state.clone()).oneshot(req);
    let mut call = Box::pin(call);
    tokio::select! {
        _ = gate.entered.notified() => {}
        res = &mut call => panic!("answered before its content write: {res:?}"),
    }
    // The handler is dropped with its write in the store's content write.
    drop(call);
    let (s, _) = send(owned(&ts, Method::DELETE, "/api/jobs/owner", &old)).await;
    assert_eq!(s, 204);
    let (mut b, sid) = stream(&ts).await;
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("owner_draining")),
        "the dropped request's write still runs: {v}"
    );
    gate.set_open(true);
    assert_eq!(
        next_owner_event(&mut b).await,
        json!({"topic": "jobs", "client": "app", "held": true})
    );
    // It landed before the new owner holds the queue.
    assert_eq!(job(&ts, "j1").await["job"]["state"], "done");
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn a_claimant_refused_owner_held_hears_the_release() {
    let ts = TestServer::spawn().await;
    let (_a, holder) = owner(&ts).await;
    // The second claimant's stream follows no topic of its own.
    let (mut waiting, sid) = stream(&ts).await;
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("owner_held")),
        "{v}"
    );
    let (s, _) = send(owned(&ts, Method::DELETE, "/api/jobs/owner", &holder)).await;
    assert_eq!(s, 204);
    // A bound for a hang (the event never coming), not a time budget.
    let heard = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        waiting.next_named("jobs_owner"),
    )
    .await
    .expect("the release reaches the refused claimant's stream");
    assert_eq!(
        heard,
        json!({"topic": "jobs", "client": "app", "held": false})
    );
    let (s, v) = claim(&ts, &sid).await;
    assert_eq!(s, 200, "{v}");
}

/// The `utf8` file body of `html`.
fn utf8(html: &str) -> Value {
    json!({"content": html, "encoding": "utf8"})
}

#[tokio::test]
async fn a_publish_step_from_a_session_does_for_it_what_a_publish_does() {
    let ts = TestServer::spawn().await;
    let (_o, token) = owner(&ts).await;
    let sid = ts.register_session("claude", "h1").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // An app the owner made, and a thread on it sent to no session.
    let (s, v) = send(
        authed(&ts, Method::POST, "/api/artifacts")
            .json(&json!({"title": "Budget", "files": {"index.html": utf8("<p>one")}})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let aid = v["artifact"]["id"].as_str().unwrap().to_string();
    let tid = ts.thread_as_owner(&aid, "make it green").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let targets = |ts: &TestServer| -> Vec<Option<String>> {
        ts.store
            .feedback_rows(&tid)
            .unwrap()
            .into_iter()
            .map(|f| f.target_session_id)
            .collect()
    };
    assert_eq!(targets(&ts), [None]);
    // The session works on the thread and drafts the change.
    ts.put_working(
        &sid,
        &aid,
        json!({"message": "Recolouring", "thread_ids": [tid]}),
    )
    .await;
    let r = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/drafts/{aid}", ts.base)),
        )
        .json(&json!({"files": {"index.html": utf8("<p>green")}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert_eq!(ts.drafts.for_artifact(&aid).len(), 1);
    running(&ts, &token, "j1", "change", Some(&aid)).await;
    let mut b = publish_body(Some(&aid), 1, "<p>green", false);
    b["publish"]["session_id"] = json!(sid);
    let (s, v) = step(&ts, &token, "j1", "publish:1", b).await;
    assert_eq!(s, 200, "{v}");
    let aid_id = clax_core::ArtifactId::parse(&aid).unwrap();
    let version = ts.store.get_version(&aid_id, 2).unwrap().unwrap();
    assert_eq!(version.session_id.as_deref(), Some(sid.as_str()));
    assert_eq!(version.addresses, [tid.as_str()], "the thread worked on");
    assert!(
        ts.working.threads_of(&sid, &aid).is_empty(),
        "working cleared"
    );
    assert!(ts.drafts.for_artifact(&aid).is_empty(), "the draft ended");
    let watched = |ts: &TestServer, a: &str| {
        ts.store
            .list_watches(&sid)
            .unwrap()
            .iter()
            .any(|w| w.artifact_id == a)
    };
    assert!(watched(&ts, &aid), "the session watches the app");
    assert_eq!(targets(&ts), [Some(sid.clone())], "untargeted, retargeted");

    // A new app needs a title, and its session watches it.
    running(&ts, &token, "j2", "make", None).await;
    let mut b = publish_body(None, 0, "<p>new", true);
    b["publish"]["session_id"] = json!(sid);
    b["publish"]["request"]
        .as_object_mut()
        .unwrap()
        .remove("title");
    let (s, v) = step(&ts, &token, "j2", "publish:0", b.clone()).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (400, Some("invalid_args")),
        "{v}"
    );
    b["publish"]["request"]["title"] = json!("Fresh");
    let (s, v) = step(&ts, &token, "j2", "publish:0", b).await;
    assert_eq!(s, 200, "{v}");
    let made = v["step"]["result"]["artifact_id"].as_str().unwrap();
    assert!(watched(&ts, made));
}
