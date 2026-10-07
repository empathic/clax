//! The tools with a page URL (spec 2026-10-05-chrome-overlay-design §6.2,
//! §9.3): `watch` makes a scope watch, and the comment tools resolve a page
//! URL to its live page.

use clax_core::model::Session;
use clax_mcp::tools::{CommentsReadArgs, WaitArgs, WatchArgs};
use clax_mcp::{ClaxTools, DaemonClient};
use clax_server::testing::{FAKE_PNG, TestServer};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

/// Tools attributed to a fresh `claude` session, and that session's ID.
async fn session_tools(ts: &TestServer) -> (ClaxTools, String) {
    let s: Session = serde_json::from_value(ts.register_session("claude", "live-1").await).unwrap();
    let sid = s.id.clone();
    let tools = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(sid.clone())),
        format!("http://localhost:{}", ts.addr.port()),
        Some(s),
        ts.home.log_path(),
    );
    (tools, sid)
}

fn ok(r: &CallToolResult) -> Value {
    assert!(r.is_error != Some(true), "{r:?}");
    serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap()
}

fn err_code(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    v["error"]["code"].as_str().unwrap().to_string()
}

/// Posts a comment on the page `url` as a named viewer; returns the live
/// page's artifact ID and the thread ID.
async fn comment(ts: &TestServer, url: &str) -> (String, String) {
    let v = ts.viewer(Some("Alex")).await;
    let form = reqwest::multipart::Form::new()
        .text("url", url.to_string())
        .text("title", "Settings")
        .text(
            "anchor",
            json!({"kind": "element", "selector": "body", "file": "index.html"}).to_string(),
        )
        .text("body", "Look")
        .text("pending", "[]")
        .text("snapshot", "<!doctype html><p>x")
        .part(
            "clip",
            reqwest::multipart::Part::bytes(FAKE_PNG.to_vec())
                .mime_str("image/png")
                .unwrap(),
        );
    let res = ts
        .client
        .post(format!("{}/api/live/threads", ts.base))
        .header("cookie", format!("clax_viewer={}", v.cookie))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let c: Value = res.json().await.unwrap();
    (
        c["page"]["artifact_id"].as_str().unwrap().to_string(),
        c["thread"]["id"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn a_watch_names_the_joined_site_and_hears_its_other_addresses() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    comment(&ts, "http://localhost:7702/").await;
    let res = ts
        .post_json(
            "/api/live/sites/join",
            json!({"origin": "http://localhost:7703", "with": "http://localhost:7702"}),
        )
        .await;
    assert_eq!(res.status(), 200);
    let w = ok(&t
        .watch(Parameters(WatchArgs {
            url_or_id: "http://localhost:7703/".into(),
            on: None,
            replies: None,
        }))
        .await
        .unwrap());
    assert_eq!(w["site"]["joined"], true);
    assert_eq!(
        w["site"]["origins"],
        json!(["http://localhost:7703", "http://localhost:7702"])
    );
    let (aid, tid) = comment(&ts, "http://localhost:7702/other").await;
    ts.send_thread(&aid, &tid).await;
    let r = t
        .wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await
        .unwrap();
    let v = ok(&r);
    assert_eq!(v["feedback"][0]["thread_id"], tid.as_str(), "{v}");
}

#[tokio::test]
async fn a_page_url_is_watched_read_and_waited_on() {
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let w = ok(&t
        .watch(Parameters(WatchArgs {
            url_or_id: "http://localhost:5173/".into(),
            on: None,
            replies: None,
        }))
        .await
        .unwrap());
    assert_eq!(w["page_url"], "http://localhost:5173/");
    assert_eq!(w["scope"], "http://localhost:5173/*");
    assert_eq!(w["watching"], true);
    assert_eq!(w["replies_armed"], true);
    assert!(w["artifact_id"].is_string());
    assert!(
        w["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/a/{}", w["artifact_id"].as_str().unwrap()))
    );

    let (aid, tid) = comment(&ts, "http://localhost:5173/settings").await;
    ts.send_thread(&aid, &tid).await;
    let r = t
        .wait_for_feedback(Parameters(WaitArgs {
            url_or_id: Some("http://localhost:5173/settings".into()),
            timeout_s: Some(5),
        }))
        .await
        .unwrap();
    let v = ok(&r);
    let items = v["feedback"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["thread_id"], tid.as_str());
    assert_eq!(
        items[0]["live"]["page_url"],
        "http://localhost:5173/settings"
    );
    let trailing = r.content[1].as_text().unwrap().text.clone();
    assert!(
        trailing.contains("(live page http://localhost:5173/settings; Clax view "),
        "{trailing}"
    );

    let read = ok(&t
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: "http://localhost:5173/settings".into(),
            ..Default::default()
        }))
        .await
        .unwrap());
    assert_eq!(read["artifact_id"], aid.as_str());
    assert_eq!(read["threads"][0]["thread_id"], tid.as_str());
    assert_eq!(
        read["threads"][0]["page_url"],
        "http://localhost:5173/settings"
    );
    let snap = read["threads"][0]["snapshot_path"].as_str().unwrap();
    assert!(snap.ends_with("/versions/1/index.html"), "{snap}");
    assert_eq!(read["threads"][0]["addressed_pending"], false);

    let never = t
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: "http://localhost:5173/never".into(),
            ..Default::default()
        }))
        .await
        .unwrap();
    assert_eq!(err_code(&never), "invalid_id");

    let off = ok(&t
        .watch(Parameters(WatchArgs {
            url_or_id: "http://localhost:5173/".into(),
            on: Some(false),
            replies: None,
        }))
        .await
        .unwrap());
    assert_eq!(off["watching"], false);
    assert_eq!(off["page_url"], "http://localhost:5173/");
    let off = ok(&t
        .watch(Parameters(WatchArgs {
            url_or_id: "http://localhost:5173/settings?tab=b#x".into(),
            on: Some(false),
            replies: None,
        }))
        .await
        .unwrap());
    assert_eq!(
        off["page_url"], "http://localhost:5173/settings",
        "normalized"
    );
}

