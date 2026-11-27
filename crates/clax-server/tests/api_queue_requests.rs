//! Queue requests on a local daemon (`clax_server::routes::queue_requests`):
//! an agent of the owner's asks for a report queue, the owner approves or
//! denies it in their browser, and the agent reads the answer, the queue's
//! service grant once. Agents still make no queue themselves. The hosted
//! side (an agent token, a scoped device sign-in) is in `api_hosted.rs`.

use base64::Engine as _;
use clax_testkit::remotes::json_of;
use clax_testkit::{ManualSleeper, TestServer};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// A request of the owner's agent in session `sid` (the daemon token with
/// `X-Clax-Session`).
async fn as_agent(
    ts: &TestServer,
    sid: &str,
    m: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut r = ts
        .authed(ts.client.request(m, format!("{}{path}", ts.base)))
        .header("x-clax-session", sid);
    if let Some(b) = body {
        r = r.json(&b);
    }
    json_of(r.send().await.unwrap()).await
}

/// A request from the owner's browser (its owner cookie).
async fn as_browser(ts: &TestServer, m: reqwest::Method, path: &str) -> (u16, Value) {
    let r = ts
        .client
        .request(m, format!("{}{path}", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({}));
    json_of(r.send().await.unwrap()).await
}

async fn session(ts: &TestServer) -> String {
    ts.register_session("claude", "h-queue").await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A request for a queue named `name` by session `sid`'s agent; its ID.
async fn asked(ts: &TestServer, sid: &str, name: &str) -> String {
    let (s, v) = as_agent(
        ts,
        sid,
        reqwest::Method::POST,
        "/api/queue-requests",
        Some(json!({
            "name": name,
            "origins": ["https://App.Example.com/", "https://app-pr-*.example.com"],
            "note": "for the invoices app",
        })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let r = &v["request"];
    assert_eq!(r["status"], "pending");
    assert_eq!(
        r["origins"],
        json!(["https://app.example.com", "https://app-pr-*.example.com"])
    );
    assert_eq!(r["reporters_see"], "own");
    assert!(r["person"].is_null(), "the owner answers: {v}");
    r["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn an_agent_asks_the_owner_approves_and_the_agent_takes_the_grant_once() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts).await;
    // An agent still makes no queue itself.
    let (s, v) = as_agent(
        &ts,
        &sid,
        reqwest::Method::POST,
        "/api/queues",
        Some(json!({"name": "Mine"})),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");

    let id = asked(&ts, &sid, "Invoices").await;
    // The owner is told in their inbox, on a card of its own.
    let v: Value = ts
        .get_authed("/api/inbox?kind=queue_request&read=all")
        .await
        .json()
        .await
        .unwrap();
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["url"], format!("/inbox?queue_request={id}"));
    assert_eq!(items[0]["queue_request"]["name"], "Invoices");
    assert_eq!(items[0]["read"], false);
    let (s, v) = as_browser(
        &ts,
        reqwest::Method::GET,
        "/api/queue-requests?status=pending",
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["requests"][0]["id"], id.as_str());

    // Pending: nothing to take yet.
    let path = format!("/api/queue-requests/{id}");
    let take = format!("{path}/take");
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::GET, &path, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "pending");
    assert!(v.get("token").is_none(), "{v}");
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &take, Some(json!({}))).await;
    assert_eq!((s, code(&v)), (409, "request_pending"), "{v}");
    assert_eq!(v["error"]["request"]["status"], "pending", "{v}");

    // Only a browser approves: the agent and the token by hand do not.
    let approve = format!("/api/queue-requests/{id}/approve");
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &approve, Some(json!({}))).await;
    assert_eq!((s, code(&v)), (403, "not_for_agents"), "{v}");
    let (s, v) = json_of(
        ts.authed(ts.client.post(format!("{}{approve}", ts.base)))
            .json(&json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "browser_required"), "{v}");

    let (s, v) = as_browser(&ts, reqwest::Method::POST, &approve).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "approved");
    assert_eq!(v["queue"]["role"], "admin");
    let qid = v["queue"]["id"].as_str().unwrap().to_string();
    assert_eq!(v["request"]["queue_id"], qid.as_str());
    let (s, v) = as_browser(&ts, reqwest::Method::POST, &approve).await;
    assert_eq!((s, code(&v)), (409, "request_closed"), "{v}");
    assert_eq!(v["error"]["request"]["status"], "approved", "{v}");

    // Reading the approved request hands nothing over, and takes nothing.
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::GET, &path, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "approved");
    assert!(
        v.get("token").is_none() && v.get("grant_taken").is_none(),
        "{v}"
    );
    // Only the agent takes it: the owner's browser does not.
    let (s, v) = as_browser(&ts, reqwest::Method::POST, &take).await;
    assert_eq!((s, code(&v)), (403, "not_an_agent"), "{v}");

    // The agent's first take hands over the service grant: token and key, once.
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &take, Some(json!({}))).await;
    assert_eq!(s, 200, "{v}");
    let token = v["token"].as_str().expect("the token, once");
    assert!(token.starts_with("cxq_"), "{v}");
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(v["private_key"].as_str().unwrap())
        .unwrap();
    assert!(!key.is_empty());
    assert_eq!(v["grant"]["name"], "Requested by claude");
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &take, Some(json!({}))).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v.get("token").is_none() && v.get("private_key").is_none(),
        "{v}"
    );
    assert_eq!(v["grant_taken"], true);
    let (_, v) = as_agent(&ts, &sid, reqwest::Method::GET, &path, None).await;
    assert_eq!(v["grant_taken"], true, "{v}");
    // The owner's queue, with that one grant; the owner reads the request
    // but never a grant.
    let (_, v) = as_browser(
        &ts,
        reqwest::Method::GET,
        &format!("/api/queues/{qid}/service-grants"),
    )
    .await;
    assert_eq!(v["grants"].as_array().unwrap().len(), 1, "{v}");
    let (s, v) = as_browser(&ts, reqwest::Method::GET, &path).await;
    assert_eq!(s, 200, "{v}");
    assert!(v.get("token").is_none(), "{v}");
    let v: Value = ts
        .get_authed("/api/inbox?kind=queue_request&read=all")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["items"][0]["read"], true,
        "answered, its card is read: {v}"
    );
}

