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

/// The `question` events of an owner's stream holding `questions` (which
/// also keeps an owner surface open).
struct Tap(clax_server::testing::EventReader);

impl Tap {
    /// The next `question` event's view.
    async fn next(&mut self) -> Value {
        self.0.next_named("question").await["question"].clone()
    }
}

async fn owner_tap(ts: &TestServer) -> Tap {
    Tap(ts.stream_as_owner(&["questions"]).await)
}

/// The late answers a `stop_hook` feedback poll of session `sid` takes now.
async fn late(ts: &TestServer, sid: &str) -> Value {
    let v: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook"))
        .await
        .json()
        .await
        .unwrap();
    v["answers"].clone()
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
    let _surface = ts.stream_as_owner(&["questions"]).await;
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
    let _surface = ts.stream_as_owner(&["questions"]).await;
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
    let _surface = ts.stream_as_owner(&["questions"]).await;
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
    let mut tap = owner_tap(&ts).await;
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
    let _surface = ts.stream_as_owner(&["questions"]).await;
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
    let mut tap = owner_tap(&ts).await;
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
    let _surface = ts.stream_as_owner(&["questions"]).await;
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
    let mut tap = owner_tap(&ts).await;
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
    let mut tap = owner_tap(&ts).await;
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
    let mut tap = owner_tap(&ts).await;
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
    let _surface = ts.stream_as_owner(&["questions"]).await;
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

#[tokio::test]
async fn owner_lists_and_answers_through_the_shell_routes() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let mut ev = ts.stream_as_owner(&["questions"]).await;
    let q: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), body())
        .await
        .json()
        .await
        .unwrap();
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    let e = ev.next_named("question").await;
    assert_eq!(
        (e["topic"].as_str(), e["question"]["id"].as_str()),
        (Some("questions"), Some(qid.as_str()))
    );
    assert!(
        e["question"].get("session_id").is_none(),
        "views never carry a session ID"
    );
    let list: Value = ts.get_authed("/api/questions").await.json().await.unwrap();
    assert_eq!(list["open"], 1);
    assert_eq!(list["questions"][0]["id"], qid.as_str());
    let bad = ts
        .answer_question_raw(&qid, json!({"answers": [{"selected": ["Z"]}]}))
        .await;
    assert_eq!(bad.status(), 400);
    let ok = ts
        .answer_question_raw(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    assert_eq!(ok.status(), 200);
    let ok: Value = ok.json().await.unwrap();
    assert_eq!(ok["question"]["answered_via"], "shell");
    assert_eq!(
        ev.next_named("question").await["question"]["status"],
        "answered"
    );
    // Answered once: a second answer is 409 with the question's state.
    let again = ts
        .answer_question_raw(&qid, json!({"answers": [{"selected": ["B"]}]}))
        .await;
    assert_eq!(again.status(), 409);
    let again: Value = again.json().await.unwrap();
    assert_eq!(again["error"]["code"], "question_closed");
    assert_eq!(again["question"]["status"], "answered");
}

