//! A decision the daemon holds for the owner's browser while an agent is
//! live (`clax_core::confirm`): the tools' client hands the 202 back as
//! `confirmation_pending` for an agent to wait on, and the CLI's tool
//! commands' client waits for the owner's answer and makes the call again.

use clax_mcp::client::{ClientError, DaemonClient};
use clax_testkit::TestServer;
use serde_json::json;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// Where [`told`] sends each line it is given.
static TOLD: Mutex<Option<UnboundedSender<String>>> = Mutex::new(None);

fn told(line: &str) {
    if let Some(tx) = TOLD.lock().unwrap().as_ref() {
        let _ = tx.send(line.to_string());
    }
}

/// A page, and an agent session live on `ts`.
async fn page_with_an_agent_live(ts: &TestServer) -> String {
    let aid = ts.publish("Held", &[("index.html", "<h1>hi</h1>")]).await["artifact"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ts.register_session("claude", "conf-1").await;
    aid
}

#[tokio::test]
async fn a_held_decision_comes_back_as_confirmation_pending() {
    let ts = TestServer::spawn().await;
    let aid = page_with_an_agent_live(&ts).await;
    let by_hand = DaemonClient::with_channel(ts.base.clone(), ts.token.clone(), None, false);
    let err = by_hand
        .patch(&aid, &json!({"site_access": "world"}))
        .await
        .unwrap_err();
    let ClientError::Api { status, error } = err else {
        panic!("{err}");
    };
    assert_eq!(status, 202);
    assert_eq!(error["code"], "confirmation_pending");
    let url = error["confirm_url"].as_str().unwrap();
    assert_eq!(error["message"], format!("Confirm in your browser: {url}"));
    assert!(
        error["confirmation_id"]
            .as_str()
            .unwrap()
            .starts_with("cf_")
    );
    assert_ne!(
        ts.site_get_anon(&aid, "/").await.status(),
        200,
        "nothing changed"
    );
}

#[tokio::test]
async fn the_clis_tool_client_waits_for_the_owner_and_makes_the_call_again() {
    let ts = TestServer::spawn().await;
    let aid = page_with_an_agent_live(&ts).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *TOLD.lock().unwrap() = Some(tx);
    let waiting = DaemonClient::with_channel(ts.base.clone(), ts.token.clone(), None, false)
        .waiting_for_confirmations(told);
    let call = {
        let aid = aid.clone();
        tokio::spawn(async move { waiting.patch(&aid, &json!({"site_access": "world"})).await })
    };
    let line = rx.recv().await.unwrap();
    let id = line
        .rsplit_once("confirm=")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("{line}"));
    assert!(
        line.starts_with("Confirm in your browser: http://"),
        "{line}"
    );
    ts.confirm(&id, true).await;
    let v = call.await.unwrap().unwrap();
    assert_eq!(v["artifact"]["site_access"], "world", "{v}");
    assert_eq!(ts.site_get_anon(&aid, "/").await.status(), 200);
    assert!(rx.try_recv().is_err(), "one line");
}
