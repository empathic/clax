//! Agent questions: the session routes (spec 2026-10-06-agent-questions-and-inbox
//! §6.1), their waiters and the `question` event.

mod common;
use common::TestServer;
use serde_json::{Value, json};
use std::time::Duration;

/// The daemon's grace for a mirrored question no poll waits on.
const GRACE: Duration = Duration::from_secs(5);

fn body() -> Value {
    json!({"source": "ask", "questions": [{"question": "Which?", "header": "Pick",
        "options": [{"label": "A"}, {"label": "B"}]}]})
}

fn hook(tool_use_id: &str) -> Value {
    json!({"source": "hook", "tool_use_id": tool_use_id, "questions": [{"question": "Which?",
        "header": "Pick", "multiSelect": false, "options": [{"label": "A", "description": "a"},
        {"label": "B", "description": "b"}]}]})
}

async fn post(ts: &TestServer, path: &str, b: Value) -> reqwest::Response {
    ts.authed(ts.client.post(format!("{}{path}", ts.base)).json(&b))
        .send()
        .await
        .unwrap()
}

async fn session(ts: &TestServer, hsid: &str) -> String {
    ts.register_session("claude", hsid).await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn ask_then_answer_wakes_the_poll() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), body()).await;
    assert_eq!(res.status(), 201);
    let created: Value = res.json().await.unwrap();
    let qid = created["question"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["question"]["status"], "open");
    assert_eq!(created["mode"], "wait");
    assert_eq!(created["terminal_after_s"], 600);
    assert_eq!(created["question"]["agent"]["harness"], "claude");
    assert_eq!(created["question"]["agent"]["project"], "w");
    assert!(
        !created.to_string().contains(&sid),
        "no view carries the session ID"
    );
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/questions/{qid}?wait=60",
        ts.base
    )));
    let waiter =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    ts.wait_question_waiters(&qid, 1).await;
    ts.answer_question(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    let got = waiter.await.unwrap();
    assert_eq!(got["question"]["status"], "answered");
    assert_eq!(got["question"]["answers"][0]["selected"][0], "A");
    assert_eq!(ts.question_waiters(&qid).await, 0, "the poll let go");
    let row = ts.store.question(&qid).unwrap().unwrap();
    assert!(row.taken_at.is_some(), "an answered result marks it taken");
}

#[tokio::test]
async fn a_poll_with_no_wait_answers_at_once_with_the_open_question() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, body()).await;
    let qid = q["question"]["id"].as_str().unwrap();
    let res = ts
        .get_authed(&format!("/api/sessions/{sid}/questions/{qid}?wait=0"))
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (v["question"]["status"].as_str(), v["waited_s"].as_u64()),
        (Some("open"), Some(0))
    );
    assert!(ts.store.question(qid).unwrap().unwrap().taken_at.is_none());
}

#[tokio::test]
async fn another_sessions_question_is_not_found() {
    let ts = TestServer::spawn().await;
    let s1 = session(&ts, "h1").await;
    let s2 = ts.register_session("codex", "h2").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let q = ts.ask(&s1, body()).await;
    let qid = q["question"]["id"].as_str().unwrap();
    let res = ts
        .get_authed(&format!("/api/sessions/{s2}/questions/{qid}"))
        .await;
    assert_eq!(res.status(), 404);
    for action in ["withdraw", "release"] {
        let res = post(
            &ts,
            &format!("/api/sessions/{s2}/questions/{qid}/{action}"),
            json!({}),
        )
        .await;
        assert_eq!(res.status(), 404, "{action}");
    }
    assert_eq!(ts.question_status(qid).await, "open");
}

