//! The tools for multiple agents and people (spec 2026-10-08 §16): an
//! agent names itself with `status`, asks its person to see withheld
//! content with `ask`'s `approval`, and hands a thread to another agent
//! with `comments_handoff`.

use clax_mcp::tools::{
    ApprovalArg, AskArgs, CommentsHandoffArgs, CommentsReadArgs, StatusArgs, WaitArgs,
};
use clax_mcp::{ClaxTools, DaemonClient};
use clax_server::testing::TestServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

/// Tools over a managed client registering a `harness` session with
/// harness session ID `hsid`, as the stdio shim does.
fn agent(ts: &TestServer, harness: &str, hsid: &str) -> ClaxTools {
    let endpoint = clax_mcp::client::Endpoint {
        base: ts.base.clone(),
        browser_base: format!("http://localhost:{}", ts.addr.port()),
        token: ts.token.clone(),
    };
    let find: clax_mcp::client::Refresh = std::sync::Arc::new(move || Ok(endpoint.clone()));
    let client = DaemonClient::managed(
        find.clone(),
        find,
        clax_core::RegisterSession {
            harness: harness.into(),
            harness_session_id: Some(hsid.into()),
            cwd: "/work".into(),
            ..Default::default()
        },
    );
    ClaxTools::new(client, String::new(), None, ts.home.log_path())
}

/// The JSON object a result carries, and whether it is an error.
fn body(r: Result<CallToolResult, rmcp::ErrorData>) -> (Value, bool) {
    let r = r.expect("a tool result");
    let v = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
    (v, r.is_error == Some(true))
}

fn ok(r: Result<CallToolResult, rmcp::ErrorData>) -> Value {
    let (v, error) = body(r);
    assert!(!error, "{v}");
    v
}