#[tokio::test]
async fn listings_filter_by_status_and_limit() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let mut ids = vec![];
    for _ in 0..3 {
        ids.push(
            ts.ask(&sid, body()).await["question"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    ts.answer_question(&ids[0], json!({"answers": [{"selected": ["A"]}]}))
        .await;
    let get = |q: &'static str| {
        let ts = &ts;
        async move {
            let res = ts.get_authed(&format!("/api/questions{q}")).await;
            (res.status().as_u16(), res.json::<Value>().await.unwrap())
        }
    };
    let ids_of = |v: &Value| -> Vec<String> {
        v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["id"].as_str().unwrap().to_string())
            .collect()
    };
    let (s, open) = get("").await;
    assert_eq!((s, open["open"].as_u64()), (200, Some(2)));
    assert_eq!(
        ids_of(&open),
        [ids[1].clone(), ids[2].clone()],
        "oldest first"
    );
    let (_, closed) = get("?status=closed").await;
    assert_eq!(ids_of(&closed), [ids[0].clone()]);
    let (_, all) = get("?status=all&limit=2").await;
    assert_eq!(ids_of(&all), [ids[2].clone(), ids[1].clone()]);
    assert_eq!(get("?status=nope").await.0, 400);
    assert_eq!(get("?limit=x").await.0, 400);
    let one: Value = ts
        .get_authed(&format!("/api/questions/{}", ids[0]))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(one["question"]["status"], "answered");
    assert_eq!(
        ts.get_authed("/api/questions/01J9ZZZZZZZZZZZZZZZZZZZZZZ")
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn the_owner_declines_and_hands_questions_to_the_terminal() {
    let ts = TestServer::spawn().await;
    let mut ev = ts.stream_as_owner(&["questions"]).await;
    let sid = session(&ts, "h1").await;
    let asked = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mirrored = ts.ask(&sid, hook("t1")).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let owner_post = |path: String| {
        ts.client
            .post(format!("{}{path}", ts.base))
            .header("cookie", ts.owner_cookie())
            .send()
    };
    let r = owner_post(format!("/api/questions/{asked}/release"))
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "an ask question has no terminal");
    let r = owner_post(format!("/api/questions/{mirrored}/release"))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = owner_post(format!("/api/questions/{asked}/decline"))
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["question"]["status"], "declined");
    let r = owner_post(format!("/api/questions/{mirrored}/decline"))
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["question"]["status"], "released");
    let mut seen = vec![];
    while seen.len() < 4 {
        let e = ev.next_named("question").await;
        seen.push(e["question"]["status"].as_str().unwrap().to_string());
    }
    assert_eq!(seen, ["open", "open", "released", "declined"]);
}

#[tokio::test]
async fn the_cli_token_answers_as_the_cli() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v: Value = post(
        &ts,
        &format!("/api/questions/{qid}/answer"),
        json!({"answers": [{"selected": ["B"]}]}),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(v["question"]["answered_via"], "cli");
}

#[tokio::test]
async fn lan_viewer_and_foreign_origin_are_refused() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sid = session(&ts, "h1").await;
    let q: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), body())
        .await
        .json()
        .await
        .unwrap();
    let qid = q["question"]["id"].as_str().unwrap();
    let (lan, base) = ts.lan();
    assert_eq!(
        lan.get(format!("{base}/api/questions"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        lan.post(format!("{base}/api/questions/{qid}/answer"))
            .json(&json!({"answers": [{"selected": ["A"]}]}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    // A local viewer cookie alone is no owner.
    let v = ts.viewer(Some("Mia")).await;
    let res = ts
        .client
        .get(format!("{}/api/questions/{qid}", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // The owner cookie from a page of another origin.
    let res = ts
        .client
        .post(format!("{}/api/questions/{qid}/answer", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", "http://localhost:5173")
        .json(&json!({"answers": [{"selected": ["A"]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    // Or a page's no-cors read.
    let res = ts
        .client
        .get(format!("{}/api/questions", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(ts.question_status(qid).await, "open");
    // A named viewer's stream may not take the topic.
    assert_eq!(v.subscribe_status(&["questions"]).await, 403);
    // /api/events never carries questions.
    let mut events = ts.events("").await;
    post(&ts, &format!("/api/sessions/{sid}/questions"), body()).await;
    ts.publish("marker", &[("index.html", "<p>m</p>")]).await;
    assert_eq!(
        events.next().await.0,
        "version",
        "the question event was skipped"
    );
}

#[tokio::test]
async fn owner_routes_refuse_lan_owner_cookies_artifact_origins_and_viewers() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let sid = session(&ts, "h1").await;
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (lan, base) = ts.lan();
    let v = ts.viewer(Some("Mia")).await;
    let viewer = format!("clax_viewer={}", v.cookie);
    let aid = ts.publish("page", &[("index.html", "<p>p</p>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let page = format!("http://{aid}.localhost:{}", ts.addr.port());
    for action in ["answer", "decline", "release"] {
        let path = format!("/api/questions/{qid}/{action}");
        let b = json!({"answers": [{"selected": ["A"]}]});
        let with_body = |r: reqwest::RequestBuilder| {
            if action == "answer" { r.json(&b) } else { r }
        };
        // A LAN peer presenting the owner cookie: the cookie needs a local peer.
        let res = with_body(lan.post(format!("{base}{path}")))
            .header("cookie", ts.owner_cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "LAN owner cookie, {action}");
        // An artifact's own origin, with the owner cookie.
        let res = with_body(ts.client.post(format!("{}{path}", ts.base)))
            .header("cookie", ts.owner_cookie())
            .header("origin", &page)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "artifact origin, {action}");
        // A local viewer cookie alone.
        let res = with_body(ts.client.post(format!("{}{path}", ts.base)))
            .header("cookie", &viewer)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 403, "viewer cookie, {action}");
    }
    let res = lan
        .get(format!("{base}/api/questions/{qid}"))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "LAN owner cookie, read");
    let res = ts
        .client
        .get(format!("{}/api/questions", ts.base))
        .header("cookie", ts.owner_cookie())
        .header("origin", &page)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "artifact origin, list");
    assert_eq!(ts.question_status(&qid).await, "open");
}

#[tokio::test]
async fn hook_mode_is_terminal_without_a_surface() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let mut b = hook("t1");
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), b.clone())
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["mode"].as_str(), r["question"]["status"].as_str()),
        (Some("terminal"), Some("released"))
    );
    assert_eq!(r["surface_open"], false);
    // A stream holding other topics is no surface.
    let _gallery = ts.stream_as_owner(&["gallery"]).await;
    b["tool_use_id"] = json!("t2");
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), b.clone())
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(r["mode"], "terminal");
    let _surface = ts.stream_as_owner(&["questions"]).await;
    b["tool_use_id"] = json!("t3");
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), b)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        (r["mode"].as_str(), r["surface_open"].as_bool()),
        (Some("wait"), Some(true))
    );
}

#[tokio::test]
async fn a_late_answer_is_handed_over_once_by_the_feedback_poll() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let q: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), body())
        .await
        .json()
        .await
        .unwrap();
    let qid = q["question"]["id"].as_str().unwrap().to_string();
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/feedback?wait=60", ts.base)),
    );
    let waiter =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    ts.wait_feedback_waiters(&sid, 1).await;
    ts.answer_question(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    let got = waiter.await.unwrap();
    assert_eq!(got["answers"][0]["id"], qid.as_str());
    assert_eq!(got["feedback"], json!([]));
    let text = got["text"].as_str().unwrap();
    assert!(
        text.contains(&format!(
            "[clax] The person answered your question \"Pick\" ({qid}, asked just now):"
        )),
        "{text}"
    );
    assert!(text.contains("  Pick: \"A\""), "{text}");
    let again: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook"))
        .await
        .json()
        .await
        .unwrap();
    assert!(again["answers"].as_array().unwrap().is_empty());
    assert!(again["text"].is_null());
}

