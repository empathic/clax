mod common;
use common::TestServer;
use serde_json::{Value, json};

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
async fn mcp_lists_the_nine_tools() {
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
            "delete",
            "list",
            "open",
            "pin",
            "publish",
            "read",
            "status",
            "unpin"
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