fn code(r: Result<CallToolResult, rmcp::ErrorData>) -> String {
    let (v, error) = body(r);
    assert!(error, "{v}");
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

fn named(name: &str) -> Parameters<StatusArgs> {
    Parameters(StatusArgs {
        name: Some(name.into()),
    })
}

#[tokio::test]
async fn status_sets_and_reports_the_name() {
    let ts = TestServer::spawn().await;
    let a = agent(&ts, "claude", "cc-1");
    let s = ok(a.status(Parameters(StatusArgs::default())).await);
    assert_eq!(s["name"], "claude", "a session with no repository: {s}");
    assert!(s.get("remote").is_none(), "{s}");
    assert_eq!(s["person"]["owner"], true, "{s}");
    assert_eq!(
        s["rights"],
        clax_core::rights::Rights::full().to_json(),
        "{s}"
    );
    let s = ok(a.status(named("fix-chart-axis")).await);
    assert_eq!(s["name"], "fix-chart-axis", "{s}");
    assert_eq!(s["session"]["name"], "fix-chart-axis", "{s}");
    let b = agent(&ts, "codex", "cx-1");
    assert_eq!(code(b.status(named("fix-chart-axis")).await), "name_taken");
    assert_eq!(code(b.status(named("bad name-")).await), "invalid_name");
    // A name its person gave it is theirs.
    let sid = ok(b.status(Parameters(StatusArgs::default())).await)["session"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .authed(ts.client.get(format!("{}/api/sessions/{sid}", ts.base)))
        .send()
        .await
        .unwrap();
    let handle = r.json::<Value>().await.unwrap()["session"]["agent_handle"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ts
        .authed(ts.client.put(format!("{}/api/agents/{handle}", ts.base)))
        .json(&json!({"name": "reviewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert_eq!(code(b.status(named("mine-now")).await), "name_locked");
}

/// A thread Mia, a connected person, starts on the owner's artifact in her
/// browser, sent to the agents; the artifact's ID and the thread's.
async fn mias_thread(ts: &TestServer, aid: &str) -> String {
    let mia = ts.connect_person("Mia", "clax:collaborator").await;
    let cookie = ts.browser_cookie(&ts.store.viewer_of_grant(&mia.grant_id()).unwrap().id);
    let t = ts.thread_as(aid, &cookie, "MIACANARY words").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.send_thread(aid, &tid).await;
    tid
}

#[tokio::test]
async fn ask_with_approval_returns_the_decision() {
    let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), |_| {}).await;
    let a = agent(&ts, "claude", "cc-1");
    let s = ok(a.status(Parameters(StatusArgs::default())).await);
    let sid = s["session"]["id"].as_str().unwrap().to_string();
    let aid = ts.publish_as(&sid, "Plan", "<h2>Plan</h2>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = mias_thread(&ts, &aid).await;
    // The owner has not approved Mia: her words are withheld from the
    // owner's agent.
    let read = ok(a
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid.clone(),
            thread_id: Some(tid.clone()),
            ..Default::default()
        }))
        .await);
    assert_eq!(
        read["threads"][0]["comments"][0]["withheld"], true,
        "{read}"
    );
    assert!(!read.to_string().contains("MIACANARY"), "{read}");
    let ask = |approval: Option<ApprovalArg>, approval_id: Option<String>, timeout_s| AskArgs {
        approval,
        approval_id,
        timeout_s: Some(timeout_s),
        ..Default::default()
    };
    let asked = ok(a
        .ask(Parameters(ask(
            Some(ApprovalArg {
                url_or_id: Some(aid.clone()),
                thread_id: Some(tid.clone()),
                author: None,
            }),
            None,
            1,
        )))
        .await);
    assert_eq!(
        (&asked["status"], &asked["call_again"]),
        (&json!("open"), &json!(true)),
        "{asked}"
    );
    let rid = asked["approval_id"].as_str().unwrap().to_string();
    // The owner decides in a browser while the agent waits again.
    let waiting = a.ask(Parameters(ask(None, Some(rid.clone()), 600)));
    let decide = async {
        ts.wait_question_waiters(&rid, 1).await;
        let r = ts
            .owner_browser()
            .post(
                &format!("/api/approvals/requests/{rid}/decide"),
                json!({"scope": "thread"}),
            )
            .await;
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    };
    let (decided, ()) = tokio::join!(waiting, decide);
    let decided = ok(decided);
    assert_eq!(
        (
            &decided["status"],
            &decided["scope"],
            &decided["call_again"]
        ),
        (&json!("approved"), &json!("thread"), &json!(false)),
        "{decided}"
    );
    let read = ok(a
        .comments_read(Parameters(CommentsReadArgs {
            url_or_id: aid.clone(),
            thread_id: Some(tid.clone()),
            ..Default::default()
        }))
        .await);
    assert_eq!(
        read["threads"][0]["comments"][0]["body"], "MIACANARY words",
        "{read}"
    );
    // Nothing is withheld there now.
    let again = a
        .ask(Parameters(ask(
            Some(ApprovalArg {
                url_or_id: None,
                thread_id: Some(tid.clone()),
                author: None,
            }),
            None,
            1,
        )))
        .await;
    assert_eq!(code(again), "nothing_withheld");
    // Questions and an approval are never asked together.
    let both = a
        .ask(Parameters(AskArgs {
            question_id: Some(rid.clone()),
            approval_id: Some(rid),
            ..Default::default()
        }))
        .await;
    assert_eq!(code(both), "invalid_args");
}

#[tokio::test]
async fn comments_handoff_moves_and_releases() {
    let ts = TestServer::spawn().await;
    let a = agent(&ts, "claude", "cc-1");
    let b = agent(&ts, "codex", "cx-1");
    let sa = ok(a.status(named("web-a")).await);
    ok(b.status(named("web-b")).await);
    let sid = sa["session"]["id"].as_str().unwrap().to_string();
    let aid = ts.publish_as(&sid, "Plan", "<h2>Plan</h2>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tid = ts.thread_as_owner(&aid, "fix the axis").await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Agent a takes the thread by being handed it.
    let got = ok(a
        .wait_for_feedback(Parameters(WaitArgs {
            url_or_id: None,
            timeout_s: Some(1),
        }))
        .await);
    assert_eq!(got["feedback"][0]["thread_id"], json!(tid), "{got}");
    let hand = |to: Option<&str>| {
        Parameters(CommentsHandoffArgs {
            url_or_id: aid.clone(),
            thread_id: tid.clone(),
            to: to.map(str::to_string),
        })
    };
    // A thread named with another artifact is not found there.
    let other = ts.publish_as(&sid, "Other", "<h2>Other</h2>").await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let wrong = a
        .comments_handoff(Parameters(CommentsHandoffArgs {
            url_or_id: other,
            thread_id: tid.clone(),
            to: Some("web-b".into()),
        }))
        .await;
    assert_eq!(code(wrong), "not_found");
    let moved = ok(a.comments_handoff(hand(Some("web-b"))).await);
    assert_eq!(moved["thread"]["owner"]["name"], "web-b", "{moved}");
    assert_eq!(moved["released"], false);
    // Agent a no longer owns it.
    assert_eq!(
        code(a.comments_handoff(hand(None)).await),
        "thread_taken",
        "only the owner releases"
    );
    let released = ok(b.comments_handoff(hand(None)).await);
    assert_eq!(released["released"], true);
    assert!(released["thread"]["owner"].is_null(), "{released}");
}