/// `POST pq` to `ts` with service token `token`, signed by `key` now, as
/// `web/sdk`'s `signedFetch` signs it.
async fn signed_post(
    ts: &TestServer,
    key: &clax_core::sig::KeyPair,
    pq: &str,
    token: &str,
    body: &Value,
) -> (u16, Value) {
    use sha2::Digest as _;
    let bytes = serde_json::to_vec(body).unwrap();
    let nonce = clax_core::sig::new_nonce();
    let sig = key
        .sign(&clax_core::sig::SignInput {
            method: "POST",
            path_and_query: pq,
            host: ts.base.strip_prefix("http://").unwrap(),
            t: ts.store.now_utc().timestamp(),
            nonce: &nonce,
            body_sha256: sha2::Sha256::digest(&bytes).into(),
            token_sha256: Some(clax_core::secret::sha256(token)),
            agent_headers_sha256: clax_core::sig::no_agent_headers_sha256(),
        })
        .unwrap()
        .to_string();
    let r = ts
        .client
        .post(format!("{}{pq}", ts.base))
        .header("authorization", format!("Clax-Queue {token}"))
        .header("clax-signature", sig)
        .header("content-type", "application/json")
        .body(bytes);
    json_of(r.send().await.unwrap()).await
}

/// An approved request of session `sid`'s agent: its take path and queue.
async fn approved(ts: &TestServer, sid: &str) -> (String, String) {
    let id = asked(ts, sid, "Invoices").await;
    let (s, v) = as_browser(
        ts,
        reqwest::Method::POST,
        &format!("/api/queue-requests/{id}/approve"),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let qid = v["queue"]["id"].as_str().unwrap().to_string();
    (format!("/api/queue-requests/{id}/take"), qid)
}

#[tokio::test]
async fn a_take_with_its_own_public_key_gets_a_grant_of_that_key_and_no_private_key() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts).await;
    let (take, qid) = approved(&ts, &sid).await;

    // A key that is not an Ed25519 public key, or another field, takes nothing.
    let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 31]);
    for (body, want) in [
        (json!({"public_key": "not a key"}), "invalid_key"),
        (json!({"public_key": short}), "invalid_key"),
        (json!({"public_key": 42}), "invalid_json"),
        (json!({"key": "x"}), "invalid_json"),
    ] {
        let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &take, Some(body.clone())).await;
        assert_eq!((s, code(&v)), (400, want), "{body}: {v}");
    }

    // A valid key still takes the grant, once, with no private key.
    let mine = clax_core::sig::KeyPair::generate();
    let public = clax_core::sig::encode_public(&mine.public());
    let (s, v) = as_agent(
        &ts,
        &sid,
        reqwest::Method::POST,
        &take,
        Some(json!({"public_key": public})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v.get("private_key").is_none(),
        "no private_key field at all: {v}"
    );
    let token = v["token"].as_str().expect("the token, once").to_string();
    assert!(token.starts_with("cxq_"), "{v}");
    assert_eq!(v["grant"]["name"], "Requested by claude");
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &take, Some(json!({}))).await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v.get("token").is_none() && v.get("private_key").is_none(),
        "{v}"
    );
    assert_eq!(v["grant_taken"], true);

    // The grant verifies the agent's own key, and no other.
    let pq = format!("/api/queues/{qid}/reporter-tokens");
    let reporter = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    let body = json!({"reporter_id": "emp-1", "origin": "https://app.example.com",
                      "public_key": reporter});
    let other = clax_core::sig::KeyPair::generate();
    let (s, v) = signed_post(&ts, &other, &pq, &token, &body).await;
    assert_eq!((s, code(&v)), (401, "invalid_signature"), "{v}");
    let (s, v) = signed_post(&ts, &mine, &pq, &token, &body).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["token"].as_str().unwrap().starts_with("cxr_"), "{v}");
}

