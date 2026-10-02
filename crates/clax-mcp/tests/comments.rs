//! The comment, watch, and feedback tools against an in-process daemon.

use clax_core::model::Session;
use clax_mcp::tools::{
    CommentsReadArgs, CommentsReplyArgs, CommentsResolveArgs, ListArgs, PublishArgs, StatusArgs,
    WaitArgs, WatchArgs, WorkingArgs,
};
use clax_mcp::{ClaxTools, DaemonClient};
use clax_server::testing::{FAKE_PNG, TestServer};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Tools attributed to a fresh `claude` session, and that session's ID.
async fn session_tools(ts: &TestServer) -> (ClaxTools, String) {
    session_tools_via(ts, &ts.base).await
}

/// Like [`session_tools`], sending API calls to `base` (such as a [`Proxy`]).
async fn session_tools_via(ts: &TestServer, base: &str) -> (ClaxTools, String) {
    let s: Session =
        serde_json::from_value(ts.register_session("claude", "tools-1").await).unwrap();
    let sid = s.id.clone();
    let tools = ClaxTools::new(
        DaemonClient::new(base.to_string(), ts.token.clone(), Some(sid.clone())),
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

/// A pass-through proxy in front of a test daemon that can fail the feedback
/// poll, and records each feedback acknowledgement body, first running a hook
/// (to land a request between a tool's read and its acknowledgement).
struct Proxy {
    upstream: String,
    http: reqwest::Client,
    fail_feedback: AtomicBool,
    before_ack: Mutex<Option<(String, Value)>>,
    acks: Mutex<Vec<Value>>,
}

impl Proxy {
    async fn start(upstream: &str) -> (Arc<Proxy>, String) {
        let p = Arc::new(Proxy {
            upstream: upstream.to_string(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            fail_feedback: AtomicBool::new(false),
            before_ack: Mutex::new(None),
            acks: Mutex::new(Vec::new()),
        });
        let app = axum::Router::new().fallback(forward).with_state(p.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (p, base)
    }
}

async fn forward(
    axum::extract::State(p): axum::extract::State<Arc<Proxy>>,
    req: axum::extract::Request,
) -> axum::response::Response {
    use axum::http::{Method, StatusCode, header};
    use axum::response::IntoResponse;
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let route = parts.uri.path();
    if parts.method == Method::GET
        && route.ends_with("/feedback")
        && p.fail_feedback.load(Ordering::SeqCst)
    {
        let err = json!({"error": {"code": "internal", "message": "injected"}});
        return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(err)).into_response();
    }
    if parts.method == Method::POST && route.ends_with("/feedback/ack") {
        p.acks
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&bytes).unwrap());
        let hook = p.before_ack.lock().unwrap().take();
        if let Some((url, body)) = hook {
            let res = p.http.post(url).json(&body).send().await.unwrap();
            assert_eq!(res.status(), 201);
        }
    }
    let target = format!(
        "{}{}",
        p.upstream,
        parts.uri.path_and_query().map_or("/", |x| x.as_str())
    );
    let mut rb = p.http.request(parts.method.clone(), target);
    for (k, v) in &parts.headers {
        if k != header::HOST && k != header::CONTENT_LENGTH {
            rb = rb.header(k, v);
        }
    }
    let res = rb.body(bytes).send().await.unwrap();
    let mut out = axum::http::Response::builder().status(res.status());
    for (k, v) in res.headers() {
        if k != header::TRANSFER_ENCODING && k != header::CONTENT_LENGTH {
            out = out.header(k, v);
        }
    }
    out.body(axum::body::Body::from(res.bytes().await.unwrap()))
        .unwrap()
}

async fn publish(t: &ClaxTools) -> String {
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
            "---\n[clax] 1 comment sent to you:\n[clax] Comment sent to you on \"Loop\""
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
    assert_eq!(th["anchor"]["file"], "index.html");
    assert_eq!(
        th["anchor"]["summary"],
        "body > main > h2  «Quarterly goals»"
    );
    assert_eq!(th["anchor"]["area"], Value::Null);
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
            c["via_harness"].as_str()
        ),
        (Some("agent"), Some("claude"), Some("claude"))
    );
    assert_eq!(got["thread"]["resolved_by"], "agent:claude");
    assert!(
        !got.to_string().contains(&sid),
        "no session ID in a public thread view"
    );
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
async fn status_reports_the_sessions_push() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let (s, _) = blocks(&t.status(Parameters(StatusArgs {})).await.unwrap());
    assert_eq!(s["push"]["tier"], Value::Null);
    assert_eq!(s["push"]["available"], false);
    assert!(
        s["push"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Claude Code has no native push"),
        "{s}"
    );
}

