//! `publish_public`: an agent publishes an artifact on a public Clax only
//! once the owner approves in Clax, and only as often as they allowed.

use clax_core::model::Session;
use clax_core::working::ManualClock;
use clax_mcp::client::DaemonClient;
use clax_mcp::tools::{ClaxTools, PublishPublicArgs};
use clax_testkit::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET: &str = "the setup secret";

fn result(r: &CallToolResult) -> Value {
    serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
}

/// A hosted daemon, and a local one signed in to it as its owner.
async fn signed_in() -> (TestServer, TestServer) {
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let public = TestServer::spawn_hosted(clock, Some(SECRET)).await;
    let host = format!("localhost:{}", public.addr.port());
    let origin = format!("http://{host}");
    let res = public
        .client
        .post(format!("{}/api/hosted/setup", public.base))
        .header("host", &host)
        .header("origin", &origin)
        .json(&json!({"secret": SECRET, "name": "Ada"}))
        .send()
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let local = TestServer::spawn().await;
    let v: Value = local
        .post_json("/api/remotes", json!({"url": origin}))
        .await
        .json()
        .await
        .unwrap();
    let code = v["user_code"].as_str().unwrap();
    let res = public
        .client
        .post(format!("{}/api/hosted/device/{code}/approve", public.base))
        .header("host", &host)
        .header("origin", &origin)
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let id = v["remote"]["id"].as_str().unwrap();
    let p: Value = local
        .post_json(&format!("/api/remotes/{id}/poll"), json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(p["status"], "connected", "{p}");
    (public, local)
}

async fn agent(ts: &TestServer) -> ClaxTools {
    let s: Session = serde_json::from_value(ts.register_session("claude", "pp-1").await).unwrap();
    ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(s.id.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    )
}

async fn call(t: &ClaxTools, args: Value) -> (bool, Value) {
    let r = t
        .publish_public(Parameters(
            serde_json::from_value::<PublishPublicArgs>(args).unwrap(),
        ))
        .await
        .unwrap();
    (r.is_error == Some(true), result(&r))
}

#[tokio::test]
async fn publish_public_waits_for_the_owner_and_publishes_only_what_they_approved() {
    let (public, local) = signed_in().await;
    let t = agent(&local).await;
    let aid = local
        .publish("Plan", &[("index.html", "<h1>Plan</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Unanswered: nothing is public, and the agent is told to call again.
    let (err, v) = call(&t, json!({"url_or_id": aid, "timeout_s": 1})).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "approval_pending");
    assert_eq!(v["call_again"], true);
    let qid = v["question_id"].as_str().unwrap().to_string();
    assert!(
        v["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/inbox?q={qid}"))
    );
    let listed: Value = public
        .get_authed("/api/artifacts")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(listed["artifacts"], json!([]));
    // The question is the daemon's, about this artifact and that remote.
    let q = local.store.question(&qid).unwrap().unwrap();
    assert_eq!(q.artifact_id.as_deref(), Some(aid.as_str()));
    assert!(
        q.questions[0].question.contains("\"Plan\""),
        "{:?}",
        q.questions
    );
    local
        .answer_question(&qid, json!({"answers": [{"selected": ["Publish"]}]}))
        .await;
    let (err, v) = call(&t, json!({"url_or_id": aid, "question_id": qid})).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "published");
    assert_eq!(v["follows_new_versions"], false);
    assert_eq!(v["pushed"], json!([{"n": 1, "remote_n": 1}]));
    // A new version needs the owner again, as they did not let it follow.
    let res = local
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            json!({"if_version": 1, "files": {"index.html": {"content": "<h1>v2</h1>", "encoding": "utf8"}}}),
        )
        .await;
    assert_eq!(res.status(), 201);
    let (_, v) = call(&t, json!({"url_or_id": aid, "timeout_s": 1})).await;
    assert_eq!(v["status"], "approval_pending", "{v}");
    let qid = v["question_id"].as_str().unwrap().to_string();
    local
        .answer_question(&qid, json!({"answers": [{"selected": ["Don't publish"]}]}))
        .await;
    let (err, v) = call(&t, json!({"url_or_id": aid, "question_id": qid})).await;
    assert!(err, "{v}");
    assert_eq!(v["error"]["code"], "not_approved", "{v}");
    let rid = local.store.remote_links(&aid).unwrap()[0]
        .remote_artifact_id
        .clone();
    let pv: Value = public
        .get_authed(&format!("/api/artifacts/{rid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(pv["artifact"]["current_version"], 1);
    // A bad mode is the agent's mistake, refused before anything is asked.
    let (err, v) = call(&t, json!({"url_or_id": aid, "mode": "overwrite"})).await;
    assert!(err && v["error"]["code"] == "invalid_args", "{v}");
}

/// An answer that arrives once the call's wait has run out leaves the push
/// to the next call, so the call ends within its wait (under Codex, inside
/// its tool timeout).
#[tokio::test]
async fn an_answer_after_the_wait_ran_out_leaves_the_push_to_the_next_call() {
    let (public, local) = signed_in().await;
    let clock = Arc::new(ManualClock::at("2026-10-12T10:00:00Z"));
    let t = agent(&local).await.with_clock(clock.clone());
    let aid = local
        .publish("Plan", &[("index.html", "<h1>Plan</h1>")])
        .await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, v) = call(&t, json!({"url_or_id": aid, "timeout_s": 1})).await;
    assert_eq!(v["status"], "approval_pending", "{v}");
    let qid = v["question_id"].as_str().unwrap().to_string();
    let waiting = {
        let (t, aid, qid) = (t.clone(), aid.clone(), qid.clone());
        tokio::spawn(async move {
            call(
                &t,
                json!({"url_or_id": aid, "question_id": qid, "timeout_s": 30}),
            )
            .await
        })
    };
    // The call waits on the question; its wait runs out, then the owner
    // approves.
    local.wait_question_waiters(&qid, 1).await;
    clock.advance(31);
    local
        .answer_question(&qid, json!({"answers": [{"selected": ["Publish"]}]}))
        .await;
    let (err, v) = waiting.await.unwrap();
    assert!(!err, "{v}");
    assert_eq!(v["status"], "answered", "{v}");
    assert_eq!(v["question_id"], qid.as_str());
    assert_eq!(v["call_again"], true);
    let listed: Value = public
        .get_authed("/api/artifacts")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(listed["artifacts"], json!([]), "nothing is public yet");
    let (err, v) = call(&t, json!({"url_or_id": aid, "question_id": qid})).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "published", "{v}");
}