#[tokio::test]
async fn threads_carry_addressed_pending_and_only_live_pages_the_page_fields() {
    use clax_mcp::tools::CommentsReplyArgs;
    let ts = TestServer::spawn().await;
    let (t, _sid) = session_tools(&ts).await;
    let (aid, tid) = comment(&ts, "http://localhost:5173/a").await;
    ts.send_thread(&aid, &tid).await;
    let replied = ok(&t
        .comments_reply(Parameters(CommentsReplyArgs {
            url_or_id: "http://localhost:5173/a".into(),
            thread_id: tid.clone(),
            text: "Fixed.".into(),
            addressed: Some(true),
        }))
        .await
        .unwrap());
    assert_eq!(replied["replied"], true, "{replied}");
    let read = ok(&t
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid,
            ..Default::default()
        }))
        .await
        .unwrap());
    assert_eq!(read["threads"][0]["addressed_pending"], true);

    let html = ts.publish("Plain", &[("index.html", "<p>x</p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap().to_string();
    ts.thread(&hid, 1, "a note").await;
    let read = ok(&t
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: format!("http://127.0.0.1:{}/a/{hid}", ts.addr.port()),
            ..Default::default()
        }))
        .await
        .unwrap());
    let th = &read["threads"][0];
    assert_eq!(th["addressed_pending"], false);
    assert!(
        th.get("page_url").is_none() && th.get("snapshot_path").is_none(),
        "{th}"
    );
}

/// Tools over a managed client whose daemon is not running when the session
/// starts (its first lookup fails), as the stdio shim builds them.
#[tokio::test]
async fn a_session_started_before_its_daemon_still_tells_clax_urls_from_pages() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let ts = TestServer::spawn().await;
    let up = Arc::new(AtomicBool::new(false));
    let endpoint = clax_mcp::client::Endpoint {
        base: ts.base.clone(),
        browser_base: format!("http://localhost:{}", ts.addr.port()),
        token: ts.token.clone(),
    };
    let find: clax_mcp::client::Refresh = {
        let up = up.clone();
        Arc::new(move || {
            if up.load(Ordering::SeqCst) {
                Ok(endpoint.clone())
            } else {
                Err(anyhow::anyhow!("no clax daemon is running"))
            }
        })
    };
    let reg = clax_core::RegisterSession {
        harness: "claude".into(),
        harness_session_id: Some("late-1".into()),
        cwd: "/work".into(),
        pid: None,
        parent_pid: None,
        transcript_path: None,
    };
    let client = DaemonClient::managed(find.clone(), find, reg);
    assert!(client.ensure_session().await.is_err(), "no daemon yet");
    let tools = ClaxTools::new(client.clone(), String::new(), None, ts.home.log_path());
    up.store(true, Ordering::SeqCst);

    let html = ts.publish("Plain", &[("index.html", "<p>x</p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap().to_string();
    let read = ok(&tools
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: format!("http://localhost:{}/a/{hid}", ts.addr.port()),
            ..Default::default()
        }))
        .await
        .unwrap());
    assert_eq!(read["artifact_id"], hid.as_str(), "{read}");

    let w = ok(&tools
        .watch(Parameters(WatchArgs {
            url_or_id: "http://localhost:5173/".into(),
            on: None,
            replies: None,
        }))
        .await
        .unwrap());
    assert_eq!(w["scope"], "http://localhost:5173/*");
}

/// The same, with the first call a `watch` on a Clax URL.
#[tokio::test]
async fn a_first_watch_on_a_clax_url_watches_the_artifact() {
    let ts = TestServer::spawn().await;
    let endpoint = clax_mcp::client::Endpoint {
        base: ts.base.clone(),
        browser_base: format!("http://localhost:{}", ts.addr.port()),
        token: ts.token.clone(),
    };
    let find: clax_mcp::client::Refresh = std::sync::Arc::new(move || Ok(endpoint.clone()));
    let reg = clax_core::RegisterSession {
        harness: "claude".into(),
        harness_session_id: Some("late-2".into()),
        cwd: "/work".into(),
        pid: None,
        parent_pid: None,
        transcript_path: None,
    };
    let client = DaemonClient::managed(find.clone(), find, reg);
    let tools = ClaxTools::new(client, String::new(), None, ts.home.log_path());
    let html = ts.publish("Plain", &[("index.html", "<p>x</p>")]).await;
    let hid = html["artifact"]["id"].as_str().unwrap().to_string();
    let w = ok(&tools
        .watch(Parameters(WatchArgs {
            url_or_id: format!("http://127.0.0.1:{}/a/{hid}", ts.addr.port()),
            on: None,
            replies: None,
        }))
        .await
        .unwrap());
    assert_eq!(w["artifact_id"], hid.as_str(), "{w}");
    assert!(w.get("scope").is_none());
}