#[tokio::test]
async fn an_answer_a_question_poll_holds_reaches_only_that_poll() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    // Answered while a poll holds it (closed in the store, so the poll is
    // not woken): no feedback poll takes it until the poll lets go.
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/questions/{qid}?wait=60",
        ts.base
    )));
    let ask = tokio::spawn(async move { req.send().await });
    ts.wait_question_waiters(&qid, 1).await;
    ts.store
        .close_question(&qid, clax_core::store::questions::Close::Decline)
        .unwrap();
    assert_eq!(
        late(&ts, &sid).await,
        json!([]),
        "held by the question poll"
    );
    ask.abort();
    ts.wait_question_waiters(&qid, 0).await;
    assert_eq!(late(&ts, &sid).await[0]["id"], qid.as_str(), "let go");
    // Each round races the question poll against a feedback poll woken by
    // the same answer; the feedback poll then returns with the next
    // question's answer alone.
    let feedback = |ts: &TestServer| {
        let req = ts.authed(
            ts.client
                .get(format!("{}/api/sessions/{sid}/feedback?wait=60", ts.base)),
        );
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() })
    };
    for round in 0..10 {
        let held = ts.ask(&sid, body()).await["question"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let req = ts.authed(ts.client.get(format!(
            "{}/api/sessions/{sid}/questions/{held}?wait=60",
            ts.base
        )));
        let ask =
            tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
        ts.wait_question_waiters(&held, 1).await;
        let waiter = feedback(&ts);
        ts.wait_feedback_waiters(&sid, 1).await;
        ts.answer_question(&held, json!({"answers": [{"selected": ["A"]}]}))
            .await;
        assert_eq!(ask.await.unwrap()["question"]["status"], "answered");
        let next = ts.ask(&sid, body()).await["question"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        ts.answer_question(&next, json!({"answers": [{"selected": ["B"]}]}))
            .await;
        let got = waiter.await.unwrap();
        let ids: Vec<&str> = got["answers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![next.as_str()], "round {round}: {got}");
        ts.wait_feedback_waiters(&sid, 0).await;
    }
}

#[tokio::test]
async fn a_held_answer_left_untaken_wakes_the_feedback_poll_when_the_hold_ends() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let req = ts.authed(ts.client.get(format!(
        "{}/api/sessions/{sid}/questions/{qid}?wait=60",
        ts.base
    )));
    let ask = tokio::spawn(async move { req.send().await });
    ts.wait_question_waiters(&qid, 1).await;
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/feedback?wait=60", ts.base)),
    );
    let waiter =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    ts.wait_feedback_waiters(&sid, 1).await;
    // Answered with no wake-up (closed in the store), then the poll lets go.
    ts.store
        .close_question(&qid, clax_core::store::questions::Close::Decline)
        .unwrap();
    ask.abort();
    let got = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the feedback poll was woken when the hold ended")
        .unwrap();
    assert_eq!(got["answers"][0]["id"], qid.as_str());
}

