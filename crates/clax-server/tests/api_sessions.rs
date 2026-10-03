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
        req = req.header("X-Clax-Session", s);
    }
    ts.authed(req).send().await.unwrap()
}

#[tokio::test]
async fn register_heartbeat_end_roundtrip() {
    let ts = TestServer::spawn().await;
    let s = register(
        &ts,
        json!({"harness": "claude", "harness_session_id": "h1", "cwd": "/w", "pid": 10, "parent_pid": 5}),
    )
    .await;
    let id = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["harness"], "claude");
    assert_eq!(s["ended_at"], Value::Null);
    // Registering the same harness session again returns the same row.
    let again = register(
        &ts,
        json!({"harness": "claude", "harness_session_id": "h1", "cwd": "/w", "pid": 11}),
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
        .get_authed("/api/sessions?live=true")
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
        .get_authed("/api/sessions?live=true")
        .await
        .json()
        .await
        .unwrap();
    assert!(live["sessions"].as_array().unwrap().is_empty());
    let all: Value = ts.get_authed("/api/sessions").await.json().await.unwrap();
    assert_eq!(all["sessions"].as_array().unwrap().len(), 1);
    let one: Value = ts
        .get_authed(&format!("/api/sessions/{id}"))
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
    assert_eq!(ts.get_authed("/api/sessions/nope").await.status(), 404);
}

#[tokio::test]
async fn join_by_parent_pid_meets_the_shim_in_either_order() {
    let ts = TestServer::spawn().await;
    let shim = register(
        &ts,
        json!({"harness": "claude", "cwd": "/w", "pid": 10, "parent_pid": 5}),
    )
    .await;
    let res = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "claude", "parent_pid": 5, "harness_session_id": "h1"}),
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
    assert_eq!(
        adopted["cwd"], "/w",
        "the shim's cwd fills the hook-only row"
    );

    let res = ts
        .post_json(
            "/api/sessions/join",
            json!({"harness": "pi", "parent_pid": 9, "harness_session_id": "p1", "cwd": "/hook"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap()["session"]["cwd"],
        "/hook"
    );
}

#[tokio::test]
async fn publish_with_session_header_attributes_owner_and_versions() {
    let ts = TestServer::spawn().await;
    let s = register(&ts, json!({"harness": "claude", "cwd": "/w"})).await;
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
        .header("X-Clax-Session", oid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2", "encoding": "utf8"}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let v2: Value = res.json().await.unwrap();
    assert_eq!(v2["version"]["session_id"], oid);
    assert_eq!(v2["artifact"]["owner_session_id"], sid);

    let got: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(got.get("owner_session").is_none(), "{got}");
    assert_eq!(got["artifact"]["owner_session_id"], sid);
    assert_eq!(got["artifact"]["owner_live"], true);
    assert_eq!(got["artifact"]["owner_harness"], "claude");
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["owner_live"], true);
    assert_eq!(list["artifacts"][0]["owner_harness"], "claude");

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
    assert_eq!(got["artifact"]["owner_live"], false);
    assert_eq!(got["artifact"]["owner_harness"], "claude");
}

#[tokio::test]
async fn publish_without_header_has_no_owner() {
    let ts = TestServer::spawn().await;
    let created = ts.publish("Plain", &[("index.html", "<p>")]).await;
    assert_eq!(created["artifact"]["owner_session_id"], Value::Null);
    let aid = created["artifact"]["id"].as_str().unwrap();
    let got: Value = ts
        .get_authed(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(got.get("owner_session").is_none(), "{got}");
    assert_eq!(got["artifact"]["owner_live"], false);
    assert_eq!(got["artifact"]["owner_harness"], Value::Null);
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
        .header("X-Clax-Session", sid)
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

#[tokio::test]
async fn non_utf8_session_header_is_400_unknown_session() {
    let ts = TestServer::spawn().await;
    let req = ts
        .authed(ts.client.post(format!("{}/api/artifacts", ts.base)))
        .header(
            "X-Clax-Session",
            reqwest::header::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
        )
        .json(
            &json!({"title": "T", "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}}),
        );
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
}

#[tokio::test]
async fn session_reads_need_the_token() {
    let ts = TestServer::spawn().await;
    let s = register(&ts, json!({"harness": "claude", "cwd": "/home/me/secret"})).await;
    let id = s["id"].as_str().unwrap();
    for path in [
        "/api/sessions".to_string(),
        "/api/sessions?live=true".to_string(),
        format!("/api/sessions/{id}"),
    ] {
        let res = ts.get(&path).await;
        assert_eq!(res.status(), 401, "{path}");
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["error"]["code"], "unauthorized", "{path}");
        let res = ts
            .client
            .get(format!("{}{path}", ts.base))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "{path}");
        assert_eq!(ts.get_authed(&path).await.status(), 200, "{path}");
    }
}

#[tokio::test]
async fn register_accepts_only_known_harnesses() {
    let ts = TestServer::spawn().await;
    for harness in ["claude-code", "", " ", "Claude"] {
        let res = ts
            .post_json("/api/sessions", json!({"harness": harness, "cwd": "/"}))
            .await;
        assert_eq!(res.status(), 400, "{harness:?}");
        let body: Value = res.json().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_args", "{harness:?}");
        assert_eq!(
            body["error"]["message"],
            "harness must be one of claude, codex, grok, pi"
        );
    }
    for harness in ["claude", "codex", "grok", "pi"] {
        register(&ts, json!({"harness": harness, "cwd": "/"})).await;
    }
}

#[tokio::test]
async fn a_grok_session_without_a_follower_has_no_push() {
    let ts = TestServer::spawn().await;
    let s = register(
        &ts,
        json!({"harness": "grok", "harness_session_id": "019a-grok", "cwd": "/w", "pid": 10, "parent_pid": 5}),
    )
    .await;
    assert_eq!(s["harness_session_id"], "019a-grok");
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{}", s["id"].as_str().unwrap()))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(sess["push"]["tier"], "monitor");
    assert_eq!(sess["push"]["available"], false);
    assert!(
        sess["push"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("no clax feedback follow is running"),
        "{sess}"
    );
}

#[tokio::test]
async fn an_empty_harness_session_id_is_no_id() {
    let ts = TestServer::spawn().await;
    let a = register(
        &ts,
        json!({"harness": "claude", "harness_session_id": "", "cwd": "/", "parent_pid": 1001}),
    )
    .await;
    let b = register(
        &ts,
        json!({"harness": "claude", "harness_session_id": "", "cwd": "/", "parent_pid": 1002}),
    )
    .await;
    assert_eq!(a["harness_session_id"], Value::Null);
    assert_eq!(b["harness_session_id"], Value::Null);
    assert_ne!(a["id"], b["id"], "two harness processes are two sessions");
}
