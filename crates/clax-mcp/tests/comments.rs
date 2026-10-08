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
    fail_question_poll: AtomicBool,
    before_ack: Mutex<Option<(String, Value)>>,
    acks: Mutex<Vec<Value>>,
}

impl Proxy {
    async fn start(upstream: &str) -> (Arc<Proxy>, String) {
        let p = Arc::new(Proxy {
            upstream: upstream.to_string(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            fail_feedback: AtomicBool::new(false),
            fail_question_poll: AtomicBool::new(false),
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
    if parts.method == Method::GET
        && route.contains("/questions/")
        && p.fail_question_poll.load(Ordering::SeqCst)
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
            addressed: None,
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
            addressed: None,
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
            addressed: None,
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
            .starts_with("optional: comments sent to this session already arrive"),
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
    // A wait that ran out: nothing, and a prompt to call again. `waited_s` is
    // the daemon's wall clock, at least the timeout and more on a loaded
    // machine.
    assert_ran_out(&v, 1);
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
    assert_ran_out(&v, 1);
}

/// `v` is the answer to a wait that ran out after at least `secs` seconds.
fn assert_ran_out(v: &Value, secs: u64) {
    assert_eq!(v["feedback"], json!([]), "{v}");
    assert_eq!(v["call_again"], true, "{v}");
    assert!(v["waited_s"].as_u64().unwrap() >= secs, "{v}");
    assert_eq!(v.as_object().unwrap().len(), 3, "{v}");
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
            addressed: None,
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

#[tokio::test]
async fn a_batch_piggybacks_as_one_group_on_the_next_tool_result() {
    let ts = TestServer::spawn().await;
    let (tools, _sid) = session_tools(&ts).await;
    let (p, _) = blocks(
        &tools
            .publish(Parameters(PublishArgs {
                html: Some("<h2>Goals</h2>".into()),
                title: Some("Batch".into()),
                ..Default::default()
            }))
            .await
            .unwrap(),
    );
    let aid = p["artifact_id"].as_str().unwrap().to_string();
    let a = ts.thread(&aid, 1, "one").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = ts.thread(&aid, 1, "two").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.client
        .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .json(&json!({"thread_ids": [a, b]}))
        .send()
        .await
        .unwrap();
    let (v, trailing) = blocks(&tools.list(Parameters(ListArgs::default())).await.unwrap());
    assert_eq!(v["feedback"].as_array().unwrap().len(), 2);
    assert_eq!(v["feedback"][0]["batch"]["size"], 2);
    let text = trailing.unwrap();
    assert!(text.starts_with("---\n[clax] 2 comments sent to you:\n[clax] 2 comments on \"Batch\", sent together by Viewer.\n"), "{text}");
}

#[tokio::test]
async fn addressed_is_refused_on_an_html_artifact() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let aid = publish(&t).await;
    let sent = ts.thread(&aid, 1, "@agent fix it").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let res = t
        .comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: aid.clone(),
            thread_id: sent.clone(),
            text: "Fixed.".into(),
            addressed: Some(true),
        }))
        .await
        .unwrap();
    assert_eq!(res.is_error, Some(true));
    let e: Value = serde_json::from_str(&res.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(e["error"]["code"], "invalid_args");
    let got: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{sent}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        got["thread"]["comments"].as_array().unwrap().len(),
        1,
        "nothing was written"
    );
}

#[tokio::test]
async fn a_late_answer_ends_wait_for_feedback_and_rides_on_a_tool_result_once() {
    let ts = TestServer::spawn().await;
    let (t, sid) = session_tools(&ts).await;
    let ask = |label: &str| {
        json!({"source": "ask", "questions": [{"question": "Which?", "header": label,
            "options": [{"label": "A"}, {"label": "B"}]}]})
    };
    let q1 = ts.ask(&sid, ask("First")).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.answer_question(&q1, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    let (v, trailing) = blocks(
        &t.wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(v["call_again"], false);
    assert_eq!(v["feedback"], json!([]));
    assert_eq!(v["answers"][0]["id"], q1.as_str());
    assert!(
        trailing
            .unwrap()
            .starts_with("---\n[clax] The person answered your question \"First\""),
    );
    let q2 = ts.ask(&sid, ask("Second")).await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.answer_question(&q2, json!({"answers": [{"selected": ["B"]}]}))
        .await;
    let (v, trailing) = blocks(&t.status(Parameters(StatusArgs {})).await.unwrap());
    assert_eq!(v["answers"][0]["id"], q2.as_str());
    assert!(trailing.unwrap().contains("\"Second\""));
    let (v, trailing) = blocks(&t.status(Parameters(StatusArgs {})).await.unwrap());
    assert!(v.get("answers").is_none(), "handed over once");
    assert!(trailing.is_none());
}

/// Tools attributed to a fresh session of `harness` on `ts`.
async fn harness_tools(ts: &TestServer, harness: &str, hsid: &str) -> ClaxTools {
    harness_tools_via(ts, harness, hsid, &ts.base).await
}

/// Like [`harness_tools`], sending API calls to `base` (such as a [`Proxy`]).
async fn harness_tools_via(ts: &TestServer, harness: &str, hsid: &str, base: &str) -> ClaxTools {
    let s: Session = serde_json::from_value(ts.register_session(harness, hsid).await).unwrap();
    let sid = s.id.clone();
    ClaxTools::new(
        DaemonClient::new(base.to_string(), ts.token.clone(), Some(sid)),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    )
}

/// `ask` with `args`; its JSON block, asserting a success.
async fn ask(tools: &ClaxTools, args: Value) -> Value {
    let r = tools
        .ask(Parameters(serde_json::from_value(args).unwrap()))
        .await
        .unwrap();
    blocks(&r).0
}

/// `ask` with `args`; its error code, asserting an error.
async fn ask_err(tools: &ClaxTools, args: Value) -> String {
    let r = tools
        .ask(Parameters(serde_json::from_value(args).unwrap()))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    v["error"]["code"].as_str().unwrap().to_string()
}

fn which() -> Value {
    json!([{"question": "Which?", "header": "Pick", "options": [{"label": "A"}, {"label": "B"}]}])
}

#[tokio::test]
async fn ask_returns_the_answer_and_resumes_after_call_again() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let tools = harness_tools(&ts, "claude", "ask-1").await;
    let t = tools.clone();
    let first =
        tokio::spawn(async move { ask(&t, json!({"questions": which(), "timeout_s": 1})).await });
    clock.wait_for(Duration::from_secs(1), 1).await;
    clock.fire(Duration::from_secs(1));
    let first = first.await.unwrap();
    assert_eq!(first["status"], "open");
    assert_eq!(first["call_again"], true);
    assert_eq!(first["surface_open"], false, "no owner surface is open");
    assert_eq!(first["reply"], Value::Null);
    assert!(first.get("answers").is_none(), "{first}");
    let qid = first["question_id"].as_str().unwrap().to_string();
    assert_eq!(
        first["url"],
        format!("http://localhost:{}/inbox?q={qid}", ts.addr.port())
    );

    let (t, q) = (tools.clone(), qid.clone());
    let waiting =
        tokio::spawn(async move { ask(&t, json!({"question_id": q, "timeout_s": 60})).await });
    ts.wait_question_waiters(&qid, 1).await;
    ts.answer_question(&qid, json!({"answers": [{"selected": ["B"]}]}))
        .await;
    let got = waiting.await.unwrap();
    assert_eq!(got["status"], "answered");
    assert_eq!(got["call_again"], false);
    assert!(got.get("surface_open").is_none(), "{got}");
    assert_eq!(
        got["reply"],
        json!([{"question": "Which?", "header": "Pick", "selected": ["B"], "text": null}])
    );
    assert!(got["note"].as_str().unwrap().contains("own words"));
    assert!(got.get("feedback").is_some());
    assert!(got.get("answers").is_none(), "no late answers: {got}");
    // The answer was handed over by `ask`, so no feedback tier repeats it.
    let (v, trailing) = blocks(&tools.status(Parameters(StatusArgs {})).await.unwrap());
    assert!(v.get("answers").is_none(), "{v}");
    assert!(trailing.is_none());
}

#[tokio::test]
async fn ask_reports_an_open_surface_and_its_page() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let _owner = ts.stream_as_owner(&["questions"]).await;
    let page = ts.publish("Dash", &[("index.html", "<p>dash</p>")]).await;
    let aid = page["artifact"]["id"].as_str().unwrap().to_string();
    let tools = harness_tools(&ts, "claude", "ask-surface").await;
    let t = tools.clone();
    let a = aid.clone();
    let first = tokio::spawn(async move {
        ask(
            &t,
            json!({"questions": which(), "url_or_id": a, "timeout_s": 2}),
        )
        .await
    });
    clock.wait_for(Duration::from_secs(2), 1).await;
    clock.fire(Duration::from_secs(2));
    let first = first.await.unwrap();
    assert_eq!(first["surface_open"], true);
    let qid = first["question_id"].as_str().unwrap();
    let row = ts.store.question(qid).unwrap().unwrap();
    assert_eq!(row.artifact_id.as_deref(), Some(aid.as_str()));
    // A declined question carries no answers.
    let res = ts
        .client
        .post(format!("{}/api/questions/{qid}/decline", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let got = ask(&tools, json!({"question_id": qid})).await;
    assert_eq!(
        (&got["status"], &got["reply"], &got["call_again"]),
        (&json!("declined"), &Value::Null, &json!(false))
    );
}

#[tokio::test]
async fn ask_cancel_and_foreign_ids() {
    let ts = TestServer::spawn().await;
    let tools = harness_tools(&ts, "claude", "ask-own").await;
    let other = harness_tools(&ts, "codex", "ask-other").await;
    let created = ts
        .ask(
            tools_session(&tools).await.as_str(),
            json!({"source": "ask", "questions": [{"question": "Q", "header": "H"}]}),
        )
        .await;
    let qid = created["question"]["id"].as_str().unwrap();
    assert_eq!(
        ask_err(&other, json!({"question_id": qid})).await,
        "not_found"
    );
    assert_eq!(
        ask_err(&other, json!({"question_id": qid, "cancel": true})).await,
        "not_found"
    );
    let c = ask(&tools, json!({"question_id": qid, "cancel": true})).await;
    assert_eq!(
        (&c["status"], &c["call_again"]),
        (&json!("withdrawn"), &json!(false))
    );
    // Cancelling again reports what closed it.
    let c = ask(&tools, json!({"question_id": qid, "cancel": true})).await;
    assert_eq!(c["status"], "withdrawn");

    assert_eq!(ask_err(&tools, json!({})).await, "invalid_args");
    assert_eq!(
        ask_err(&tools, json!({"questions": which(), "question_id": qid})).await,
        "invalid_args"
    );
    assert_eq!(
        ask_err(&tools, json!({"questions": which(), "cancel": true})).await,
        "invalid_args"
    );
    assert_eq!(
        ask_err(&tools, json!({"question_id": "../../x"})).await,
        "invalid_args"
    );
    assert_eq!(
        ask_err(
            &tools,
            json!({"questions": [{"question": "Q", "header": "Thirteen char"}]})
        )
        .await,
        "invalid_question"
    );
    assert_eq!(
        ask_err(
            &tools,
            json!({"questions": which(), "url_or_id": "not an id"})
        )
        .await,
        "invalid_id"
    );
}

/// The session ID `tools` act for, from `status`.
async fn tools_session(tools: &ClaxTools) -> String {
    let (v, _) = blocks(&tools.status(Parameters(StatusArgs {})).await.unwrap());
    v["session"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn cancelling_an_answered_question_hands_the_answer_over_once() {
    let ts = TestServer::spawn().await;
    let tools = harness_tools(&ts, "claude", "ask-late").await;
    let sid = tools_session(&tools).await;
    let qid = ts
        .ask(&sid, json!({"source": "ask", "questions": which()}))
        .await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.answer_question(&qid, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    let c = ask(&tools, json!({"question_id": qid, "cancel": true})).await;
    assert_eq!(c["status"], "answered");
    assert_eq!(c["reply"][0]["selected"], json!(["A"]));
    let (v, _) = blocks(&tools.status(Parameters(StatusArgs {})).await.unwrap());
    assert!(v.get("answers").is_none(), "handed over once: {v}");
}

#[tokio::test]
async fn ask_waits_50_s_by_default_under_codex() {
    assert_eq!(clax_mcp::tools::default_ask_wait("codex"), 50);
    assert_eq!(clax_mcp::tools::default_ask_wait("claude"), 600);
    assert_eq!(clax_mcp::tools::default_ask_wait("grok"), 600);
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let tools = harness_tools(&ts, "codex", "ask-codex").await;
    let t = tools.clone();
    let call = tokio::spawn(async move { ask(&t, json!({"questions": which()})).await });
    clock.wait_for(Duration::from_secs(50), 1).await;
    clock.fire(Duration::from_secs(50));
    assert_eq!(call.await.unwrap()["status"], "open");
}

#[tokio::test]
async fn ask_needs_a_session() {
    let ts = TestServer::spawn().await;
    let tools = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        format!("http://localhost:{}", ts.addr.port()),
        None,
        ts.home.log_path(),
    );
    assert_eq!(
        ask_err(&tools, json!({"questions": which()})).await,
        "no_session"
    );
}

/// Question `label` asked by session `sid` through the session route; its ID.
async fn asked(ts: &TestServer, sid: &str, header: &str) -> String {
    ts.ask(
        sid,
        json!({"source": "ask", "questions": [{"question": "Which?", "header": header,
            "options": [{"label": "A"}, {"label": "B"}]}]}),
    )
    .await["question"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn ask_keeps_its_reply_apart_from_late_answers_to_other_questions() {
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let tools = harness_tools(&ts, "claude", "ask-two").await;
    let sid = tools_session(&tools).await;
    // Q1 times out and asks to be called again; the agent moves on.
    let t = tools.clone();
    let first = tokio::spawn(async move {
        ask(
            &t,
            json!({"questions": [{"question": "First?", "header": "One"}], "timeout_s": 1}),
        )
        .await
    });
    clock.wait_for(Duration::from_secs(1), 1).await;
    clock.fire(Duration::from_secs(1));
    let first = first.await.unwrap();
    assert_eq!(
        (&first["status"], &first["call_again"]),
        (&json!("open"), &json!(true))
    );
    let q1 = first["question_id"].as_str().unwrap().to_string();
    // Q2 is asked, and the person answers both while Q2 waits.
    let t = tools.clone();
    let second = tokio::spawn(async move {
        ask(
            &t,
            json!({"questions": [{"question": "Second?", "header": "Two"}], "timeout_s": 60}),
        )
        .await
    });
    clock.wait_for(Duration::from_secs(60), 1).await;
    let q2 = ts
        .store
        .list_questions(clax_core::store::questions::ListStatus::Open, 10)
        .unwrap()
        .0
        .into_iter()
        .map(|r| r.id)
        .find(|id| *id != q1)
        .expect("Q2 is open");
    ts.answer_question(&q1, json!({"answers": [{"text": "one"}]}))
        .await;
    ts.answer_question(&q2, json!({"answers": [{"text": "two"}]}))
        .await;
    let got = second.await.unwrap();
    assert_eq!(got["question_id"], q2.as_str());
    assert_eq!(
        got["reply"],
        json!([{"question": "Second?", "header": "Two", "selected": [], "text": "two"}])
    );
    let late: Vec<&str> = got["answers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(late, [q1.as_str()], "{got}");
    // Each is handed over once.
    let (v, _) = blocks(&tools.status(Parameters(StatusArgs {})).await.unwrap());
    assert!(v.get("answers").is_none(), "{v}");

    // An open result keeps `reply: null` beside late answers.
    let q3 = asked(&ts, &sid, "Three").await;
    ts.answer_question(&q3, json!({"answers": [{"selected": ["A"]}]}))
        .await;
    let t = tools.clone();
    let open = tokio::spawn(async move {
        ask(
            &t,
            json!({"questions": [{"question": "Fourth?", "header": "Four"}], "timeout_s": 2}),
        )
        .await
    });
    clock.wait_for(Duration::from_secs(2), 1).await;
    clock.fire(Duration::from_secs(2));
    let open = open.await.unwrap();
    assert_eq!(open["status"], "open");
    assert_eq!(open["reply"], Value::Null);
    assert_eq!(open["answers"][0]["id"], q3.as_str());
}

#[tokio::test]
async fn a_failed_wait_after_asking_names_the_question() {
    let ts = TestServer::spawn().await;
    let (proxy, base) = Proxy::start(&ts.base).await;
    let tools = harness_tools_via(&ts, "claude", "ask-fail", &base).await;
    proxy.fail_question_poll.store(true, Ordering::SeqCst);
    let r = tools
        .ask(Parameters(
            serde_json::from_value(json!({"questions": which()})).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    let qid = v["error"]["question_id"]
        .as_str()
        .expect("the question is named");
    assert_eq!(ts.question_status(qid).await, "open");
    proxy.fail_question_poll.store(false, Ordering::SeqCst);
    let c = ask(&tools, json!({"question_id": qid, "cancel": true})).await;
    assert_eq!(c["status"], "withdrawn");
}

#[tokio::test]
async fn ask_waits_at_most_50_s_under_codex() {
    assert_eq!(clax_mcp::tools::max_ask_wait("codex"), 50);
    assert_eq!(clax_mcp::tools::max_ask_wait("claude"), 600);
    let (ts, clock) = TestServer::spawn_question_clock().await;
    let tools = harness_tools(&ts, "codex", "ask-codex-cap").await;
    let t = tools.clone();
    let call =
        tokio::spawn(async move { ask(&t, json!({"questions": which(), "timeout_s": 300})).await });
    clock.wait_for(Duration::from_secs(50), 1).await;
    clock.fire(Duration::from_secs(50));
    assert_eq!(call.await.unwrap()["status"], "open");
}
