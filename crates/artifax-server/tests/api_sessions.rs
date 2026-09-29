mod common;
use common::TestServer;
use serde_json::{Value, json};

async fn register(ts: &TestServer, body: Value) -> Value {
    let res = ts.post_json("/api/sessions", body).await;
    assert_eq!(res.status(), 201, "{}", res.text().await.unwrap());
    res.json::<Value>().await.unwrap()["session"].clone()
}

async fn publish_as(ts: &TestServer, session: Option<&str>) -> reqwest::Response {
    let mut req = ts
        .client
        .post(format!("{}/api/artifacts", ts.base))
        .json(&json!({
            "title": "T",
            "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        }));
    if let Some(s) = session {
        req = req.header("X-Artifax-Session", s);
    }
    ts.authed(req).send().await.unwrap()
}

#[tokio::test]
async fn register_heartbeat_end_roundtrip() {
    let ts = TestServer::spawn().await;
    let s = register(
        &ts,
        json!({"harness": "claude-code", "harness_session_id": "h1", "cwd": "/w", "pid": 10, "parent_pid": 5}),
    )
    .await;
    let id = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["harness"], "claude-code");
    assert_eq!(s["ended_at"], Value::Null);
    // Registering the same harness session again returns the same row.
    let again = register(
        &ts,
        json!({"harness": "claude-code", "harness_session_id": "h1", "cwd": "/w", "pid": 11}),
    )
    .await;
    assert_eq!(again["id"], id);
    assert_eq!(again["pid"], 11);

    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{id}", ts.base)))
        .json(&json!({"heartbeat": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let beat: Value = res.json().await.unwrap();
    assert!(beat["session"]["last_seen_at"].as_str() >= s["last_seen_at"].as_str());

    let live: Value = ts
        .get("/api/sessions?live=true")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(live["sessions"].as_array().unwrap().len(), 1);

    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{id}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert!(res.json::<Value>().await.unwrap()["session"]["ended_at"].is_string());
    let live: Value = ts
        .get("/api/sessions?live=true")
        .await
        .json()
        .await
        .unwrap();
    assert!(live["sessions"].as_array().unwrap().is_empty());
    let all: Value = ts.get("/api/sessions").await.json().await.unwrap();
    assert_eq!(all["sessions"].as_array().unwrap().len(), 1);
    let one: Value = ts
        .get(&format!("/api/sessions/{id}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(one["session"]["id"], id);
}

#[tokio::test]
async fn write_routes_need_the_token_and_bad_input_is_json_400() {
    let ts = TestServer::spawn().await;
    let res = ts
        .client
        .post(format!("{}/api/sessions", ts.base))
        .json(&json!({"harness": "codex", "cwd": "/"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = ts.post_json("/api/sessions", json!({"cwd": "/"})).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_json"
    );
    let s = register(&ts, json!({"harness": "codex", "cwd": "/"})).await;
    let res = ts
        .authed(ts.client.patch(format!(
            "{}/api/sessions/{}",
            ts.base,
            s["id"].as_str().unwrap()
        )))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(ts.get("/api/sessions/nope").await.status(), 404);
}

#[tokio::test]
async fn join_by_parent_pid_meets_the_shim_in_either_order() {
    let ts = TestServer::spawn().await;
    let shim = register(
        &ts,
        json!({"harness": "claude-code", "cwd": "/w", "pid": 10, "parent_pid": 5}),
    )
    .await;
    let res = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "claude-code", "parent_pid": 5, "harness_session_id": "h1"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let joined = res.json::<Value>().await.unwrap()["session"].clone();
    assert_eq!(joined["id"], shim["id"]);
    assert_eq!(joined["harness_session_id"], "h1");

    let hook = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "codex", "parent_pid": 6, "harness_session_id": "c1"}),
        )
        .await
        .json::<Value>()
        .await
        .unwrap()["session"]
        .clone();
    let adopted = register(
        &ts,
        json!({"harness": "codex", "cwd": "/w", "pid": 20, "parent_pid": 6}),
    )
    .await;
    assert_eq!(adopted["id"], hook["id"]);
    assert_eq!(adopted["pid"], 20);
}

#[tokio::test]
async fn publish_with_session_header_attributes_owner_and_versions() {
    let ts = TestServer::spawn().await;
    let s = register(&ts, json!({"harness": "claude-code", "cwd": "/w"})).await;
    let sid = s["id"].as_str().unwrap();
    let res = publish_as(&ts, Some(sid)).await;
    assert_eq!(res.status(), 201);
    let created: Value = res.json().await.unwrap();
    let aid = created["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["artifact"]["owner_session_id"], sid);
    assert_eq!(created["version"]["session_id"], sid);

    // A second session publishing v2 is recorded on the version, not as owner.
    let other = register(&ts, json!({"harness": "codex", "cwd": "/w"})).await;
    let oid = other["id"].as_str().unwrap();
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("X-Artifax-Session", oid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let v2: Value = res.json().await.unwrap();
    assert_eq!(v2["version"]["session_id"], oid);
    assert_eq!(v2["artifact"]["owner_session_id"], sid);

    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["owner_session"]["id"], sid);
    assert_eq!(got["owner_session"]["harness"], "claude-code");
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["owner_live"], true);
    assert_eq!(list["artifacts"][0]["owner_harness"], "claude-code");

    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["owner_live"], false);
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(got["owner_session"]["ended_at"].is_string());
}

#[tokio::test]
async fn publish_without_header_has_no_owner() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("Plain", &[("index.html", "<p>")]).await;
    assert_eq!(created["artifact"]["owner_session_id"], Value::Null);
    let aid = created["artifact"]["id"].as_str().unwrap();
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["owner_session"], Value::Null);
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["owner_live"], false);
}

#[tokio::test]
async fn unknown_or_ended_session_header_is_400_on_both_publish_routes() {
    let ts = TestServer::spawn().await;
    let res = publish_as(&ts, Some("nosuchsession")).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
    assert!(
        ts.get("/api/artifacts")
            .await
            .json::<Value>()
            .await
            .unwrap()["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let s = register(&ts, json!({"harness": "codex", "cwd": "/"})).await;
    let sid = s["id"].as_str().unwrap();
    let created = ts.publish("A", &[("index.html", "<p>")]).await;
    let aid = created["artifact"]["id"].as_str().unwrap();
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    let res = ts
        .authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("X-Artifax-Session", sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["versions"].as_array().unwrap().len(), 1);
}
