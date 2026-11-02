//! `connect_instance`: an agent asks its person to connect another Clax
//! instance and waits for the answer; the person answers in their browser.

use clax_mcp::tools::{ClaxTools, ConnectInstanceArgs, Opener};
use clax_testkit::TestServer;
use clax_testkit::mcp::agent_tools;
use clax_testkit::remotes::{Public, json_of};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

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
    let t = Arc::new(agent_tools(&local, "ci-1").await);
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

#[tokio::test(flavor = "multi_thread")]
async fn connecting_to_an_artifact_watches_it() {
    let local = TestServer::spawn().await;
    let public = Public::new().await;
    clax_testkit::remotes::connect(&local, &public).await;
    let theirs = public.publish().await;
    let t = agent_tools(&local, "ci-watch").await;
    // Connected already: asking for a page there watches it at once, so
    // the agent shows there and is reached there.
    let page = format!("{}/a/{theirs}", public.origin);
    let (err, v) = call(&t, json!({"url_or_origin": page})).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "connected", "{v}");
    assert_eq!(v["watch"]["watching"], true, "{v}");
    assert_eq!(v["watch"]["artifact_id"], json!(theirs), "{v}");
    // The instance's origin alone names no page: nothing is watched.
    let (err, v) = call(&t, json!({"url_or_origin": public.origin})).await;
    assert!(!err, "{v}");
    assert_eq!(v["status"], "connected", "{v}");
    assert!(
        v.get("watch").is_none() && v.get("watch_error").is_none(),
        "{v}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_instance_opens_its_card_in_a_signed_in_browser() {
    let dir = tempfile::tempdir().unwrap();
    let opener = clax_fake_exe::install(
        &dir.path().join("open"),
        clax_testkit::opener::RECORDING_OPENER,
    );
    let (local, clock) = TestServer::spawn_question_clock().await;
    let public = Public::new().await;
    let t = Arc::new(
        agent_tools(&local, "ci-open")
            .await
            .with_opener(Opener::Program(opener))
            .with_open_wait(Duration::from_secs(30)),
    );
    let asking = {
        let (t, url) = (t.clone(), format!("{}/a/7q3k9mzx2b4t", public.origin));
        tokio::spawn(async move { call(&t, json!({"url_or_origin": url, "timeout_s": 1})).await })
    };
    clock.wait_for(Duration::from_secs(1), 1).await;
    clock.fire(Duration::from_secs(1));
    let (err, v) = asking.await.unwrap();
    assert!(!err, "{v}");
    assert_eq!(v["opened"], true, "{v}");
    assert!(
        !v.to_string().contains("code="),
        "the result names no login link: {v}"
    );
    // The browser was handed a one-use login link to the card, which makes
    // a browser not yet the owner's one.
    let id = v["request_id"].as_str().unwrap().to_string();
    let link = clax_testkit::opener::recorded_url(
        &std::fs::read_to_string(dir.path().join("got")).unwrap(),
    );
    assert!(
        link.starts_with(&format!(
            "http://localhost:{}/login?code=cxi_",
            local.addr.port()
        )),
        "{link}"
    );
    assert!(
        link.ends_with(&format!("next=%2Finbox%3Fremote_request%3D{id}")),
        "{link}"
    );
    // Waiting on the request again opens nothing more.
    std::fs::remove_file(dir.path().join("got")).unwrap();
    let again = {
        let t = t.clone();
        tokio::spawn(async move { call(&t, json!({"request_id": id, "timeout_s": 1})).await })
    };
    clock.wait_for(Duration::from_secs(1), 1).await;
    clock.fire(Duration::from_secs(1));
    let (err, v) = again.await.unwrap();
    assert!(!err, "{v}");
    assert!(v.get("opened").is_none(), "{v}");
    assert!(!dir.path().join("got").exists());
}