#[tokio::test]
async fn unknown_and_ended_sessions_and_missing_artifacts() {
    let ts = TestServer::spawn().await;
    let res = post(&ts, "/api/sessions/nope/questions", body()).await;
    assert_eq!(res.status(), 404);
    let sid = session(&ts, "h1").await;
    let mut b = body();
    b["artifact_id"] = json!("7q3k9mzx2b4t");
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(res.status(), 404, "artifact_id names a live artifact");
    let a = ts.publish("Report", &[("index.html", "<p>hi")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let mut b = body();
    b["artifact_id"] = json!(aid);
    let q = ts.ask(&sid, b).await;
    assert_eq!(q["question"]["artifact"]["id"], aid);
    assert_eq!(q["question"]["artifact"]["title"], "Report");
    ts.end_session(&sid).await;
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), body()).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "unknown_session"
    );
}

#[tokio::test]
async fn invalid_questions_and_limits() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let mut b = body();
    b["questions"][0]["header"] = json!("Thirteen char");
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(res.status(), 400);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_question"
    );
    let mut b = body();
    b["source"] = json!("other");
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(res.status(), 400);
    let mut b = body();
    b["questions"][0]["extra"] = json!(1);
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_question"
    );
    let mut b = hook("t0");
    b["questions"] = json!("nope");
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_question"
    );
    let big = "x".repeat(130 * 1024);
    let mut b = body();
    b["questions"][0]["options"][0]["preview"] = json!(big);
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(res.status(), 413, "the request is at most 128 KiB");
    for _ in 0..8 {
        assert_eq!(
            post(&ts, &format!("/api/sessions/{sid}/questions"), body())
                .await
                .status(),
            201
        );
    }
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), body()).await;
    assert_eq!(res.status(), 429);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "limit_reached"
    );
    let lan = ts.lan();
    let res = lan
        .0
        .post(format!("{}/api/sessions/{sid}/questions", lan.1))
        .json(&body())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401, "session routes need the token");
}

#[tokio::test]
async fn the_same_tool_use_returns_the_first_question() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions"),
        hook("toolu_1"),
    )
    .await;
    assert_eq!(res.status(), 201);
    let first: Value = res.json().await.unwrap();
    assert_eq!(first["question"]["source"], "hook");
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions"),
        hook("toolu_1"),
    )
    .await;
    assert_eq!(res.status(), 200);
    let again: Value = res.json().await.unwrap();
    assert_eq!(again["question"]["id"], first["question"]["id"]);
}

#[tokio::test]
async fn a_hook_question_takes_the_artifact_of_the_newest_working_record() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let a = ts.publish_as(&sid, "Board", "<p>b").await;
    let aid = a["artifact"]["id"].as_str().unwrap();
    let res = ts
        .authed(
            ts.client
                .put(format!("{}/api/sessions/{sid}/working/{aid}", ts.base))
                .json(&json!({"message": "drawing"})),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let q = ts.ask(&sid, hook("t1")).await;
    assert_eq!(q["question"]["artifact"]["id"], aid);
    let q = ts.ask(&sid, body()).await;
    assert!(q["question"]["artifact"].is_null(), "only hook questions");
}

#[tokio::test]
async fn withdraw_closes_an_open_question_once() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, body()).await;
    let qid = q["question"]["id"].as_str().unwrap();
    let mut tap = ts.question_events();
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions/{qid}/withdraw"),
        json!({}),
    )
    .await;
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.json::<Value>().await.unwrap()["question"]["status"],
        "withdrawn"
    );
    assert_eq!(tap.next().await["status"], "withdrawn");
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions/{qid}/withdraw"),
        json!({}),
    )
    .await;
    assert_eq!(res.status(), 409);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "question_closed");
    assert_eq!(v["question"]["status"], "withdrawn");
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions/{qid}/release"),
        json!({}),
    )
    .await;
    assert_eq!(
        res.status(),
        400,
        "an ask question never moves to the terminal"
    );
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "not_mirrored"
    );
}

