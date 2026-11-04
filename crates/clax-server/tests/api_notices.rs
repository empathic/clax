use clax_realtime::feedback::FollowKind;
use clax_testkit::{TestServer, until, within};
use serde_json::{Value, json};

/// Registers a Grok session and returns its ID.
async fn grok_session(ts: &TestServer) -> String {
    let s = ts.register_session("grok", "g1").await;
    s["id"].as_str().unwrap().to_string()
}

/// Registers a Claude Code session and returns its ID.
async fn claude_session(ts: &TestServer) -> String {
    let s = ts.register_session("claude", "c1").await;
    s["id"].as_str().unwrap().to_string()
}

/// Publishes as `sid` (which watches with replies armed); returns the artifact ID.
async fn published(ts: &TestServer, sid: &str) -> String {
    let a = ts.publish_as(sid, "T", "<h2>x</h2>").await;
    a["artifact"]["id"].as_str().unwrap().to_string()
}

/// Opens a thread on `aid` and sends it to the agent; returns the thread ID.
async fn send_comment(ts: &TestServer, aid: &str) -> String {
    let t = ts.thread(aid, 1, "hello").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.send_thread(aid, &tid).await;
    tid
}

/// Publishes as `sid`, opens a thread and sends it to the agent; returns
/// (artifact ID, thread ID).
async fn sent_comment(ts: &TestServer, sid: &str) -> (String, String) {
    let aid = published(ts, sid).await;
    let tid = send_comment(ts, &aid).await;
    (aid, tid)
}

/// Returns once a notices poll of `sid` counts as its follower (or, for
/// `FollowKind::Channel`, its channel): it is in progress.
async fn following(ts: &TestServer, sid: &str, kind: FollowKind) {
    until(&format!("a notices poll of {sid} in progress"), || async {
        ts.state.followers.has(sid, kind).then_some(())
    })
    .await;
}

async fn notices(ts: &TestServer, sid: &str, wait: u64) -> Value {
    let res = ts
        .get_authed(&format!("/api/sessions/{sid}/notices?wait={wait}"))
        .await;
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

#[tokio::test]
async fn a_notice_points_at_the_comment_once_and_delivers_nothing() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let (aid, tid) = sent_comment(&ts, &sid).await;
    let v = notices(&ts, &sid, 0).await;
    assert_eq!(v["notices"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["notices"][0]["thread_id"], tid);
    assert_eq!(v["notices"][0]["artifact_id"], aid);
    let line = v["lines"][0].as_str().unwrap();
    assert!(
        line.starts_with("[clax] New comment on ") && line.contains(&aid) && !line.contains('\n'),
        "{line}"
    );
    assert!(
        !line.contains("hello"),
        "the line never carries the comment"
    );
    let again = notices(&ts, &sid, 0).await;
    assert!(again["notices"].as_array().unwrap().is_empty());
    // Still undelivered: the Stop hook's tier hands it over.
    let f: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(f["feedback"].as_array().unwrap().len(), 1, "{f}");
}

/// The poll waits 600 s, past the HANG guard: an answer at all is the new
/// comment ending it.
#[tokio::test]
async fn a_waiting_notices_poll_wakes_on_a_new_comment() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let aid = published(&ts, &sid).await;
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=600", ts.base)),
    );
    let waiter = tokio::spawn(async move {
        let body: Value = req.send().await.unwrap().json().await.unwrap();
        body
    });
    following(&ts, &sid, FollowKind::Follower).await;
    let tid = send_comment(&ts, &aid).await;
    let body = within("the woken notices poll", waiter).await.unwrap();
    assert_eq!(body["notices"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["notices"][0]["thread_id"], tid);
}

#[tokio::test]
async fn notices_stay_quiet_while_the_session_waits_for_feedback() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let aid = published(&ts, &sid).await;
    let wait = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/feedback?tier=wait&wait=600",
        ts.base
    )));
    let waiter = tokio::spawn(async move {
        let body: Value = wait.send().await.unwrap().json().await.unwrap();
        body
    });
    ts.wait_feedback_waiters(&sid, 1).await;
    let quiet = notices(&ts, &sid, 0).await;
    assert_eq!(quiet, json!({"notices": [], "lines": [], "waited_s": 0}));
    let tid = send_comment(&ts, &aid).await;
    let got = within("the wait poll's answer", waiter).await.unwrap();
    assert_eq!(got["feedback"][0]["thread_id"], tid, "{got}");
    // Delivered by the wait: never announced afterwards.
    let after = notices(&ts, &sid, 0).await;
    assert!(after["notices"].as_array().unwrap().is_empty(), "{after}");
}

