//! `read` and the comment tools on an artifact of a connected remote (a
//! hosted test daemon), through the local daemon's remote artifact routes:
//! rendered as for a local artifact, with its address there and `remote`.

use clax_core::model::Session;
use clax_mcp::client::DaemonClient;
use clax_mcp::tools::{
    ClaxTools, CommentsHandoffArgs, CommentsReadArgs, CommentsReplyArgs, CommentsResolveArgs,
    ReadArgs,
};
use clax_testkit::TestServer;
use clax_testkit::remotes::{Public, connect, json_of};
use reqwest::Method;
use reqwest::header::{COOKIE, HOST, ORIGIN};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

async fn agent(ts: &TestServer) -> ClaxTools {
    let s: Session =
        serde_json::from_value(ts.register_session("claude", "remote-1").await).unwrap();
    ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(s.id.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    )
}

fn body(r: CallToolResult) -> Value {
    assert!(r.is_error != Some(true), "{r:?}");
    serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
}

async fn remote_artifact(public: &Public, page: &str) -> String {
    let (s, v) = public
        .cli(
            Method::POST,
            "/api/artifacts",
            Some(json!({"title": "Theirs",
                "files": {"index.html": {"content": page, "encoding": "utf8"}}})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    v["artifact"]["id"].as_str().unwrap().to_string()
}

async fn browser_thread(public: &Public, raid: &str, text: &str) -> String {
    let ts = &public.ts;
    let res = ts
        .client
        .post(format!("{}/api/artifacts/{raid}/threads", ts.base))
        .header(HOST, format!("localhost:{}", ts.addr.port()))
        .header(COOKIE, format!("clax_session={}", public.cookie))
        .header(ORIGIN, &public.origin)
        .multipart(
            reqwest::multipart::Form::new()
                .text("anchor", json!({"kind": "artifact"}).to_string())
                .text("body", text.to_string())
                .text("version", "1"),
        )
        .send()
        .await
        .unwrap();
    let (s, v) = json_of(res).await;
    assert_eq!(s, 201, "{v}");
    v["thread"]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_comment_tools_answer_a_remote_thread_as_a_local_one() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let name = remote["name"].as_str().unwrap().to_string();
    let raid = remote_artifact(&public, "<h1>Theirs</h1>").await;
    let tid = browser_thread(&public, &raid, "@agent fix the title").await;
    let t = agent(&local).await;
    let url = format!("{}/a/{raid}/v/1", public.origin);
    let at = format!("{}/a/{raid}", public.origin);
    let instance = json!({"origin": public.origin, "name": name});

    let r = body(
        t.read(Parameters(ReadArgs {
            url_or_id: url.clone(),
            ..Default::default()
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (r["content"].as_str(), r["version"].as_u64()),
        (Some("<h1>Theirs</h1>"), Some(1)),
        "{r}"
    );
    assert_eq!((&r["url"], &r["remote"]), (&json!(at), &instance), "{r}");

    let r = body(
        t.comments_read(Parameters(CommentsReadArgs {
            url_or_id: url.clone(),
            ..Default::default()
        }))
        .await
        .unwrap(),
    );
    assert_eq!((&r["url"], &r["remote"]), (&json!(at), &instance), "{r}");
    assert_eq!(r["artifact_id"], json!(raid), "{r}");
    let thread = &r["threads"][0];
    assert_eq!(thread["thread_id"], json!(tid), "{r}");
    assert_eq!(thread["sent_to_agent"], true, "{r}");
    assert_eq!(thread["comments"][0]["body"], "@agent fix the title", "{r}");
    assert!(r.get("withheld_comments").is_none(), "{r}");

    let r = body(
        t.comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: url.clone(),
            thread_id: tid.clone(),
            text: "Fixed.".into(),
            addressed: None,
        }))
        .await
        .unwrap(),
    );
    assert_eq!(r["replied"], true, "{r}");
    let r = body(
        t.comments_resolve(Parameters(CommentsResolveArgs {
            url_or_id: url.clone(),
            thread_id: tid.clone(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(
        (r["resolved"].as_bool(), r["status"].as_str()),
        (Some(true), Some("resolved")),
        "{r}"
    );
    let r = body(
        t.comments_handoff(Parameters(CommentsHandoffArgs {
            url_or_id: url.clone(),
            thread_id: tid.clone(),
            to: None,
        }))
        .await
        .unwrap(),
    );
    assert_eq!(r["released"], true, "{r}");
    assert_eq!(r["remote"], instance, "{r}");

    // There, the reply is the agent's, named as this session's harness, and
    // the thread is resolved.
    let (s, v) = public
        .cli(
            Method::GET,
            &format!("/api/artifacts/{raid}/threads/{tid}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["thread"]["status"], "resolved", "{v}");
    let last = v["thread"]["comments"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        (last["author_kind"].as_str(), last["body"].as_str()),
        (Some("agent"), Some("Fixed.")),
        "{v}"
    );

    // `addressed` is a live page's.
    let r = t
        .comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: url,
            thread_id: tid,
            text: "x".into(),
            addressed: Some(true),
        }))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true), "{r:?}");
}