#[tokio::test]
async fn wait_for_feedback_returns_on_a_send_and_asks_to_call_again() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let tid = ts.thread(&aid, 1, "live").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let waiting = t.wait_for_feedback(Parameters(WaitArgs {
        url_or_id: Some(aid.clone()),
        timeout_s: Some(60),
    }));
    let sending = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        ts.send_thread(&aid, &tid).await;
        Instant::now()
    };
    let (r, sent) = tokio::join!(waiting, sending);
    let answered = Instant::now();
    // Far inside the 60 s wait: the send woke it.
    assert!(answered.saturating_duration_since(sent) < Duration::from_secs(20));
    let (v, trailing) = blocks(&r.unwrap());
    assert_eq!(v["feedback"].as_array().unwrap().len(), 1);
    assert_eq!(v["call_again"], false);
    assert!(
        trailing
            .unwrap()
            .starts_with("---\n[clax] 1 comment sent to you:")
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
    // `timeout_s: 0` waits the one-second minimum, so a loop cannot spin.
    let started = Instant::now();
    let (v, _) = blocks(
        &t.wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(0),
        }))
        .await
        .unwrap(),
    );
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(
        v,
        json!({"feedback": [], "waited_s": 1, "call_again": true})
    );
}

#[tokio::test]
async fn session_tools_without_a_session_say_so() {
    let ts = TestServer::spawn().await;
    let t = ClaxTools::new(
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

#[tokio::test]
async fn a_comment_added_between_read_and_ack_is_still_delivered() {
    let ts = TestServer::spawn().await;
    let (proxy, base) = Proxy::start(&ts.base).await;
    let (t, _sid) = session_tools_via(&ts, &base).await;
    let aid = publish(&t).await;
    let th = ts.thread(&aid, 1, "@agent first").await;
    let tid = th["id"].as_str().unwrap().to_string();
    let first = th["comments"][0]["id"].as_str().unwrap().to_string();
    // The viewer's second comment lands after the tool's read, before its ack.
    *proxy.before_ack.lock().unwrap() = Some((
        format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base),
        json!({"body": "and the footer"}),
    ));
    let (v, trailing) = blocks(
        &t.comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid.clone(),
            ..Default::default()
        }))
        .await
        .unwrap(),
    );
    let bodies: Vec<&str> = v["threads"][0]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["@agent first"]);
    assert_eq!(
        *proxy.acks.lock().unwrap(),
        vec![json!({"comment_ids": [first]})]
    );
    let fed: Vec<&str> = v["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        fed,
        ["and the footer"],
        "the unseen comment still reaches the agent"
    );
    assert!(trailing.is_some());
}

#[tokio::test]
async fn a_failed_piggyback_fetch_leaves_the_result_a_success() {
    let ts = TestServer::spawn().await;
    let (proxy, base) = Proxy::start(&ts.base).await;
    let (t, _sid) = session_tools_via(&ts, &base).await;
    let aid = publish(&t).await;
    ts.thread(&aid, 1, "@agent pending").await;
    proxy.fail_feedback.store(true, Ordering::SeqCst);
    let r = t.list(Parameters(ListArgs::default())).await.unwrap();
    assert_eq!(r.content.len(), 1, "{r:?}");
    let (v, _) = blocks(&r);
    assert_eq!(v["feedback"], json!([]));
    assert_eq!(v["artifacts"][0]["id"], aid);
    proxy.fail_feedback.store(false, Ordering::SeqCst);
    let (v, _) = blocks(&t.list(Parameters(ListArgs::default())).await.unwrap());
    assert_eq!(v["feedback"][0]["body"], "@agent pending");
}

#[tokio::test]
async fn pending_feedback_does_not_attach_to_an_error_result() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    ts.thread(&aid, 1, "@agent pending").await;
    let bad = t
        .comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: aid,
            thread_id: "nope".into(),
            text: "x".into(),
        }))
        .await
        .unwrap();
    assert_eq!(bad.is_error, Some(true));
    assert_eq!(bad.content.len(), 1);
    let e: Value = serde_json::from_str(&bad.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(e["feedback"], json!([]));
    let (v, _) = blocks(&t.list(Parameters(ListArgs::default())).await.unwrap());
    assert_eq!(
        v["feedback"][0]["body"], "@agent pending",
        "still pending after the error"
    );
}

