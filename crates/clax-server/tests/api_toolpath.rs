//! The audit context each request resolves (spec
//! 2026-10-06-toolpath-audit-design §5.2, §6, §6.9), read back through the
//! test-only route `GET /api/_test/audit/ctx`, and the appender's nudge.
use crate::common;
use clax_core::audit::{CallHeader, encode_call_header};
use clax_core::gitctx::{GitContext, GitField, encode_header};
use common::TestServer;
use serde_json::{Value, json};

const CTX: &str = "/api/_test/audit/ctx";

fn git() -> GitContext {
    GitContext {
        repo_root: "/Users/alex/work/app".into(),
        remote: Some("origin".into()),
        remote_url: Some("https://github.com/empathic/app.git".into()),
        branch: Some("feat/settings".into()),
        head: Some("9c1e5d2b0a7f4e3c8d6b1a2f3e4d5c6b7a8f9e0d".into()),
        dirty: false,
        diff_sha256: None,
        diff_bytes: None,
        diff_truncated: false,
        untracked: 0,
        captured_at: "2026-10-06T14:03:11.512Z".into(),
    }
}

fn call() -> CallHeader {
    CallHeader {
        call_id: "01JBC0000000000000000000AA".into(),
        tool: "publish".into(),
        harness_tool: Some("mcp__clax__publish".into()),
        args_sha256: format!("sha256:{}", "c6".repeat(32)),
        started_at: "2026-10-06T14:03:11.402Z".into(),
        harness_call_id: None,
    }
}