#[tokio::test]
async fn a_release_then_an_answer_is_question_closed_with_the_state() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, hook("t")).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        post(
            &ts,
            &format!("/api/sessions/{sid}/questions/{qid}/release"),
            json!({})
        )
        .await
        .status(),
        200
    );
    let res = ts
        .answer_question_raw(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    assert_eq!(res.status(), 409);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "question_closed");
    assert_eq!(v["question"]["status"], "released");
    // The terminal's answer is recorded on the released question.
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions:terminal"),
        json!({"tool_use_id": "t", "answers": {"Which?": "B"}}),
    )
    .await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        (
            v["question"]["status"].as_str(),
            v["question"]["answered_via"].as_str()
        ),
        (Some("answered"), Some("terminal"))
    );
    assert_eq!(v["question"]["answers"][0]["selected"][0], "B");
    // Nothing is released for that call any more.
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions:terminal"),
        json!({"tool_use_id": "t", "answers": {"Which?": "A"}}),
    )
    .await;
    assert_eq!(res.status(), 204);
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions:terminal"),
        json!({"tool_use_id": "unknown", "answers": {}}),
    )
    .await;
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn ending_the_session_withdraws_its_questions() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, body()).await;
    let qid = q["question"]["id"].as_str().unwrap();
    let mut tap = ts.question_events();
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/questions/{qid}?wait=60",
        ts.base
    )));
    let waiter =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    ts.wait_question_waiters(qid, 1).await;
    ts.end_session(&sid).await;
    assert_eq!(ts.question_status(qid).await, "withdrawn");
    let ev = tap.next().await;
    assert_eq!(
        (ev["id"].as_str(), ev["status"].as_str()),
        (Some(qid), Some("withdrawn"))
    );
    assert_eq!(waiter.await.unwrap()["question"]["status"], "withdrawn");
}

#[tokio::test]
async fn questions_never_reach_the_public_event_stream() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let mut events = ts.events("").await;
    ts.ask(&sid, body()).await;
    let a = ts.publish("After", &[("index.html", "<p>x")]).await;
    let (name, data) = events.next().await;
    assert_eq!(name, "version", "the question event was not sent: {data}");
    assert_eq!(data["artifact_id"], a["artifact"]["id"]);
}

#[tokio::test]
async fn a_question_takes_at_most_four_polls_at_once() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, body()).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    let mut polls = Vec::new();
    for _ in 0..4 {
        let req = ts.authed(ts.client.get(format!(
            "{}/api/sessions/{sid}/questions/{qid}?wait=60",
            ts.base
        )));
        polls.push(tokio::spawn(
            async move { req.send().await.unwrap().status() },
        ));
    }
    ts.wait_question_waiters(&qid, 4).await;
    let res = ts
        .get_authed(&format!("/api/sessions/{sid}/questions/{qid}?wait=60"))
        .await;
    assert_eq!(res.status(), 429);
    assert_eq!(
        res.json::<Value>().await.unwrap()["error"]["code"],
        "limit_reached"
    );
    ts.answer_question(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    for p in polls {
        assert_eq!(p.await.unwrap(), 200);
    }
}

#[tokio::test]
async fn an_ended_sessions_routes_are_unknown_session() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, hook("t")).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        post(
            &ts,
            &format!("/api/sessions/{sid}/questions/{qid}/release"),
            json!({})
        )
        .await
        .status(),
        200
    );
    ts.end_session(&sid).await;
    assert_eq!(
        ts.question_status(&qid).await,
        "released",
        "only open ones are withdrawn"
    );
    let code = |res: reqwest::Response| async move {
        (
            res.status().as_u16(),
            res.json::<Value>().await.unwrap()["error"]["code"].clone(),
        )
    };
    let want = (400, json!("unknown_session"));
    assert_eq!(
        code(
            ts.get_authed(&format!("/api/sessions/{sid}/questions/{qid}"))
                .await
        )
        .await,
        want
    );
    for action in ["withdraw", "release"] {
        let res = post(
            &ts,
            &format!("/api/sessions/{sid}/questions/{qid}/{action}"),
            json!({}),
        )
        .await;
        assert_eq!(code(res).await, want, "{action}");
    }
    let res = post(
        &ts,
        &format!("/api/sessions/{sid}/questions:terminal"),
        json!({"tool_use_id": "t", "answers": {"Which?": "B"}}),
    )
    .await;
    assert_eq!(code(res).await, want, "terminal");
    assert_eq!(ts.question_status(&qid).await, "released");
    let res = ts
        .get_authed(&format!("/api/sessions/nope/questions/{qid}"))
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn a_hook_question_never_polled_is_withdrawn_after_the_grace() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let sid = session(&ts, "h1").await;
    let mut tap = ts.question_events();
    let q = ts.ask(&sid, hook("toolu_1")).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    assert_eq!(tap.next().await["status"], "open");
    clock.wait_for(GRACE, 1).await;
    assert_eq!(ts.question_status(&qid).await, "open");
    assert_eq!(clock.fire(GRACE), 1);
    let ev = tap.next().await;
    assert_eq!(
        (ev["id"].as_str(), ev["status"].as_str()),
        (Some(&*qid), Some("withdrawn"))
    );
}

