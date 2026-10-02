mod common;
use common::TestServer;
use serde_json::{Value, json};

async fn setup(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "cl1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts
        .publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>")
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn publish(ts: &TestServer, sid: &str, aid: &str, extra: Value) -> reqwest::Response {
    let mut body = json!({"if_version": 1, "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    ts.authed(
        ts.client
            .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
    )
    .header("x-clax-session", sid)
    .json(&body)
    .send()
    .await
    .unwrap()
}

#[tokio::test]
async fn a_publish_links_the_threads_the_session_was_working_on_then_clears() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=thread")).await;
    let res = publish(&ts, &sid, &aid, json!({"note": "Two columns"})).await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["note"], "Two columns");
    assert_eq!(v["version"]["addresses"], json!([tid]));
    assert_eq!(v["note_truncated"], false);
    assert_eq!(
        ev.next_named("thread").await["thread"]["addressed_in"],
        json!([2])
    );
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        t["thread"]["status"], "open",
        "linking leaves the thread open"
    );
    let w: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["working"], json!([]));
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["versions"][1]["addresses"], json!([tid]));
}

#[tokio::test]
async fn explicit_addresses_are_checked_before_anything_is_published() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    let res = publish(
        &ts,
        &sid,
        &aid,
        json!({"addresses": [clax_core::new_ulid()]}),
    )
    .await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_thread"
    );
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(a["artifact"]["current_version"], 1);
    let v: Value = publish(&ts, &sid, &aid, json!({"addresses": [tid]}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["version"]["addresses"], json!([tid]));
}

#[tokio::test]
async fn an_agent_resolve_links_to_the_current_version_once() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    ts.send_thread(&aid, &tid).await;
    let res = ts
        .authed(ts.client.post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/resolve",
            ts.base
        )))
        .header("x-clax-session", &sid)
        .json(&json!({"as": "agent"}))
        .send()
        .await
        .unwrap();
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["thread"]["addressed_in"], json!([1]));
}

#[tokio::test]
async fn seen_marks_are_per_viewer_monotonic_and_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, _) = setup(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let other = ts.viewer(None).await;
    let get = |cookie: String| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            ts.client
                .get(format!("{}/api/viewers/me/seen?artifact={aid}", ts.base))
                .header("cookie", format!("clax_viewer={cookie}"))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let put = |cookie: Option<String>, n: u32, origin: Option<&'static str>| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            let mut r = ts
                .client
                .put(format!("{}/api/viewers/me/seen", ts.base))
                .json(&json!({"artifact_id": aid, "version": n}));
            if let Some(c) = cookie {
                r = r.header("cookie", format!("clax_viewer={c}"));
            }
            if let Some(o) = origin {
                r = r.header("origin", o);
            }
            r.send().await.unwrap()
        }
    };
    assert_eq!(get(alex.cookie.clone()).await, json!({"seen": null}));
    assert_eq!(
        put(Some(alex.cookie.clone()), 3, None)
            .await
            .json::<Value>()
            .await
            .unwrap(),
        json!({"seen": 3})
    );
    assert_eq!(
        put(Some(alex.cookie.clone()), 2, None)
            .await
            .json::<Value>()
            .await
            .unwrap(),
        json!({"seen": 3})
    );
    assert_eq!(get(other.cookie.clone()).await, json!({"seen": null}));
    assert_eq!(put(None, 1, None).await.status(), 400);
    assert_eq!(
        put(Some(alex.cookie.clone()), 4, Some("http://evil.example"))
            .await
            .status(),
        403
    );
}
