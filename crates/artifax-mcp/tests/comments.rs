//! The comment, watch, and feedback tools against an in-process daemon.

use artifax_core::model::Session;
use artifax_mcp::tools::{
    CommentsReadArgs, CommentsReplyArgs, CommentsResolveArgs, ListArgs, PublishArgs, StatusArgs,
    WaitArgs, WatchArgs,
};
use artifax_mcp::{ArtifaxTools, DaemonClient};
use artifax_server::testing::{FAKE_PNG, TestServer};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Tools attributed to a fresh `claude` session, and that session's ID.
async fn session_tools(ts: &TestServer) -> (ArtifaxTools, String) {
    let s: Session =
        serde_json::from_value(ts.register_session("claude", "tools-1").await).unwrap();
    let sid = s.id.clone();
    let tools = ArtifaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(sid.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    );
    (tools, sid)
}

/// The JSON block and, when present, the trailing text block.
fn blocks(r: &CallToolResult) -> (Value, Option<String>) {
    assert!(r.is_error != Some(true), "{r:?}");
    let v = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    (
        v,
        r.content.get(1).map(|b| b.as_text().unwrap().text.clone()),
    )
}

async fn publish(t: &ArtifaxTools) -> String {
    let r = t
        .publish(Parameters(PublishArgs {
            html: Some("<main><h2>Goals</h2></main>".into()),
            title: Some("Loop".into()),
            ..Default::default()
        }))
        .await
        .unwrap();
    blocks(&r).0["artifact_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn every_result_carries_pending_feedback_once() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let th = ts.thread(&aid, 1, "Make this two columns.").await;
    ts.send_thread(&aid, th["id"].as_str().unwrap()).await;
    let (v, trailing) = blocks(&t.list(Parameters(ListArgs::default())).await.unwrap());
    assert_eq!(v["feedback"].as_array().unwrap().len(), 1);
    assert_eq!(v["feedback"][0]["body"], "Make this two columns.");
    let trailing = trailing.expect("trailing block");
    assert!(
        trailing.starts_with(
            "---\n[artifax] 1 comment sent to you:\n[artifax] Comment sent to you on \"Loop\""
        ),
        "{trailing}"
    );
    assert!(trailing.ends_with("Reply with comments_reply, then comments_resolve when done."));
    let r = t.list(Parameters(ListArgs::default())).await.unwrap();
    assert_eq!(r.content.len(), 1);
    assert_eq!(blocks(&r).0["feedback"], json!([]));
}

#[tokio::test]
async fn comments_read_summarises_threads_and_acknowledges_them() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let res: Value = ts
        .create_thread(&aid, 1, "@agent drop the third bullet", Some(FAKE_PNG))
        .await
        .json()
        .await
        .unwrap();
    let tid = res["thread"]["id"].as_str().unwrap().to_string();
    let (v, _) = blocks(
        &t.comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid.clone(),
            ..Default::default()
        }))
        .await
        .unwrap(),
    );
    let th = &v["threads"][0];
    assert_eq!(th["thread_id"], tid);
    assert_eq!(th["sent_to_agent"], true);
    assert_eq!(th["anchor"]["selector"], "body > main > h2");
    assert_eq!(th["comments"][0]["body"], "@agent drop the third bullet");
    let clip = th["clip_path"].as_str().unwrap();
    assert!(
        std::path::Path::new(clip).is_absolute() && std::path::Path::new(clip).exists(),
        "{clip}"
    );
    assert!(
        v["note"]
            .as_str()
            .unwrap()
            .contains("people viewing the page")
    );
    assert_eq!(
        v["feedback"],
        json!([]),
        "reading acknowledged it, so nothing piggybacks"
    );
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["thread"]["feedback_state"]["state"], "acknowledged");
    let one = blocks(
        &t.comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid.clone(),
            thread_id: Some(tid.clone()),
            ..Default::default()
        }))
        .await
        .unwrap(),
    )
    .0;
    assert_eq!(one["threads"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn reply_and_resolve_follow_the_sent_rule() {
    let ts = TestServer::spawn().await;
    let (t, sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let plain = ts.thread(&aid, 1, "plain note").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let sent = ts.thread(&aid, 1, "@agent fix it").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (v, _) = blocks(
        &t.comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: aid.clone(),
            thread_id: plain.clone(),
            text: "ok".into(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(v["replied"], false);
    assert!(v["guidance"].as_str().unwrap().contains("not sent to you"));
    let (v, _) = blocks(
        &t.comments_resolve(Parameters(CommentsResolveArgs {
            url_or_id: aid.clone(),
            thread_id: plain,
        }))
        .await
        .unwrap(),
    );
    assert_eq!(v["resolved"], false);
    let (v, _) = blocks(
        &t.comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: aid.clone(),
            thread_id: sent.clone(),
            text: "Fixed.".into(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(v["replied"], true);
    let (v, _) = blocks(
        &t.comments_resolve(Parameters(CommentsResolveArgs {
            url_or_id: aid.clone(),
            thread_id: sent.clone(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (v["resolved"].clone(), v["status"].clone()),
        (json!(true), json!("resolved"))
    );
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{sent}"))
        .await
        .json()
        .await
        .unwrap();
    let c = &got["thread"]["comments"][1];
    assert_eq!(
        (
            c["author_kind"].as_str(),
            c["author_name"].as_str(),
            c["via_session_id"].as_str()
        ),
        (Some("agent"), Some("claude"), Some(sid.as_str()))
    );
    assert_eq!(got["thread"]["resolved_by"], format!("agent:{sid}"));
    let bad = t
        .comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: aid,
            thread_id: "../../x".into(),
            text: "x".into(),
        }))
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    let e: Value = serde_json::from_str(&bad.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(e["error"]["code"], "invalid_args");
}

#[tokio::test]
async fn watch_toggles_and_status_lists_watches() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let (v, _) = blocks(
        &t.watch(Parameters(WatchArgs {
            url_or_id: aid.clone(),
            on: None,
            replies: Some(false),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (v["watching"].clone(), v["replies_armed"].clone()),
        (json!(true), json!(false))
    );
    let (s, _) = blocks(&t.status(Parameters(StatusArgs {})).await.unwrap());
    assert_eq!(s["watches"][0]["artifact_id"], aid);
    assert_eq!(s["watches"][0]["replies_armed"], false);
    let (v, _) = blocks(
        &t.watch(Parameters(WatchArgs {
            url_or_id: aid,
            on: Some(false),
            replies: None,
        }))
        .await
        .unwrap(),
    );
    assert_eq!(v["watching"], false);
    let (s, _) = blocks(&t.status(Parameters(StatusArgs {})).await.unwrap());
    assert_eq!(s["watches"], json!([]));
}

#[tokio::test]
async fn wait_for_feedback_returns_within_a_second_and_asks_to_call_again() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let tid = ts.thread(&aid, 1, "live").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let waiting = t.wait_for_feedback(Parameters(WaitArgs {
        url_or_id: Some(aid.clone()),
        timeout_s: Some(5),
    }));
    let sending = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        ts.send_thread(&aid, &tid).await;
        Instant::now()
    };
    let (r, sent) = tokio::join!(waiting, sending);
    let answered = Instant::now();
    assert!(answered.saturating_duration_since(sent) < Duration::from_secs(1));
    let (v, trailing) = blocks(&r.unwrap());
    assert_eq!(v["feedback"].as_array().unwrap().len(), 1);
    assert_eq!(v["call_again"], false);
    assert!(
        trailing
            .unwrap()
            .starts_with("---\n[artifax] 1 comment sent to you:")
    );
    let (v, trailing) = blocks(
        &t.wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        v,
        json!({"feedback": [], "waited_s": 1, "call_again": true})
    );
    assert!(trailing.is_none());
}

#[tokio::test]
async fn session_tools_without_a_session_say_so() {
    let ts = TestServer::spawn().await;
    let t = ArtifaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    );
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    for r in [
        t.watch(Parameters(WatchArgs {
            url_or_id: aid.clone(),
            on: None,
            replies: None,
        }))
        .await
        .unwrap(),
        t.wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await
        .unwrap(),
    ] {
        assert_eq!(r.is_error, Some(true));
        let e: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(e["error"]["code"], "no_session");
    }
    let (v, _) = blocks(
        &t.comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid,
            ..Default::default()
        }))
        .await
        .unwrap(),
    );
    assert_eq!(v["threads"], json!([]));
}
