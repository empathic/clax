//! `publish_public`'s `content_security_policy`: an agent serves an
//! artifact's own site without Clax's Content Security Policy only once its
//! person approves in Clax, and returns it to `self` at once.

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

/// The `content-security-policy` of `aid`'s page as the owner's browser
/// reads it.
async fn page_policy(ts: &TestServer, aid: &str) -> String {
    let res = ts.site_get(aid, "/").await;
    assert_eq!(res.status(), 200);
    res.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string()
}

/// Whether `aid`'s page is held to its own site (the lockdown policy).
async fn held_to_self(ts: &TestServer, aid: &str) -> bool {
    let p = page_policy(ts, aid).await;
    if p.contains("connect-src 'self'") {
        return true;
    }
    assert!(
        p.starts_with("frame-ancestors ") && !p.contains(';'),
        "an unrestricted policy is frame-ancestors alone: {p}"
    );
    false
}

#[tokio::test]
async fn an_agent_serving_a_site_unrestricted_waits_for_approval() {
    let ts = TestServer::spawn().await;
    let t = agent_tools(&ts, "cp-1").await;
    let aid = ts.publish("Plan", &[("index.html", "<h1>Plan</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        held_to_self(&ts, &aid).await,
        "a new artifact is held to its own site"
    );
    let unrestricted =
        json!({"url_or_id": aid, "content_security_policy": "unrestricted", "timeout_s": 1});
    let (err, v) = call(&t, unrestricted.clone()).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "approval_pending", "{v}");
    assert_eq!(v["call_again"], true);
    assert!(
        v["note"]
            .as_str()
            .unwrap()
            .contains("content_security_policy"),
        "{v}"
    );
    let qid = v["question_id"].as_str().unwrap().to_string();
    assert!(
        held_to_self(&ts, &aid).await,
        "nothing changes before the answer"
    );
    let q = ts.store.question(&qid).unwrap().unwrap();
    assert_eq!(q.artifact_id.as_deref(), Some(aid.as_str()));
    // Asking again waits on the same question.
    let (_, again) = call(&t, unrestricted.clone()).await;
    assert_eq!(again["question_id"], qid.as_str());
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Serve it unrestricted"]}]}),
    )
    .await;
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "content_security_policy": "unrestricted", "question_id": qid}),
    )
    .await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "unrestricted", "{v}");
    assert_eq!(v["content_security_policy"], "unrestricted", "{v}");
    assert_eq!(
        v["site_url"],
        format!("http://{}/", ts.site_host(&aid)),
        "{v}"
    );
    assert!(!held_to_self(&ts, &aid).await);
    // Returning to `self` asks no one, and takes effect on the next request.
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "content_security_policy": "self"}),
    )
    .await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "self", "{v}");
    assert_eq!(v["content_security_policy"], "self", "{v}");
    assert!(held_to_self(&ts, &aid).await);
    // An approval is spent once used.
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "content_security_policy": "unrestricted", "question_id": qid}),
    )
    .await;
    assert!(err && v["error"]["code"] == "approval_used", "{v}");
    // Keeping it to its own site refuses the change.
    let (_, v) = call(&t, unrestricted.clone()).await;
    let qid = v["question_id"].as_str().unwrap().to_string();
    ts.answer_question(
        &qid,
        json!({"answers": [{"selected": ["Keep it to its own site"]}]}),
    )
    .await;
    let (err, v) = call(
        &t,
        json!({"url_or_id": aid, "content_security_policy": "unrestricted", "question_id": qid}),
    )
    .await;
    assert!(err && v["error"]["code"] == "not_approved", "{v}");
    assert!(held_to_self(&ts, &aid).await);
    // The owner by hand sets it once they confirm in their browser, as
    // the agent is live (`clax_http::confirm`).
    let res = ts
        .confirmed(
            reqwest::Method::PATCH,
            &format!("/api/artifacts/{aid}"),
            json!({"content_security_policy": "unrestricted"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    assert!(!held_to_self(&ts, &aid).await);
    // A remote, a mode or a site_access alongside is another kind of call.
    for extra in [
        json!({"mode": "new"}),
        json!({"remote": "pub"}),
        json!({"site_access": "world"}),
        json!({"content_security_policy": "open"}),
    ] {
        let mut args = json!({"url_or_id": aid, "content_security_policy": "unrestricted"});
        for (k, v) in extra.as_object().unwrap() {
            args[k] = v.clone();
        }
        let (err, v) = call(&t, args).await;
        assert!(err && v["error"]["code"] == "invalid_args", "{extra}: {v}");
    }
}
