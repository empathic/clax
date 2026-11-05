//! `publish_public`'s `site_access`: an agent opens an artifact's own site
//! to the world only once its person approves in Clax, and closes it at
//! once.

use clax_mcp::tools::{ClaxTools, PublishPublicArgs};
use clax_testkit::TestServer;
use clax_testkit::mcp::agent_tools;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};

async fn call(t: &ClaxTools, args: Value) -> (bool, Value) {
    let r = t
        .publish_public(Parameters(
            serde_json::from_value::<PublishPublicArgs>(args).unwrap(),
        ))
        .await
        .unwrap();
    let v = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    (r.is_error == Some(true), v)
}

/// Whether a request with no site session reads `aid`'s site.
async fn open_to_anyone(ts: &TestServer, aid: &str) -> bool {
    ts.site_get_anon(aid, "/").await.status() == 200
}

#[tokio::test]
async fn an_agent_opening_to_the_world_waits_for_approval() {
    let ts = TestServer::spawn().await;
    let t = agent_tools(&ts, "sa-1").await;
    let aid = ts.publish("Plan", &[("index.html", "<h1>Plan</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let open = json!({"url_or_id": aid, "site_access": "world", "timeout_s": 1});
    let (err, v) = call(&t, open.clone()).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "approval_pending", "{v}");
    assert_eq!(v["call_again"], true);
    let qid = v["question_id"].as_str().unwrap().to_string();
    assert!(
        !open_to_anyone(&ts, &aid).await,
        "nothing opens before the answer"
    );
    let q = ts.store.question(&qid).unwrap().unwrap();
    assert_eq!(q.artifact_id.as_deref(), Some(aid.as_str()));
    assert!(
        q.questions[0].question.contains("\"Plan\""),
        "{:?}",
        q.questions
    );
    // Asking again waits on the same question.
    let (_, again) = call(&t, open.clone()).await;
    assert_eq!(again["question_id"], qid.as_str());
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Open to the world"]}]}),
    )
    .await;
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "site_access": "world", "question_id": qid}),
    )
    .await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "open", "{v}");
    assert_eq!(
        v["site_url"],
        format!("http://{}/", ts.site_host(&aid)),
        "{v}"
    );
    assert!(open_to_anyone(&ts, &aid).await);
    // Closing asks no one, and takes effect on the next request.
    let (err, v) = call(&t, json!({"url_or_id": aid, "site_access": "private"})).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "private");
    assert!(!open_to_anyone(&ts, &aid).await);
    // An approval is spent once used.
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "site_access": "world", "question_id": qid}),
    )
    .await;
    assert!(err && v["error"]["code"] == "approval_used", "{v}");
    // Keeping it private refuses the opening.
    let (_, v) = call(&t, open.clone()).await;
    let qid = v["question_id"].as_str().unwrap().to_string();
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Keep it private"]}]}),
    )
    .await;
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "site_access": "world", "question_id": qid}),
    )
    .await;
    assert!(err && v["error"]["code"] == "not_approved", "{v}");
    assert!(!open_to_anyone(&ts, &aid).await);
    // The owner by hand opens it once they confirm in their browser, as
    // the agent is live (`clax_http::confirm`).
    let res = ts
        .confirmed(
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"site_access": "world"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    assert!(open_to_anyone(&ts, &aid).await);
    // A remote or a mode is another kind of publishing.
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "site_access": "world", "mode": "new"}),
    )
    .await;
    assert!(err && v["error"]["code"] == "invalid_args", "{v}");
}