#[tokio::test]
async fn a_late_answers_age_is_read_from_the_question_clock() {
    let clock = std::sync::Arc::new(clax_core::working::ManualClock::at(&clax_core::Store::now()));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.question_clock = c).await;
    let sid = session(&ts, "h1").await;
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.answer_question(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    clock.advance(14 * 60 + 30);
    let got: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook"))
        .await
        .json()
        .await
        .unwrap();
    let text = got["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!(
            "[clax] The person answered your question \"Pick\" ({qid}, asked 14 min ago):"
        )),
        "{text}"
    );
}

#[tokio::test]
async fn a_skip_is_handed_over_and_codex_queue_leaves_answers_to_the_next_poll() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .client
        .post(format!("{}/api/questions/{qid}/decline", ts.base))
        .header("cookie", ts.owner_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let queue: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=queue"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        queue["answers"],
        json!([]),
        "the Codex queue is not sent answers"
    );
    let got: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["answers"][0]["status"], "declined");
    assert!(
        got["text"].as_str().unwrap().starts_with(&format!(
            "[clax] The person skipped your question \"Pick\" ({qid}):"
        )),
        "{got}"
    );
}

#[tokio::test]
async fn an_inject_poll_beside_a_wait_poll_carries_no_answers() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts, "h1").await;
    let req = ts.authed(
        ts.client
            .get(format!("{}/api/sessions/{sid}/feedback?wait=60", ts.base)),
    );
    let waiter =
        tokio::spawn(async move { req.send().await.unwrap().json::<Value>().await.unwrap() });
    ts.wait_feedback_waiters(&sid, 1).await;
    let inject: Value = ts
        .get_authed(&format!("/api/sessions/{sid}/feedback?tier=inject"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(inject["answers"], json!([]));
    let qid = ts.ask(&sid, body()).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.answer_question(&qid, json!({"answers": [{"selected": ["B"]}]}))
        .await;
    assert_eq!(waiter.await.unwrap()["answers"][0]["id"], qid.as_str());
}

#[tokio::test]
async fn the_extension_may_list_answer_and_follow_questions() {
    let ts = TestServer::spawn().await;
    let ext = ts.extension().await;
    let sid = session(&ts, "h1").await;
    let q: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), body())
        .await
        .json()
        .await
        .unwrap();
    let qid = q["question"]["id"].as_str().unwrap();
    assert_eq!(ext.get("/api/questions").await.status(), 200);
    assert_eq!(
        ext.get(&format!("/api/questions/{qid}")).await.status(),
        200
    );
    let r: Value = ext
        .post(
            &format!("/api/questions/{qid}/answer"),
            json!({"answers": [{"selected": ["B"]}]}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(r["question"]["answered_via"], "extension");
    // Its live-only stream may take the topic.
    let mut events = clax_server::testing::EventReader::from_response(ext.get("/api/stream").await);
    let id = events.next_named("ready").await["stream"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ext
        .post(
            &format!("/api/stream/{id}"),
            json!({"subscribe": ["questions"]}),
        )
        .await;
    assert_eq!(r.status(), 200);
    // The session routes stay closed to it.
    let r = ext
        .post(&format!("/api/sessions/{sid}/questions"), body())
        .await;
    assert_eq!(r.status(), 403);
}