#[tokio::test]
async fn grok_push_reports_whether_a_follower_is_connected() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let push = |v: Value| v["push"].clone();
    let before: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(push(before.clone())["tier"], "monitor");
    assert_eq!(push(before.clone())["available"], false);
    assert!(
        push(before)["reason"]
            .as_str()
            .unwrap()
            .starts_with("no clax feedback follow is running")
    );
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=600", ts.base)),
    );
    let follower = tokio::spawn(async move { req.send().await.unwrap().status() });
    following(&ts, &sid, FollowKind::Follower).await;
    let during: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(push(during.clone())["tier"], "monitor");
    assert_eq!(push(during.clone())["available"], true, "{during}");
    assert_eq!(push(during)["reason"], Value::Null);
    follower.abort();
}

#[tokio::test]
async fn notices_of_an_unknown_or_ended_session_are_errors() {
    let ts = TestServer::spawn().await;
    let missing = ts.get_authed("/api/sessions/nope/notices?wait=0").await;
    assert_eq!(missing.status(), 404);
    let sid = grok_session(&ts).await;
    let ended = ts
        .authed(
            ts.client
                .patch(format!("{}/api/sessions/{sid}", ts.base))
                .json(&json!({"ended": true})),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), 200);
    // Answered before any wait: a 600 s wait outlasts the HANG guard.
    let res = within(
        "the ended session's notices poll",
        ts.get_authed(&format!("/api/sessions/{sid}/notices?wait=600")),
    )
    .await;
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unknown_session", "{body}");
    let unauthed = ts.get(&format!("/api/sessions/{sid}/notices")).await;
    assert_eq!(unauthed.status(), 401);
}

/// The follower's poll waits 600 s and is ended by the daemon's shutdown.
#[tokio::test]
async fn claude_push_reports_a_notice_follower() {
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let ts = TestServer::spawn_with(move |s| s.shutdown = stopping).await;
    let sid = claude_session(&ts).await;
    let push = |ts: &TestServer, sid: &str| {
        let path = format!("/api/sessions/{sid}");
        let req = ts.authed(ts.client.get(format!("{}{path}", ts.base)));
        async move { req.send().await.unwrap().json::<Value>().await.unwrap()["push"].clone() }
    };
    let idle = push(&ts, &sid).await;
    assert_eq!(idle["tier"], Value::Null);
    assert_eq!(idle["available"], false);
    assert!(
        idle["reason"]
            .as_str()
            .unwrap()
            .contains("--dangerously-load-development-channels plugin:clax@clax"),
        "{idle}"
    );
    // The reason says push is optional before it says how to turn it on.
    let reason = idle["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("optional: ")
            && reason.contains("push only wakes the session while it is idle")
            && reason.contains("follow_command"),
        "{reason}"
    );
    // A notices poll in progress counts as a follower.
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=600", ts.base)),
    );
    let poll = tokio::spawn(async move { req.send().await.unwrap().status() });
    following(&ts, &sid, FollowKind::Follower).await;
    let now = push(&ts, &sid).await;
    assert_eq!(
        now,
        json!({"tier": "notice", "available": true, "reason": null})
    );
    stop.send(true).unwrap();
    assert_eq!(within("the follower's poll", poll).await.unwrap(), 200);
}

