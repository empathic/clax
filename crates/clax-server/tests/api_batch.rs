mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn setup(ts: &TestServer) -> (String, String, Vec<String>) {
    let s = ts.register_session("claude", "batch-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts
        .publish_as(
            &sid,
            "Quarterly Review",
            "<main><h2>Quarterly goals</h2></main>",
        )
        .await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut tids = Vec::new();
    for body in ["one", "two", "three"] {
        tids.push(
            ts.thread(&aid, 1, body).await["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    (sid, aid, tids)
}

async fn send(ts: &TestServer, aid: &str, body: Value) -> reqwest::Response {
    ts.client
        .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .json(&body)
        .send()
        .await
        .unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap()
}

#[tokio::test]
async fn a_batch_is_delivered_as_one_group_led_by_its_note_through_every_tier() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait"] {
        let ts = TestServer::spawn().await;
        let (sid, aid, tids) = setup(&ts).await;
        let named = ts.viewer(Some("Alex")).await;
        let res = ts
            .client
            .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
            .header("cookie", format!("clax_viewer={}", named.cookie))
            .json(&json!({"thread_ids": tids, "note": "Before the demo"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{tier}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["sent"], json!(tids));
        assert_eq!(v["batch"]["sent_by"], "Alex");
        assert_eq!(v["threads"][0]["sends"][0]["note"], "Before the demo");
        let got: Value = ts
            .get_authed(&format!("/api/sessions/{sid}/feedback?tier={tier}"))
            .await
            .json()
            .await
            .unwrap();
        let text = got["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[clax] 3 comments sent to you:", "{tier}");
        assert_eq!(
            lines[1],
            "[clax] 3 comments on \"Quarterly Review\", sent together by Alex. Note: \"Before the demo\"",
            "{tier}"
        );
        for t in &tids {
            assert!(text.contains(t.as_str()), "{tier}: {t}");
        }
    }
}

#[tokio::test]
async fn a_bad_thread_fails_the_batch_and_nothing_is_sent() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    let other = ts.publish("Other", &[("index.html", "<p>")]).await;
    let oid = other["artifact"]["id"].as_str().unwrap().to_string();
    let foreign = ts.thread(&oid, 1, "elsewhere").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = send(&ts, &aid, json!({"thread_ids": [tids[0], foreign]})).await;
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(code(&v), "unknown_thread");
    assert!(v["error"]["message"].as_str().unwrap().contains(&foreign));
    ts.client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{}/resolve",
            ts.base, tids[1]
        ))
        .send()
        .await
        .unwrap();
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(code(&v), "thread_resolved");
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{}", tids[0]))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(t["thread"]["sent_to_agent"], false, "nothing was written");
    assert_eq!(t["thread"]["sends"], json!([]));
    let v: Value = send(&ts, &aid, json!({"thread_ids": []}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(code(&v), "invalid_args");
    let v: Value = send(
        &ts,
        &aid,
        json!({"thread_ids": [tids[0]], "note": "n".repeat(281)}),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(code(&v), "note_too_long");
    assert_eq!(
        send(&ts, "7q3k9mzx2b4t", json!({"thread_ids": [tids[0]]}))
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn already_sent_threads_are_reported_and_a_batch_of_nothing_is_refused() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    ts.send_thread(&aid, &tids[0]).await;
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (v["sent"].clone(), v["unchanged"].clone()),
        (json!([tids[1]]), json!([tids[0]]))
    );
    let r = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await;
    assert_eq!(r.status(), 409);
    assert_eq!(code(&r.json().await.unwrap()), "nothing_to_send");
}

#[tokio::test]
async fn the_batch_route_refuses_a_foreign_origin_like_the_single_send() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    let r = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .header("origin", "http://evil.example")
        .json(&json!({"thread_ids": [tids[0]]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(code(&r.json().await.unwrap()), "forbidden_origin");
}

#[tokio::test]
async fn a_delivered_batch_marks_every_thread_working_and_a_publish_links_them_all() {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await;
    let (sid, aid, tids) = setup(&ts).await;
    send(&ts, &aid, json!({"thread_ids": tids})).await;
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    let w: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(w["working"][0]["thread_ids"], json!(tids));
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["addresses"], json!(tids));
}

#[tokio::test]
async fn to_sends_only_to_that_agent_and_an_unknown_handle_writes_nothing() {
    let ts = TestServer::spawn().await;
    let (owner, aid, tids) = setup(&ts).await;
    let w = ts.register_session("codex", "batch-watch").await;
    let watcher = w["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{watcher}/watches/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    let handle = a["artifact"]["participants"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["harness"] == "codex")
        .unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    let bad = send(
        &ts,
        &aid,
        json!({"thread_ids": [tids[0]], "to": "a_00000000000000000000aa"}),
    )
    .await;
    assert_eq!(bad.status(), 400);
    assert_eq!(
        bad.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_agent"
    );
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{}", tids[0]))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(t["thread"]["sent_to_agent"], false, "nothing was written");
    assert_eq!(
        send(
            &ts,
            &aid,
            json!({"thread_ids": [tids[0], tids[1]], "to": handle})
        )
        .await
        .status(),
        200
    );
    let to_watcher: Value = ts
        .get_authed(&format!("/api/sessions/{watcher}/feedback?tier=piggyback"))
        .await
        .json()
        .await
        .unwrap();
    let to_owner: Value = ts
        .get_authed(&format!("/api/sessions/{owner}/feedback?tier=piggyback"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(to_watcher["feedback"].as_array().unwrap().len(), 2);
    assert_eq!(to_owner["feedback"].as_array().unwrap().len(), 0);
    let single = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{}/send",
            ts.base, tids[2]
        ))
        .json(&json!({"to": handle}))
        .send()
        .await
        .unwrap();
    assert_eq!(single.status(), 200);
    let again: Value = ts
        .get_authed(&format!("/api/sessions/{owner}/feedback?tier=piggyback"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        again["feedback"].as_array().unwrap().len(),
        0,
        "the single send with to skips the owner too"
    );
}

async fn watcher_of(ts: &TestServer, aid: &str, hsid: &str) -> (String, String) {
    let w = ts.register_session("codex", hsid).await;
    let sid = w["id"].as_str().unwrap().to_string();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base)),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let a: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    let handle = a["artifact"]["participants"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["harness"] == "codex")
        .unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    (sid, handle)
}

async fn taken(ts: &TestServer, sid: &str) -> Vec<String> {
    let v: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await
        .json()
        .await
        .unwrap();
    v["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["body"].as_str().unwrap().to_string())
        .collect()
}

async fn reply(ts: &TestServer, aid: &str, tid: &str, body: &str) {
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{tid}/comments",
            ts.base
        ))
        .json(&json!({"body": body}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn later_comments_follow_the_agent_the_thread_was_sent_to() {
    let ts = TestServer::spawn().await;
    let (owner, aid, tids) = setup(&ts).await;
    let (watcher, handle) = watcher_of(&ts, &aid, "follow-watch").await;
    assert_eq!(
        send(&ts, &aid, json!({"thread_ids": [tids[0]], "to": handle}))
            .await
            .status(),
        200
    );
    assert_eq!(taken(&ts, &watcher).await, ["one"]);
    reply(&ts, &aid, &tids[0], "and the footer").await;
    reply(&ts, &aid, &tids[0], "@agent also the header").await;
    assert_eq!(
        taken(&ts, &watcher).await,
        ["and the footer", "@agent also the header"],
        "later comments, @agent or not, follow the target"
    );
    assert!(
        taken(&ts, &owner).await.is_empty(),
        "the owner never gets a comment sent to another agent"
    );
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{}", tids[0]))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !t.to_string().contains(&watcher),
        "the target is never served"
    );
}

#[tokio::test]
async fn once_the_target_ends_later_comments_go_to_everyone_and_a_send_without_to_clears_it() {
    let ts = TestServer::spawn().await;
    let (owner, aid, tids) = setup(&ts).await;
    let (watcher, handle) = watcher_of(&ts, &aid, "ended-watch").await;
    send(
        &ts,
        &aid,
        json!({"thread_ids": [tids[0], tids[1]], "to": handle}),
    )
    .await;
    taken(&ts, &watcher).await;
    let res = ts
        .authed(
            ts.client
                .patch(format!("{}/api/sessions/{watcher}", ts.base)),
        )
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    reply(&ts, &aid, &tids[0], "still there?").await;
    assert_eq!(
        taken(&ts, &owner).await,
        ["still there?"],
        "the target ended: the comment fans out"
    );
    let (second, _) = watcher_of(&ts, &aid, "second-watch").await;
    let res = ts
        .client
        .post(format!(
            "{}/api/artifacts/{aid}/threads/{}/send",
            ts.base, tids[1]
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "a send without to");
    reply(&ts, &aid, &tids[1], "for everyone").await;
    assert_eq!(taken(&ts, &owner).await, ["for everyone"]);
    assert_eq!(
        taken(&ts, &second).await,
        ["for everyone"],
        "no target: the owner and every live watcher"
    );
}
