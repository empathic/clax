//! `connect_instance`: an agent asks its person to connect another Clax
//! instance and waits for the answer; the person answers in their browser.

use clax_core::model::Session;
use clax_mcp::client::DaemonClient;
use clax_mcp::tools::{ClaxTools, ConnectInstanceArgs};
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, json_of};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

async fn agent(ts: &TestServer) -> ClaxTools {
    let s: Session = serde_json::from_value(ts.register_session("claude", "ci-1").await).unwrap();
    ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(s.id.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    )
}

async fn call(t: &ClaxTools, args: Value) -> (bool, Value) {
    let r = t
        .connect_instance(Parameters(
            serde_json::from_value::<ConnectInstanceArgs>(args).unwrap(),
        ))
        .await
        .unwrap();
    let v = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    (r.is_error == Some(true), v)
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_instance_asks_once_and_waits_for_the_answer() {
    let (local, clock) = TestServer::spawn_question_clock().await;
    let public = Public::new().await;
    let t = Arc::new(agent(&local).await);
    let url = format!("{}/a/7q3k9mzx2b4t", public.origin);
    // Unanswered within its wait: the agent is told to call again.
    let asking = {
        let (t, url) = (t.clone(), url.clone());
        tokio::spawn(async move {
            call(
                &t,
                json!({"url_or_origin": url, "reason": "to read the plan", "timeout_s": 1}),
            )
            .await
        })
    };
    clock.wait_for(Duration::from_secs(1), 1).await;
    clock.fire(Duration::from_secs(1));
    let (err, v) = asking.await.unwrap();
    assert!(!err, "{v}");
    assert_eq!(
        (&v["status"], &v["call_again"]),
        (&json!("open"), &json!(true)),
        "{v}"
    );
    let id = v["request_id"].as_str().unwrap().to_string();
    assert!(
        v["tell_terminal"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/inbox?remote_request={id}")),
        "{v}"
    );
    // The person says Never, in their browser.
    let (s, a) = json_of(
        local
            .client
            .post(format!("{}/api/remotes/requests/{id}/answer", local.base))
            .header("cookie", local.owner_cookie())
            .json(&json!({"answer": "never"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 200, "{a}");
    let (err, v) = call(&t, json!({"request_id": id, "timeout_s": 1})).await;
    assert!(!err, "{v}");
    assert_eq!(
        (&v["status"], &v["call_again"]),
        (&json!("muted"), &json!(false)),
        "{v}"
    );
    // Asking again asks nobody.
    let (err, v) = call(&t, json!({"url_or_origin": public.origin})).await;
    assert!(!err, "{v}");
    assert_eq!(
        (&v["status"], &v["call_again"]),
        (&json!("muted"), &json!(false)),
        "{v}"
    );
    assert!(v.get("request_id").is_none(), "nothing to wait on: {v}");
    // Neither or both of url_or_origin and request_id is refused.
    let (err, v) = call(&t, json!({})).await;
    assert!(err, "{v}");
    assert_eq!(v["error"]["code"], "invalid_args", "{v}");
}