#[tokio::test]
async fn a_hook_question_whose_poll_gave_up_is_withdrawn_after_the_grace() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let sid = session(&ts, "h1").await;
    let mut tap = ts.question_events();
    let q = ts.ask(&sid, hook("toolu_1")).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    tap.next().await;
    // A poll that gives up at once (wait=0) holds it and lets go: the
    // creation's grace is cancelled and a new one started.
    let v: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/questions/{qid}?wait=0"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["question"]["status"], "open");
    clock.wait_for(GRACE, 2).await;
    assert_eq!(clock.fire(GRACE), 2);
    let ev = tap.next().await;
    assert_eq!(
        (ev["id"].as_str(), ev["status"].as_str()),
        (Some(&*qid), Some("withdrawn"))
    );
}

#[tokio::test]
async fn the_grace_starts_when_the_last_poll_lets_go() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let sid = session(&ts, "h1").await;
    let mut tap = ts.question_events();
    let q = ts.ask(&sid, hook("toolu_1")).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    assert_eq!(tap.next().await["status"], "open");
    clock.wait_for(GRACE, 1).await;
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/questions/{qid}?wait=1",
        ts.base
    )));
    let poll =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    ts.wait_question_waiters(&qid, 1).await;
    clock.wait_for(Duration::from_secs(1), 1).await;
    // The creation's grace ends while the poll holds the question: nothing.
    assert_eq!(clock.fire(GRACE), 1);
    // The poll's wait ends; it lets go and a new grace starts.
    assert_eq!(clock.fire(Duration::from_secs(1)), 1);
    assert_eq!(
        poll.await.unwrap()["question"]["status"],
        "open",
        "held past the creation grace"
    );
    clock.wait_for(GRACE, 1).await;
    assert_eq!(ts.question_status(&qid).await, "open");
    assert_eq!(clock.fire(GRACE), 1);
    let ev = tap.next().await;
    assert_eq!(
        (ev["id"].as_str(), ev["status"].as_str()),
        (Some(&*qid), Some("withdrawn"))
    );
}

#[tokio::test]
async fn an_ask_question_has_no_grace() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, body()).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    let v: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/questions/{qid}?wait=0"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["question"]["status"], "open");
    assert_eq!(clock.waiting(GRACE), 0);
}

#[tokio::test]
async fn a_closed_hook_question_gets_no_grace_from_a_poll() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, hook("t")).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    clock.wait_for(GRACE, 1).await;
    let res = ts
        .post_json(
            &format!("/api/sessions/{sid}/questions/{qid}/release"),
            json!({}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let v: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/questions/{qid}?wait=0"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(v["question"]["status"], "released");
    assert_eq!(clock.waiting(GRACE), 1, "only the creation's grace");
}

#[tokio::test]
async fn a_poll_answers_open_when_its_wait_ends() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let sid = session(&ts, "h1").await;
    let q = ts.ask(&sid, body()).await;
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/questions/{qid}?wait=30",
        ts.base
    )));
    let poll =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    clock.wait_for(Duration::from_secs(30), 1).await;
    assert_eq!(clock.fire(Duration::from_secs(30)), 1);
    assert_eq!(poll.await.unwrap()["question"]["status"], "open");
    assert_eq!(ts.question_waiters(&qid).await, 0);
}