async fn ctx(req: reqwest::RequestBuilder) -> Value {
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

#[tokio::test]
async fn ctx_owner_cookie_is_owner_shell() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_public_id().await;
    let url = format!("{}{CTX}", ts.base);
    let v = ctx(ts.client.get(&url).header("cookie", ts.owner_cookie())).await;
    assert_eq!(v["actor"], json!({"type": "owner", "public_id": owner}));
    assert_eq!(v["via"], "shell");
    assert_eq!(
        (&v["git"], &v["git_capture"], &v["call"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    // The token alone is the owner through the CLI; it may name its channel.
    let v = ctx(ts.authed(ts.client.get(&url))).await;
    assert_eq!(v["actor"], json!({"type": "owner", "public_id": owner}));
    assert_eq!(v["via"], "cli");
    // A browser of the owner's (where a page's own requests carry the
    // owner cookie too) is the owner through the shell, whatever channel,
    // session, git state or tool call it claims.
    let session = ts.register_session("claude", "hs-owner").await;
    let v = ctx(ts
        .client
        .get(&url)
        .header("cookie", ts.owner_cookie())
        .header("x-clax-via", "mcp")
        .header("x-clax-session", session["id"].as_str().unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .header("x-clax-call", encode_call_header(&call()).unwrap()))
    .await;
    assert_eq!(v["actor"], json!({"type": "owner", "public_id": owner}));
    assert_eq!(v["via"], "shell");
    assert_eq!(
        (&v["git"], &v["git_capture"], &v["call"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    // The page publishing through the shell names `page`, which is no channel.
    let v = ctx(ts.authed(ts.client.get(&url)).header("x-clax-via", "page")).await;
    assert_eq!(v["via"], "cli");
}

#[tokio::test]
async fn ctx_lan_viewer_has_public_id_and_name() {
    let ts = TestServer::spawn().await;
    let sam = ts.viewer(Some("Sam")).await;
    let (lan, base) = ts.lan();
    let url = format!("{base}{CTX}");
    let v = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={}", sam.cookie)))
    .await;
    assert_eq!(
        v["actor"],
        json!({"type": "viewer", "public_id": sam.public_id, "display_name": "Sam"})
    );
    assert_eq!(v["via"], "lan");
    // A viewer's claims of a channel, a session, git state or a tool call
    // are not believed.
    let session = ts.register_session("claude", "hs-1").await;
    let v = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={}", sam.cookie))
        .header("x-clax-via", "mcp")
        .header("x-clax-session", session["id"].as_str().unwrap())
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .header("x-clax-call", encode_call_header(&call()).unwrap()))
    .await;
    assert_eq!(v["actor"]["type"], "viewer");
    assert_eq!(v["via"], "lan");
    assert_eq!(
        (&v["git"], &v["git_capture"], &v["call"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    // A first-time viewer is recorded under the public ID they are given.
    let fresh = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
    let v = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={fresh}")))
    .await;
    assert_eq!(v["actor"]["type"], "viewer");
    let pid = v["actor"]["public_id"].as_str().unwrap().to_string();
    assert!(clax_core::is_public_id(&pid), "{v}");
    let again = ctx(lan
        .get(&url)
        .header("cookie", format!("clax_viewer={fresh}")))
    .await;
    assert_eq!(again["actor"]["public_id"], pid.as_str());
    // No cookie: anonymous.
    let v = ctx(lan.get(&url)).await;
    assert_eq!(v["actor"], json!({"type": "anonymous"}));
    assert_eq!(v["via"], "lan");
}

#[tokio::test]
async fn ctx_session_header_is_agent_with_harness_ids() {
    let ts = TestServer::spawn().await;
    let session = ts.register_session("claude", "3f2c-hs").await;
    let sid = session["id"].as_str().unwrap().to_string();
    let handle = session["agent_handle"].as_str().unwrap().to_string();
    let transcript = "/Users/alex/.claude/projects/app/3f2c-hs.jsonl";
    rusqlite::Connection::open(ts.home.db_path())
        .unwrap()
        .execute(
            "UPDATE sessions SET transcript_path = ?1 WHERE id = ?2",
            rusqlite::params![transcript, sid],
        )
        .unwrap();
    let url = format!("{}{CTX}", ts.base);
    let v = ctx(ts
        .authed(ts.client.get(&url))
        .header("x-clax-session", &sid)
        .header("x-clax-git", encode_header(&GitField::Ok(git())).unwrap())
        .header("x-clax-call", encode_call_header(&call()).unwrap()))
    .await;
    assert_eq!(
        v["actor"],
        json!({"type": "agent", "session_id": sid, "harness": "claude",
               "harness_session_id": "3f2c-hs", "agent_handle": handle,
               "transcript_path": transcript})
    );
    // A session without a channel header came through the MCP shim.
    assert_eq!(v["via"], "mcp");
    assert_eq!(v["git"], serde_json::to_value(git()).unwrap());
    assert_eq!(v["git_capture"], "ok");
    assert_eq!(v["call"], serde_json::to_value(call()).unwrap());
    // The agent side names its channel, and why it has no git context.
    let v = ctx(ts
        .authed(ts.client.get(&url))
        .header("x-clax-session", &sid)
        .header("x-clax-via", "pi")
        .header(
            "x-clax-git",
            encode_header(&GitField::Capture("not-a-repo")).unwrap(),
        ))
    .await;
    assert_eq!(
        (v["actor"]["type"].as_str(), v["via"].as_str()),
        (Some("agent"), Some("pi"))
    );
    assert_eq!(
        (&v["git"], &v["git_capture"]),
        (&Value::Null, &json!("not-a-repo"))
    );
    // An ended session still names the agent that acted.
    let res = ts
        .authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    let v = ctx(ts
        .authed(ts.client.get(&url))
        .header("x-clax-session", &sid))
    .await;
    assert_eq!(v["actor"]["session_id"], sid.as_str());
    // `mcp` without a session is the sessionless `/mcp` route.
    let v = ctx(ts.authed(ts.client.get(&url)).header("x-clax-via", "mcp")).await;
    assert_eq!(v["actor"], json!({"type": "agent", "session_id": null}));
    assert_eq!(v["via"], "mcp");
    // `hook` or `pi` without a known session is the owner, on that channel.
    let owner = ts.owner_public_id().await;
    for via in ["hook", "pi"] {
        for session in [None, Some("01JB0000000000000000000000")] {
            let mut req = ts.authed(ts.client.get(&url)).header("x-clax-via", via);
            if let Some(s) = session {
                req = req.header("x-clax-session", s);
            }
            let v = ctx(req).await;
            assert_eq!(
                v["actor"],
                json!({"type": "owner", "public_id": owner}),
                "{via}"
            );
            assert_eq!(v["via"], via);
        }
    }
}

#[tokio::test]
async fn ctx_bad_headers_are_invalid_not_errors() {
    let ts = TestServer::spawn().await;
    let owner = ts.owner_public_id().await;
    let url = format!("{}{CTX}", ts.base);
    let bad = reqwest::header::HeaderValue::from_bytes(b"\xff\xfe").unwrap();
    let mut bad_call = call();
    bad_call.call_id = "nope".into();
    let bad_call = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&bad_call).unwrap())
    };
    for (git, call, via, session) in [
        (
            "!!not base64!!".parse().unwrap(),
            "!!".parse().unwrap(),
            "bogus".parse().unwrap(),
            "not-a-ulid".parse().unwrap(),
        ),
        (bad.clone(), bad.clone(), bad.clone(), bad.clone()),
        (
            "A".repeat(4096).parse().unwrap(),
            bad_call.parse().unwrap(),
            "daemon".parse().unwrap(),
            "01JB0000000000000000000000".parse().unwrap(),
        ),
    ] {
        let v = ctx(ts
            .authed(ts.client.get(&url))
            .header("x-clax-git", git)
            .header("x-clax-call", call)
            .header("x-clax-via", via)
            .header("x-clax-session", session))
        .await;
        assert_eq!(
            v["actor"],
            json!({"type": "owner", "public_id": owner}),
            "{v}"
        );
        assert_eq!(v["via"], "cli", "{v}");
        assert_eq!(
            (&v["git"], &v["git_capture"]),
            (&Value::Null, &json!("invalid")),
            "{v}"
        );
        assert_eq!(v["call"], Value::Null, "{v}");
    }
}

#[test]
fn nudge_never_blocks() {
    let wake = clax_server::audit::AuditWake::new();
    let rx = wake.take_receiver().expect("the appender's receiver");
    assert!(wake.take_receiver().is_none(), "one appender");
    // Nobody drains the channel: every nudge past the first finds it full.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let w = wake.clone();
    std::thread::spawn(move || {
        for _ in 0..100_000 {
            w.nudge();
        }
        done_tx.send(()).unwrap();
    });
    done_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("100 000 nudges return without a reader");
    // They coalesce into one wake-up.
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
    // With the appender gone, a nudge still returns.
    drop(rx);
    wake.nudge();
}
