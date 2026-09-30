mod common;
use common::TestServer;
use serde_json::{Value, json};
use std::net::{IpAddr, UdpSocket};

const ACCEPT: &str = "application/json, text/event-stream";

fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "api-mcp-test", "version": "0"}
        }
    })
}

fn mcp_post(ts: &TestServer, body: &Value) -> reqwest::RequestBuilder {
    ts.client
        .post(format!("{}/mcp", ts.base))
        .header("accept", ACCEPT)
        .json(body)
}

/// The JSON-RPC message in a response that is either plain JSON or an SSE stream.
async fn rpc_message(res: reqwest::Response) -> Value {
    let text = res.text().await.unwrap();
    if let Ok(v) = serde_json::from_str(&text) {
        return v;
    }
    text.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v.get("id").is_some())
        .unwrap_or_else(|| panic!("no JSON-RPC response in {text:?}"))
}

#[tokio::test]
async fn mcp_requires_the_bearer_token() {
    let ts = TestServer::spawn().await;
    let res = mcp_post(&ts, &initialize()).send().await.unwrap();
    assert_eq!(res.status(), 401);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");

    let res = mcp_post(&ts, &initialize())
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn mcp_refuses_a_non_local_host() {
    let ts = TestServer::spawn().await;
    let res = mcp_post(&ts, &initialize())
        .bearer_auth(&ts.token)
        .header("host", "evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn mcp_lists_the_twenty_two_tools() {
    let ts = TestServer::spawn().await;
    let res = mcp_post(&ts, &initialize())
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let session = res
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    let init = rpc_message(res).await;
    assert!(init["result"]["instructions"].is_string(), "{init}");

    let with_session = |req: reqwest::RequestBuilder| match &session {
        Some(s) => req.header("mcp-session-id", s),
        None => req,
    };
    let res = with_session(
        mcp_post(
            &ts,
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .bearer_auth(&ts.token),
    )
    .send()
    .await
    .unwrap();
    assert!(res.status().is_success(), "{}", res.status());

    let res = with_session(
        mcp_post(
            &ts,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )
        .bearer_auth(&ts.token),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let list = rpc_message(res).await;
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{list}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "asset_upload",
            "comments_read",
            "comments_reply",
            "comments_resolve",
            "db_batch",
            "db_delete",
            "db_get",
            "db_list",
            "db_query",
            "db_set",
            "db_str_replace",
            "db_update",
            "delete",
            "list",
            "open",
            "pin",
            "publish",
            "read",
            "status",
            "unpin",
            "wait_for_feedback",
            "watch"
        ]
    );

    // A tool call goes through the daemon's own REST API.
    let res = with_session(
        mcp_post(
            &ts,
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                "name": "publish",
                "arguments": {"html": "<p>via mcp</p>", "title": "Via MCP"}
            }}),
        )
        .bearer_auth(&ts.token),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let call = rpc_message(res).await;
    assert_eq!(call["result"]["isError"], false, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let published: Value = serde_json::from_str(text).unwrap();
    let id = published["artifact_id"].as_str().unwrap();
    assert_eq!(
        published["url"],
        format!("http://localhost:{}/a/{id}", ts.addr.port())
    );
    let listed: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(listed["artifacts"][0]["title"], "Via MCP");
}

#[tokio::test]
async fn raw_file_route_serves_unwrapped_bytes_sandboxed() {
    let ts = TestServer::spawn().await;
    let v = ts
        .publish("Raw", &[("index.html", "<h1>raw</h1>"), ("a.css", "p{}")])
        .await;
    let id = v["artifact"]["id"].as_str().unwrap();
    let res = ts
        .get(&format!("/api/artifacts/{id}/versions/1/files/index.html"))
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-security-policy"], "sandbox");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(res.text().await.unwrap(), "<h1>raw</h1>");
    let res = ts
        .get(&format!("/api/artifacts/{id}/versions/1/files/a.css"))
        .await;
    assert_eq!(res.text().await.unwrap(), "p{}");
    let res = ts
        .get(&format!("/api/artifacts/{id}/versions/1/files/missing.css"))
        .await;
    assert_eq!(res.status(), 404);
    let res = ts
        .get(&format!("/api/artifacts/{id}/versions/9/files/index.html"))
        .await;
    assert_eq!(res.status(), 404);
}

/// A non-loopback IPv4 address of this machine (connecting a UDP socket sends nothing).
fn non_loopback_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("10.255.255.255:1").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tokio::test]
async fn mcp_accepts_the_daemons_own_address_when_bound_to_it() {
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("skipping: this machine has no non-loopback IPv4 address");
        return;
    };
    let ts = TestServer::spawn_on(ip, |_| {}).await;
    assert_eq!(ts.base, format!("http://{ip}:{}", ts.addr.port()));
    let res = mcp_post(&ts, &initialize())
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let init = rpc_message(res).await;
    assert!(init["result"]["instructions"].is_string(), "{init}");
    let res = mcp_post(&ts, &initialize())
        .bearer_auth(&ts.token)
        .header("host", "evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

/// Initializes an MCP session on `/mcp`; the `mcp-session-id`, if any.
async fn mcp_session(ts: &TestServer) -> Option<String> {
    let res = mcp_post(ts, &initialize())
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let session = res
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    rpc_message(res).await;
    let mut req = mcp_post(
        ts,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .bearer_auth(&ts.token);
    if let Some(s) = &session {
        req = req.header("mcp-session-id", s);
    }
    assert!(req.send().await.unwrap().status().is_success());
    session
}

async fn mcp_call(
    ts: &TestServer,
    session: &Option<String>,
    id: u32,
    name: &str,
    args: Value,
) -> Value {
    let mut req = mcp_post(
        ts,
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}}),
    )
    .bearer_auth(&ts.token);
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    let res = req.send().await.unwrap();
    assert_eq!(res.status(), 200);
    rpc_message(res).await
}

/// The daemon's `/mcp` has no harness session, so agent replies and resolves
/// fail with `unknown_session` and write nothing.
#[tokio::test]
async fn mcp_reply_and_resolve_without_a_session_are_unknown_session() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent fix").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let session = mcp_session(&ts).await;
    for (id, name, args) in [
        (
            2,
            "comments_reply",
            json!({"url_or_id": aid, "thread_id": tid, "text": "done"}),
        ),
        (
            3,
            "comments_resolve",
            json!({"url_or_id": aid, "thread_id": tid}),
        ),
    ] {
        let call = mcp_call(&ts, &session, id, name, args).await;
        assert_eq!(call["result"]["isError"], true, "{name}: {call}");
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown_session"), "{name}: {text}");
    }
    let after: Value = ts
        .get(&format!("/api/artifacts/{aid}/threads/{tid}"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(after["thread"]["status"], "open");
    assert_eq!(after["thread"]["comments"].as_array().unwrap().len(), 1);
}