/// The page's agent list says whether each live agent is woken by a
/// comment while idle: a Claude Code session is once a notices poll (a
/// `clax feedback follow`, or the shim's channel) is connected.
#[tokio::test]
async fn a_follower_poll_marks_its_agent_woken_on_the_page() {
    let ts = TestServer::spawn().await;
    let sid = claude_session(&ts).await;
    let aid = published(&ts, &sid).await;
    let wakes = |ts: &TestServer, aid: &str| {
        let req = ts.authed(ts.client.get(format!("{}/api/artifacts/{aid}", ts.base)));
        async move {
            let v: Value = req.send().await.unwrap().json().await.unwrap();
            v["artifact"]["participants"]["agents"][0]["wakes"].clone()
        }
    };
    assert_eq!(wakes(&ts, &aid).await, false);
    // Every listing the shell reads says the same: the gallery's, the
    // agents roster, and the page's first load.
    let listed: Value = ts.get_authed("/api/artifacts").await.json().await.unwrap();
    let card = listed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == aid.as_str())
        .unwrap()
        .clone();
    assert_eq!(card["participants"]["agents"][0]["wakes"], false, "{card}");
    let one: Value = ts
        .get_authed(&format!("/api/artifacts?artifact={aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        one["artifacts"][0]["participants"]["agents"][0]["wakes"], false,
        "{one}"
    );
    let session: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    let handle = session["session"]["agent_handle"].clone();
    let roster: Value = ts.get_authed("/api/agents").await.json().await.unwrap();
    let me = roster["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["handle"] == handle)
        .unwrap()
        .clone();
    assert_eq!(me["wakes"], false, "{me}");
    let page = ts
        .get_authed(&format!("/a/{aid}"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("\"wakes\":false"),
        "the owner's first load names it"
    );
    // A browser that is neither the owner nor the agent's person is not told.
    let anon = ts.get(&format!("/a/{aid}")).await.text().await.unwrap();
    assert!(
        !anon.contains("\"wakes\""),
        "a LAN viewer's first load names none"
    );
    let lan: Value = ts
        .get(&format!("/api/artifacts/{aid}"))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        lan["artifact"]["participants"]["agents"][0]
            .get("wakes")
            .is_none(),
        "{lan}"
    );
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=600", ts.base)),
    );
    let poll = tokio::spawn(async move { req.send().await.unwrap().status() });
    until("the follower counted", || async {
        (wakes(&ts, &aid).await == true).then_some(())
    })
    .await;
    poll.abort();
    // A Pi session is woken only while its extension's injection loop
    // polls: a client merely named pi is not.
    let pi = ts.register_session("pi", "p1").await;
    let pid = pi["id"].as_str().unwrap().to_string();
    let paid = published(&ts, &pid).await;
    assert_eq!(wakes(&ts, &paid).await, false);
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{pid}/feedback?tier=inject&wait=600",
        ts.base
    )));
    let inject = tokio::spawn(async move { req.send().await.unwrap().status() });
    until("the injection loop counted", || async {
        (wakes(&ts, &paid).await == true).then_some(())
    })
    .await;
    inject.abort();
}

/// The shim's channel poll leaves Claude Code unsure, and takes no notice
/// while a follower of the session is connected, so a follower started
/// because Claude Code refused the channel gets every notice.
#[tokio::test]
async fn a_channel_is_unsure_and_yields_its_notices_to_a_follower() {
    let ts = TestServer::spawn().await;
    let sid = claude_session(&ts).await;
    let aid = published(&ts, &sid).await;
    let wakes = |ts: &TestServer, aid: &str| {
        let req = ts.authed(ts.client.get(format!("{}/api/artifacts/{aid}", ts.base)));
        async move {
            let v: Value = req.send().await.unwrap().json().await.unwrap();
            v["artifact"]["participants"]["agents"][0]["wakes"].clone()
        }
    };
    let channel = |ts: &TestServer, sid: &str, wait: u64| {
        let req = ts.authed(ts.client.get(format!(
            "{}/api/sessions/{sid}/notices?wait={wait}&via=channel",
            ts.base
        )));
        async move { req.send().await.unwrap().json::<Value>().await.unwrap() }
    };
    let poll = tokio::spawn(channel(&ts, &sid, 600));
    until("the channel counted", || async {
        (wakes(&ts, &aid).await == "unsure").then_some(())
    })
    .await;
    let push: Value = ts
        .get_authed(&format!("/api/sessions/{sid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            &push["push"]["tier"],
            &push["push"]["available"],
            &push["push"]["sure"]
        ),
        (&json!("channel"), &json!(true), &json!(false)),
        "{push}"
    );
    poll.abort();
    // A follower connects: the channel takes nothing, the follower all.
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/notices?wait=600", ts.base)),
    );
    let follower =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    until("the follower counted", || async {
        (wakes(&ts, &aid).await == true).then_some(())
    })
    .await;
    send_comment(&ts, &aid).await;
    let quiet = channel(&ts, &sid, 0).await;
    assert!(quiet["notices"].as_array().unwrap().is_empty(), "{quiet}");
    let got = within("the follower's notice", follower).await.unwrap();
    assert_eq!(got["notices"].as_array().unwrap().len(), 1, "{got}");
}