#[tokio::test]
async fn comments_read_describes_a_drawn_area() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let area = json!({"x": 0.1, "y": 0.2, "w": 0.4213, "h": 0.18});
    let anchor = json!({"kind": "area", "selector": "body > main", "area": area});
    let form = reqwest::multipart::Form::new()
        .text("anchor", anchor.to_string())
        .text("body", "what is this gap?")
        .text("version", "1");
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{aid}/threads", ts.base))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let (v, _) = blocks(
        &t.comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid.clone(),
            ..Default::default()
        }))
        .await
        .unwrap(),
    );
    let a = &v["threads"][0]["anchor"];
    assert_eq!(a["kind"], "area");
    assert_eq!(a["area"], area);
    assert_eq!(a["summary"], "area in body > main (42% × 18%)");
}

#[tokio::test]
async fn working_marks_the_artifact_and_done_clears_it() {
    let ts = TestServer::spawn().await;
    let (tools, _sid) = session_tools(&ts).await;
    let (pub_, _) = blocks(
        &tools
            .publish(Parameters(PublishArgs {
                html: Some("<h2>Goals</h2>".into()),
                title: Some("T".into()),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    let aid = pub_["artifact_id"].as_str().unwrap().to_string();
    let tid = ts.thread(&aid, 1, "@agent columns").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (v, _) = blocks(
        &tools
            .working(Parameters(WorkingArgs {
                url_or_id: aid.clone(),
                thread_ids: Some(vec![tid.clone()]),
                message: Some("Two columns".into()),
                done: None,
            }))
            .await
            .unwrap(),
    );
    assert_eq!(v["working"], true);
    assert_eq!(v["message"], "Two columns");
    assert_eq!(v["thread_ids"], json!([tid]));
    assert_eq!(v["expires_in_s"], 120);
    assert_eq!(v["message_truncated"], false);
    let public: Value = ts
        .get(&format!("/api/artifacts/{aid}/working"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(public["working"][0]["message"], "Two columns");
    let (v, _) = blocks(
        &tools
            .working(Parameters(WorkingArgs {
                url_or_id: aid.clone(),
                done: Some(true),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    assert_eq!(
        v,
        json!({"artifact_id": aid, "url": v["url"], "working": false, "cleared": true, "feedback": v["feedback"]})
    );
}

#[tokio::test]
async fn working_refuses_bad_threads_and_the_sessionless_endpoint() {
    let ts = TestServer::spawn().await;
    let (tools, _) = session_tools(&ts).await;
    let (pub_, _) = blocks(
        &tools
            .publish(Parameters(PublishArgs {
                html: Some("<p>".into()),
                title: Some("T".into()),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    let aid = pub_["artifact_id"].as_str().unwrap().to_string();
    let err = |r: CallToolResult| -> String {
        assert_eq!(r.is_error, Some(true));
        let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
        v["error"]["code"].as_str().unwrap().to_string()
    };
    let r = tools
        .working(Parameters(WorkingArgs {
            url_or_id: aid.clone(),
            thread_ids: Some(vec!["x".into()]),
            ..Default::default()
        }))
        .await
        .unwrap();
    assert_eq!(err(r), "invalid_args");
    let r = tools
        .working(Parameters(WorkingArgs {
            url_or_id: aid.clone(),
            thread_ids: Some(vec![clax_core::new_ulid()]),
            ..Default::default()
        }))
        .await
        .unwrap();
    assert_eq!(err(r), "unknown_thread");
    let sessionless = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        ts.base.clone(),
        None,
        ts.home.log_path(),
    );
    let r = sessionless
        .working(Parameters(WorkingArgs {
            url_or_id: aid,
            ..Default::default()
        }))
        .await
        .unwrap();
    assert_eq!(err(r), "no_session");
}

#[tokio::test]
async fn publish_carries_a_note_and_links_addressed_and_working_threads() {
    let ts = TestServer::spawn().await;
    let (tools, sid) = session_tools(&ts).await;
    let (v1, _) = blocks(
        &tools
            .publish(Parameters(PublishArgs {
                html: Some("<h2>Goals</h2>".into()),
                title: Some("T".into()),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    let aid = v1["artifact_id"].as_str().unwrap().to_string();
    let t1 = ts.thread(&aid, 1, "@agent a").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t2 = ts.thread(&aid, 1, "plain b").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback"))
        .await;
    let (v2, _) = blocks(
        &tools
            .publish(Parameters(PublishArgs {
                id: Some(aid.clone()),
                html: Some("<h2>Goals</h2><p>2</p>".into()),
                note: Some("Two columns".into()),
                addresses: Some(vec![t2.clone()]),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    assert_eq!(v2["note"], "Two columns");
    assert_eq!(v2["note_truncated"], false);
    assert_eq!(v2["addressed"], json!([t2, t1]));
    let t: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{t1}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(t["thread"]["status"], "open");
}
