//! `read` and the comment tools on an artifact of a connected remote (a
//! hosted test daemon), through the local daemon's remote artifact routes:
//! rendered as for a local artifact, with its address there and `remote`.

use clax_mcp::tools::{
    CommentsHandoffArgs, CommentsReadArgs, CommentsReplyArgs, CommentsResolveArgs, ReadArgs,
};
use clax_testkit::TestServer;
use clax_testkit::mcp::agent_tools;
use clax_testkit::remotes::{Public, connect};
use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

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

#[tokio::test(flavor = "multi_thread")]
async fn the_comment_tools_answer_a_remote_thread_as_a_local_one() {
    let public = Public::new().await;
    let local = TestServer::spawn().await;
    let remote = connect(&local, &public).await;
    let name = remote["name"].as_str().unwrap().to_string();
    let raid = remote_artifact(&public, "<h1>Theirs</h1>").await;
    let tid = public
        .browser_thread(
            &raid,
            &public.cookie,
            json!({"kind": "artifact"}),
            "@agent fix the title",
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t = agent_tools(&local, "remote-1").await;
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

/// A daemon that answers `GET /api/resolve` with `status` and `error`, and
/// serves artifact `k3m9x2a7b8c4` (version 1, `index.html`): its base.
async fn resolving_daemon(status: u16, error: Value) -> String {
    use axum::routing::get;
    let resolve = get(move || {
        let error = error.clone();
        async move {
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                axum::Json(json!({"error": error})),
            )
        }
    });
    let app = axum::Router::new()
        .route("/api/resolve", resolve)
        .route(
            "/api/artifacts/k3m9x2a7b8c4",
            get(|| async {
                axum::Json(json!({
                    "artifact": {"id": "k3m9x2a7b8c4", "current_version": 1},
                    "versions": [{"n": 1, "files": {"index.html": {"content_type": "text/html"}}}],
                }))
            }),
        )
        .route(
            "/api/artifacts/k3m9x2a7b8c4/versions/1/files/index.html",
            get(|| async { ([("content-type", "text/html")], "<h1>Here</h1>") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn read_on(base: &str, url: &str) -> CallToolResult {
    let t = clax_mcp::tools::ClaxTools::new(
        clax_mcp::client::DaemonClient::new(base.to_string(), "tok".into(), None),
        "http://localhost:1".into(),
        None,
        std::path::PathBuf::from("/nonexistent/daemon.log"),
    );
    t.read(Parameters(
        serde_json::from_value::<ReadArgs>(json!({"url_or_id": url})).unwrap(),
    ))
    .await
    .unwrap()
}

fn error_code(r: &CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    v["error"]["code"].clone()
}

/// A daemon without `GET /api/resolve` (404) leaves a URL of its own to the
/// client's reading, as does one that answers `invalid_url` for a URL of its
/// own naming no artifact (`invalid_id`, as the client reads it).
#[tokio::test(flavor = "multi_thread")]
async fn a_url_is_read_here_only_when_the_daemon_has_no_resolver_or_names_nothing() {
    let old = resolving_daemon(404, json!({"code": "not_found", "message": "no route"})).await;
    let r = body(read_on(&old, "http://localhost:1/a/k3m9x2a7b8c4").await);
    assert_eq!(r["content"], "<h1>Here</h1>", "{r}");
    let nothing = resolving_daemon(
        400,
        json!({"code": "invalid_url", "message": "names no artifact"}),
    )
    .await;
    let r = read_on(&nothing, "http://localhost:1/settings").await;
    assert_eq!(error_code(&r), "invalid_id");
}

/// Any other failure to resolve a URL is the tool's error: the client does
/// not read the URL itself then.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_resolution_is_the_tools_error() {
    let busy = resolving_daemon(503, json!({"code": "busy", "message": "try again"})).await;
    let r = read_on(&busy, "http://localhost:1/a/k3m9x2a7b8c4").await;
    assert_eq!(error_code(&r), "busy");
    let refused = resolving_daemon(401, json!({"code": "unauthorized", "message": "no"})).await;
    let r = read_on(&refused, "http://localhost:1/a/k3m9x2a7b8c4").await;
    assert_eq!(error_code(&r), "unauthorized");
}