#[tokio::test]
async fn a_take_with_no_body_hands_over_the_pair_it_made() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts).await;
    let (take, qid) = approved(&ts, &sid).await;
    let (s, v) = as_agent(&ts, &sid, reqwest::Method::POST, &take, None).await;
    assert_eq!(s, 200, "{v}");
    let token = v["token"].as_str().expect("the token, once").to_string();
    let pkcs8 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(v["private_key"].as_str().expect("the private key, once"))
        .unwrap();
    let key = clax_core::sig::KeyPair::from_pkcs8(&pkcs8).unwrap();
    let pq = format!("/api/queues/{qid}/reporter-tokens");
    let reporter = clax_core::sig::encode_public(&clax_core::sig::KeyPair::generate().public());
    let body = json!({"reporter_id": "emp-1", "origin": "https://app.example.com",
                      "public_key": reporter});
    let (s, v) = signed_post(&ts, &key, &pq, &token, &body).await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn a_denied_request_makes_no_queue() {
    let ts = TestServer::spawn().await;
    let sid = session(&ts).await;
    let id = asked(&ts, &sid, "Invoices").await;
    let (s, v) = as_browser(
        &ts,
        reqwest::Method::POST,
        &format!("/api/queue-requests/{id}/deny"),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "denied");
    let (s, v) = as_agent(
        &ts,
        &sid,
        reqwest::Method::GET,
        &format!("/api/queue-requests/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "denied");
    assert!(v.get("token").is_none(), "{v}");
    let (s, v) = as_agent(
        &ts,
        &sid,
        reqwest::Method::POST,
        &format!("/api/queue-requests/{id}/take"),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "denied");
    assert!(v.get("token").is_none(), "{v}");
    let (_, v) = as_browser(&ts, reqwest::Method::GET, "/api/queues").await;
    assert_eq!(v["queues"], json!([]));
}

#[tokio::test]
async fn a_person_makes_queues_and_asks_for_none() {
    let ts = TestServer::spawn().await;
    // The owner by hand and in their browser make queues; asking is an
    // agent's.
    let (s, v) = json_of(
        ts.authed(ts.client.post(format!("{}/api/queue-requests", ts.base)))
            .json(&json!({"name": "Q"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((s, code(&v)), (403, "not_an_agent"), "{v}");
    let (s, v) = json_of(
        ts.client
            .post(format!("{}/api/queue-requests", ts.base))
            .json(&json!({"name": "Q"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s, 401, "{v}");
    // Bad fields are refused as `POST /api/queues` refuses them.
    let sid = session(&ts).await;
    for (body, want) in [
        (json!({"name": " "}), "invalid_name"),
        (
            json!({"name": "Q", "origins": ["ftp://x"]}),
            "invalid_origin",
        ),
        (
            json!({"name": "Q", "note": "x".repeat(501)}),
            "invalid_args",
        ),
    ] {
        let (s, v) = as_agent(
            &ts,
            &sid,
            reqwest::Method::POST,
            "/api/queue-requests",
            Some(body),
        )
        .await;
        assert_eq!((s, code(&v)), (400, want), "{v}");
    }
    // Another caller's request is no one's to read: 404 for a made-up ID.
    let (s, _) = as_agent(
        &ts,
        &sid,
        reqwest::Method::GET,
        "/api/queue-requests/01J9Q0000000000000000000RR",
        None,
    )
    .await;
    assert_eq!(s, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_read_answers_as_soon_as_the_owner_approves() {
    let (ts, clock): (TestServer, Arc<ManualSleeper>) = TestServer::spawn_question_clock().await;
    let sid = session(&ts).await;
    let id = asked(&ts, &sid, "Invoices").await;
    let path = format!("/api/queue-requests/{id}?wait=60");
    let waiting = {
        let r = ts
            .authed(ts.client.get(format!("{}{path}", ts.base)))
            .header("x-clax-session", &sid);
        tokio::spawn(async move { json_of(r.send().await.unwrap()).await })
    };
    clock.wait_for(Duration::from_secs(60), 1).await;
    let (s, _) = as_browser(
        &ts,
        reqwest::Method::POST,
        &format!("/api/queue-requests/{id}/approve"),
    )
    .await;
    assert_eq!(s, 200);
    let (s, v) = waiting.await.unwrap();
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "approved");
    assert!(v.get("token").is_none(), "a read hands nothing over: {v}");

    // A wait that runs out answers pending.
    let id = asked(&ts, &sid, "Second").await;
    let waiting = {
        let r = ts
            .authed(
                ts.client
                    .get(format!("{}/api/queue-requests/{id}?wait=30", ts.base)),
            )
            .header("x-clax-session", &sid);
        tokio::spawn(async move { json_of(r.send().await.unwrap()).await })
    };
    clock.wait_for(Duration::from_secs(30), 1).await;
    assert_eq!(clock.fire(Duration::from_secs(30)), 1);
    let (s, v) = waiting.await.unwrap();
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["status"], "pending");
}

/// `queue_request`'s result for `args` from `tools`, and whether it is an
/// error.
async fn tool(tools: &clax_mcp::tools::ClaxTools, args: Value) -> (bool, Value) {
    let r = tools
        .call_direct(
            rmcp::model::CallToolRequestParams::new("queue_request")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let rmcp::model::CallToolResponse::Complete(r) = r else {
        panic!("queue_request completes")
    };
    let r = serde_json::to_value(r).unwrap();
    let text = r["content"][0]["text"].as_str().unwrap().to_string();
    (r["isError"] == true, serde_json::from_str(&text).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_queue_request_tool_waits_for_approval_and_writes_the_grant_to_write_env_alone() {
    let (ts, clock): (TestServer, Arc<ManualSleeper>) = TestServer::spawn_question_clock().await;
    let sid = session(&ts).await;
    let dir = tempfile::tempdir().unwrap();
    let tools = clax_mcp::tools::ClaxTools::new(
        clax_mcp::client::DaemonClient::new(ts.base.clone(), ts.token.clone(), Some(sid)),
        ts.base.clone(),
        None,
        dir.path().join("mcp.log"),
    )
    .with_opener(clax_mcp::tools::Opener::Off);

    // A relative path, or one it cannot append to, is refused before
    // anything is asked.
    for bad in [
        "app/.env".to_string(),
        dir.path().join("no-such-dir/.env").display().to_string(),
    ] {
        let (err, v) = tool(
            &tools,
            json!({"name": "Invoices", "origins": ["https://app.example.com"], "write_env": bad}),
        )
        .await;
        assert!(err, "{v}");
    }
    // On another machine's Clax the tools write no file of its.
    let (err, v) = tool(
        &tools.for_remote_agent(),
        json!({"name": "Invoices", "origins": ["https://app.example.com"], "write_env": dir.path().join(".env")}),
    )
    .await;
    assert!(err, "{v}");
    let (_, v) = as_browser(&ts, reqwest::Method::GET, "/api/queue-requests").await;
    assert_eq!(v["requests"], json!([]), "nothing was asked: {v}");

    let env = dir.path().join(".env");
    std::fs::write(&env, "PORT=3000").unwrap();
    let call = {
        let (tools, env) = (tools.clone(), env.clone());
        tokio::spawn(async move {
            tool(
                &tools,
                json!({
                    "name": "Invoices",
                    "origins": ["https://app.example.com"],
                    "note": "for the invoices app",
                    "timeout_s": 30,
                    "write_env": env,
                }),
            )
            .await
        })
    };
    // The tool waits on its request (30 s, within every harness's longest
    // wait); the owner approves it.
    clock.wait_for(Duration::from_secs(30), 1).await;
    let (_, v) = as_browser(
        &ts,
        reqwest::Method::GET,
        "/api/queue-requests?status=pending",
    )
    .await;
    let id = v["requests"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(v["requests"][0]["note"], "for the invoices app");
    let (s, v) = as_browser(
        &ts,
        reqwest::Method::POST,
        &format!("/api/queue-requests/{id}/approve"),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let qid = v["queue"]["id"].as_str().unwrap().to_string();

    let (err, v) = call.await.unwrap();
    assert!(!err, "{v}");
    assert_eq!(v["status"], "approved", "{v}");
    assert_eq!(v["call_again"], false);
    assert_eq!(v["wrote"], env.display().to_string());
    assert_eq!(
        v["variables"],
        json!(["CLAX_QUEUE", "CLAX_QUEUE_TOKEN", "CLAX_QUEUE_KEY"])
    );
    for k in ["token", "private_key", "secret"] {
        assert!(v.get(k).is_none(), "{k} stays out of the result: {v}");
    }
    let text = v.to_string();
    assert!(!text.contains("cxq_"), "no token in the result: {v}");
    let written = std::fs::read_to_string(&env).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 4, "{written}");
    assert_eq!(lines[0], "PORT=3000");
    assert_eq!(lines[1], format!("CLAX_QUEUE={qid}"));
    let token = lines[2].strip_prefix("CLAX_QUEUE_TOKEN=").unwrap();
    assert!(token.starts_with("cxq_"), "{written}");
    let key = lines[3].strip_prefix("CLAX_QUEUE_KEY=").unwrap();
    assert!(!key.is_empty() && !text.contains(key), "{written}");

    // Called again, the grant was taken: nothing secret, nothing written.
    let (err, v) = tool(
        &tools,
        json!({"request_id": id, "timeout_s": 1, "write_env": env}),
    )
    .await;
    assert!(!err, "{v}");
    assert_eq!(v["grant_taken"], true, "{v}");
    assert!(v.get("token").is_none() && v.get("wrote").is_none(), "{v}");
    assert_eq!(std::fs::read_to_string(&env).unwrap(), written);
}
