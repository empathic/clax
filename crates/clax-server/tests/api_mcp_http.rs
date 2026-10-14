//! The tool set's JSON-RPC adapter (`/mcp/json`, `clax_mcp::http`) against
//! rmcp's service (`/mcp`): the same answers to the same messages, real
//! clients' exchanges, and the HTTP rules of a server with no SSE stream
//! and no session. The shared suite in `api_mcp.rs` runs on both too.

use crate::api_mcp::{ACCEPT, ADAPTER, RMCP, initialize, mcp_post, rpc_message};
use crate::common;
use common::TestServer;
use serde_json::{Value, json};

/// The per-request metadata 2026-07-28 requires, for `version`.
fn meta(version: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "api-mcp-http-test", "version": "0"},
    })
}

/// POSTs `body` to `path` with `headers` and the token, and the session
/// header when there is one: the status and the JSON-RPC message.
async fn send(
    ts: &TestServer,
    path: &str,
    session: &Option<String>,
    headers: &[(&str, &str)],
    body: &Value,
) -> (u16, Value) {
    let mut req = mcp_post(ts, path, body).bearer_auth(&ts.token);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    let res = req.send().await.unwrap();
    (res.status().as_u16(), rpc_message(res).await)
}

/// Initializes at `version` on `path` and sends `notifications/initialized`:
/// the session rmcp issues, if any.
async fn session(ts: &TestServer, path: &str, version: &str) -> Option<String> {
    let mut body = initialize();
    body["params"]["protocolVersion"] = json!(version);
    let res = mcp_post(ts, path, &body)
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
        path,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .bearer_auth(&ts.token)
    .header("mcp-protocol-version", version);
    if let Some(s) = &session {
        req = req.header("mcp-session-id", s);
    }
    assert!(req.send().await.unwrap().status().is_success());
    session
}

/// Each message, on both endpoints, gets the same status and the same
/// JSON-RPC answer: `initialize` at every version (and at one neither
/// knows), the session-era methods after an `initialize`, and the
/// 2026-07-28 per-request ones with no session.
#[tokio::test]
async fn the_adapter_answers_as_rmcp_does() {
    let ts = TestServer::spawn().await;
    for version in [
        "2024-11-05",
        "2025-03-26",
        "2025-06-18",
        "2025-11-25",
        "2026-07-28",
        "2099-01-01",
    ] {
        let mut body = initialize();
        body["params"]["protocolVersion"] = json!(version);
        let a = send(&ts, RMCP, &None, &[], &body).await;
        let b = send(&ts, ADAPTER, &None, &[], &body).await;
        assert_eq!(a, b, "initialize at {version}");
    }

    let legacy = [
        json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "resources/templates/list"}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "prompts/list"}),
        json!({"jsonrpc": "2.0", "id": 7, "method": "prompts/get", "params": {"name": "x"}}),
        json!({"jsonrpc": "2.0", "id": 8, "method": "no/such/method", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "nope", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 10, "method": "tools/call", "params": {"name": "publish", "arguments": {"html": 5}}}),
        json!({"jsonrpc": "2.0", "id": 11, "method": "tools/call", "params": {"name": "list", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 12, "method": "tools/call", "params": {"name": "read", "arguments": {"url_or_id": "0000000000000"}}}),
        json!({"jsonrpc": "2.0", "id": 13, "method": "tools/call", "params": {"name": "comments_reply", "arguments": {"url_or_id": "x", "thread_id": "y", "text": "z"}}}),
        json!({"jsonrpc": "2.0", "id": 14, "method": "tools/call", "params": {"name": "list", "arguments": 5}}),
        json!({"jsonrpc": "2.0", "id": 15, "method": "tools/call", "params": {"name": "list", "arguments": {"limit": "many"}}}),
        json!({"jsonrpc": "2.0", "id": 16, "method": "tools/call", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 17, "method": "tools/call", "params": {"name": "comments_reply", "arguments": {"url_or_id": "x", "thread_id": "../../../../mcp/json", "text": "z"}}}),
    ];
    for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
        let rmcp = session(&ts, RMCP, version).await;
        let adapter = session(&ts, ADAPTER, version).await;
        assert_eq!(adapter, None, "the adapter issues no session");
        // 2025-03-26 clients send no version header.
        let headers: &[(&str, &str)] = if version == "2025-03-26" {
            &[]
        } else {
            &[("mcp-protocol-version", version)]
        };
        for body in &legacy {
            let a = send(&ts, RMCP, &rmcp, headers, body).await;
            let b = send(&ts, ADAPTER, &adapter, headers, body).await;
            assert_eq!(a, b, "{body} at {version}");
        }
    }

    let v = "2026-07-28";
    let per_request = [
        (
            "tools/list",
            json!({"jsonrpc": "2.0", "id": 20, "method": "tools/list", "params": {"_meta": meta(v)}}),
        ),
        (
            "ping",
            json!({"jsonrpc": "2.0", "id": 21, "method": "ping", "params": {"_meta": meta(v)}}),
        ),
        (
            "server/discover",
            json!({"jsonrpc": "2.0", "id": 22, "method": "server/discover", "params": {"_meta": meta(v)}}),
        ),
        (
            "server/discover",
            json!({"jsonrpc": "2.0", "id": 23, "method": "server/discover", "params": {"_meta": meta("2099-01-01")}}),
        ),
        (
            "server/discover",
            json!({"jsonrpc": "2.0", "id": 24, "method": "server/discover", "params": {}}),
        ),
        (
            "tools/list",
            json!({"jsonrpc": "2.0", "id": 25, "method": "tools/list", "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": v}}}),
        ),
    ];
    for (method, body) in &per_request {
        let version = body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"]
            .as_str()
            .unwrap_or(v);
        let headers = [("mcp-protocol-version", version), ("mcp-method", *method)];
        let a = send(&ts, RMCP, &None, &headers, body).await;
        let b = send(&ts, ADAPTER, &None, &headers, body).await;
        assert_eq!(a, b, "{body}");
    }
    // The per-request header without the metadata it promises.
    let body = json!({"jsonrpc": "2.0", "id": 26, "method": "tools/list", "params": {}});
    let headers = [("mcp-protocol-version", v), ("mcp-method", "tools/list")];
    let a = send(&ts, RMCP, &None, &headers, &body).await;
    let b = send(&ts, ADAPTER, &None, &headers, &body).await;
    assert_eq!(a, b, "{body}");
}

/// The exchange Claude Code's `claude mcp list` makes against an HTTP MCP
/// server (`claude mcp add --transport http`), recorded from Claude Code
/// 2.1.296: a `server/discover` probe and `tools/list`, on the 2026-07-28
/// per-request lifecycle. Replayed on both endpoints it is answered alike,
/// and a `tools/call` in the same shape is too.
#[tokio::test]
async fn claude_codes_recorded_exchange_is_answered_alike() {
    let ts = TestServer::spawn().await;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/claude_code_mcp_http.json"))
        .expect("fixture is JSON");
    let mut requests: Vec<Value> = fixture["requests"].as_array().unwrap().clone();
    assert_eq!(requests.len(), 2);
    // A tools/call as the same client would send it.
    let mut call = requests[1].clone();
    call["headers"]["mcp-method"] = json!("tools/call");
    call["headers"]["mcp-name"] = json!("list");
    call["body"]["id"] = json!(1);
    call["body"]["method"] = json!("tools/call");
    call["body"]["params"]["name"] = json!("list");
    call["body"]["params"]["arguments"] = json!({});
    requests.push(call);
    let mut answers = Vec::new();
    for r in &requests {
        let headers: Vec<(String, String)> = r["headers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let a = send(&ts, RMCP, &None, &headers, &r["body"]).await;
        let b = send(&ts, ADAPTER, &None, &headers, &r["body"]).await;
        assert_eq!(a, b, "{}", r["body"]);
        assert_eq!(b.0, 200, "{}", b.1);
        answers.push(b.1);
    }
    let versions = &answers[0]["result"]["supportedVersions"];
    assert!(
        versions
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "2026-07-28"),
        "{versions}"
    );
    assert_eq!(
        answers[0]["result"]["instructions"],
        initialize_instructions(&ts).await
    );
    let tools = answers[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), clax_mcp::ClaxTools::tool_count());
    assert_eq!(answers[2]["result"]["isError"], false, "{}", answers[2]);
}

/// The instructions rmcp's `/mcp` gives at `initialize`.
async fn initialize_instructions(ts: &TestServer) -> Value {
    let (_, v) = send(ts, RMCP, &None, &[], &initialize()).await;
    v["result"]["instructions"].clone()
}

/// rmcp's own client (the Streamable HTTP client Codex embeds) connects to
/// both endpoints, lists the tools and publishes through each.
#[tokio::test]
async fn rmcps_client_works_on_both_endpoints() {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    let ts = TestServer::spawn().await;
    for path in [RMCP, ADAPTER] {
        let config = StreamableHttpClientTransportConfig::with_uri(format!("{}{path}", ts.base))
            .auth_header(ts.token.clone());
        let client =
            ().serve(StreamableHttpClientTransport::from_config(config))
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        let info = client.peer_info().expect("server info");
        assert_eq!(
            info.server_info.as_ref().map(|i| i.name.as_str()),
            Some("clax"),
            "{path}"
        );
        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools, clax_mcp::ClaxTools::tools(), "{path}");
        let title = format!("Via {path}");
        let args = json!({"html": "<p>hi</p>", "title": title});
        let r = client
            .call_tool(
                CallToolRequestParams::new("publish")
                    .with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(false), "{path}: {r:?}");
        client.cancel().await.unwrap();
        let listed: Value = ts.get("/api/artifacts").await.json().await.unwrap();
        assert_eq!(listed["artifacts"][0]["title"], title.as_str());
    }
}

/// What a server with no SSE stream and no session answers over HTTP:
/// `GET` and `DELETE` are 405 naming `POST`, notifications are 202 with no
/// body, batches are refused, and malformed bodies are refused with clear
/// JSON-RPC errors.
#[tokio::test]
async fn the_adapter_follows_the_http_rules_of_a_stateless_server() {
    let ts = TestServer::spawn().await;
    let url = format!("{}{ADAPTER}", ts.base);
    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let res = ts
            .client
            .request(method.clone(), &url)
            .bearer_auth(&ts.token)
            .header("accept", "text/event-stream")
            .header("mcp-session-id", "anything")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 405, "{method}");
        assert_eq!(res.headers()["allow"], "POST", "{method}");
    }
    // A GET without the token is refused before anything else.
    let res = ts.client.get(&url).send().await.unwrap();
    assert_eq!(res.status(), 401);

    let post = |body: String| {
        ts.client
            .post(&url)
            .bearer_auth(&ts.token)
            .header("accept", ACCEPT)
            .header("content-type", "application/json")
            .body(body)
    };
    // `initialize` issues no session.
    let res = post(initialize().to_string()).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(res.headers().get("mcp-session-id").is_none());
    assert_eq!(res.headers()["content-type"], "application/json");

    let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let res = post(notification.to_string()).send().await.unwrap();
    assert_eq!(res.status(), 202);
    assert!(res.bytes().await.unwrap().is_empty());
    // A response from the client is taken without an answer too.
    let res = post(json!({"jsonrpc": "2.0", "id": 9, "result": {}}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);

    // A message that does not parse as a request is answered with the
    // JSON-RPC error naming why.
    for (body, code, message) in [
        (
            json!({"jsonrpc": "2.0", "id": 3, "method": "no/such/method"}),
            -32601,
            Some("no/such/method"),
        ),
        // rmcp's model reads a `tools/call` it cannot parse as a method of
        // its own, which no handler answers.
        (
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"arguments": {}}}),
            -32601,
            Some("tools/call"),
        ),
        (
            json!({"jsonrpc": "1.0", "id": 5, "method": "ping"}),
            -32600,
            None,
        ),
    ] {
        let res = post(body.to_string()).send().await.unwrap();
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["id"], body["id"], "{v}");
        assert_eq!(v["error"]["code"], code, "{status} {v}");
        if let Some(m) = message {
            assert_eq!(v["error"]["message"], m, "{v}");
        }
    }
    // Batches are refused, whatever they hold: MCP dropped them, rmcp's
    // service does not read them, and each would run its tool calls under
    // one request.
    for batch in [
        json!([]),
        json!([notification]),
        json!([initialize(), {"jsonrpc": "2.0", "id": "two", "method": "ping"}]),
    ] {
        let res = post(batch.to_string()).send().await.unwrap();
        assert_eq!(res.status(), 400, "{batch}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], -32600, "{v}");
        assert_eq!(
            v["error"]["message"],
            "Invalid Request: JSON-RPC batches are not supported"
        );
    }

    let res = post("{not json".into()).send().await.unwrap();
    assert_eq!(res.status(), 400);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], -32700, "{v}");

    let res = ts
        .client
        .post(&url)
        .bearer_auth(&ts.token)
        .header("content-type", "text/plain")
        .body(initialize().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 415);
    let res = ts
        .client
        .post(&url)
        .bearer_auth(&ts.token)
        .header("accept", "text/event-stream")
        .json(&initialize())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 406);
    // A client that names no media type gets JSON.
    let res = ts
        .client
        .post(&url)
        .bearer_auth(&ts.token)
        .json(&initialize())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = post(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string())
        .header("mcp-protocol-version", "1999-01-01")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

/// A person's agent, with a valid signed grant, may not use the adapter,
/// as it may not use `/mcp`: both are the owner's.
#[tokio::test]
async fn a_persons_agent_is_refused_on_both_endpoints() {
    use clax_testkit::people::{As, World};
    let w = World::two_people().await;
    let agent = w.sam_agent();
    let As::Grant(c) = agent.who else {
        unreachable!("Sam's agent holds a grant")
    };
    let body = serde_json::to_vec(&initialize()).unwrap();
    let mut answers = Vec::new();
    for path in [RMCP, ADAPTER] {
        let res = c
            .call(clax_testkit::Call {
                method: "POST",
                path,
                authorization: Some(format!("Clax-Grant {}", c.access())),
                body: Some(body.clone()),
                headers: vec![
                    ("content-type", "application/json".into()),
                    ("accept", ACCEPT.into()),
                    ("x-clax-session", agent.session.clone()),
                ],
                ..Default::default()
            })
            .await;
        let status = res.status().as_u16();
        let v: Value = res.json().await.unwrap();
        assert_eq!(status, 403, "{path}: {v}");
        answers.push(v);
    }
    assert_eq!(answers[0], answers[1]);
    assert_eq!(answers[1]["error"]["code"], "forbidden", "{}", answers[1]);
}

/// The daemon's state, captured as the test server starts.
async fn server_and_state() -> (TestServer, clax_http::state::AppState) {
    let mut slot = None;
    let ts = TestServer::spawn_with(|s| slot = Some(s.clone())).await;
    (ts, slot.expect("the state is built"))
}

/// One in-process request to `path` through `lb`, with `authorization`.
async fn in_process(
    lb: &clax_server::loopback::Loopback,
    method: &str,
    path: &str,
    authorization: Option<&str>,
    body: &str,
) -> (u16, Value) {
    use clax_mcp::client::InProcess;
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("accept", ACCEPT)
        .header("x-clax-channel", "agent");
    if let Some(a) = authorization {
        req = req.header("authorization", a);
    }
    let res = lb
        .call(req.body(bytes::Bytes::from(body.to_string())).unwrap())
        .await;
    let status = res.status().as_u16();
    let v = serde_json::from_slice(res.body()).unwrap_or(Value::Null);
    (status, v)
}

/// An in-process request carries what its caller sent and nothing more:
/// no connection of this machine's (so none of a loopback peer's rights),
/// at a `Host` the `/api` check admits whatever `self_base` names, and only
/// to the routes the tool set calls, never back into an MCP endpoint.
#[tokio::test]
async fn an_in_process_request_has_only_its_callers_rights() {
    let (ts, mut state) = server_and_state().await;
    // A Durable Object's base is its public origin.
    state.self_base = "https://clax.example".into();
    let lb = clax_server::loopback::Loopback::new(state);
    let owner = format!("Bearer {}", ts.token);
    let (status, v) = in_process(&lb, "GET", "/api/artifacts", Some(&owner), "").await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = in_process(
        &lb,
        "GET",
        "/api/live/pages?url=http%3A%2F%2Fexample.com%2F",
        Some(&owner),
        "",
    )
    .await;
    assert_eq!(status, 200, "{v}");
    // Over loopback, a request without the token may see live pages and
    // read the token.
    let local = format!("{}/api/live/pages?url=http%3A%2F%2Fexample.com%2F", ts.base);
    assert_eq!(ts.client.get(&local).send().await.unwrap().status(), 200);
    let token = format!("{}/api/token", ts.base);
    assert_eq!(ts.client.get(&token).send().await.unwrap().status(), 200);
    // In-process, it may not.
    for auth in [None, Some("Bearer not-the-token")] {
        let (status, v) = in_process(
            &lb,
            "GET",
            "/api/live/pages?url=http%3A%2F%2Fexample.com%2F",
            auth,
            "",
        )
        .await;
        assert_eq!(status, 404, "{auth:?}: {v}");
        let (status, v) = in_process(&lb, "GET", "/api/token", auth, "").await;
        assert_ne!(status, 200, "{auth:?}: {v}");
        let body = json!({"title": "No", "files": {"index.html": "<p>no</p>"}}).to_string();
        let (status, v) = in_process(&lb, "POST", "/api/artifacts", auth, &body).await;
        assert_eq!(status, 401, "{auth:?}: {v}");
    }
    for path in [ADAPTER, RMCP, "/c/anything", "/"] {
        let body = initialize().to_string();
        let (status, v) = in_process(&lb, "POST", path, Some(&owner), &body).await;
        assert_eq!(status, 404, "{path}: {v}");
        assert_eq!(v["error"]["code"], "not_in_process", "{path}: {v}");
    }
}

/// A response larger than the loopback's limit is not read: the caller
/// gets `502 response_too_large`.
#[tokio::test]
async fn an_in_process_response_is_capped() {
    let (ts, state) = server_and_state().await;
    let lb = clax_server::loopback::Loopback::new(state.clone()).with_response_limit(64);
    let owner = format!("Bearer {}", ts.token);
    ts.publish(
        "A title long enough to pass the limit",
        &[("index.html", "<p>x</p>")],
    )
    .await;
    let (status, v) = in_process(&lb, "GET", "/api/artifacts", Some(&owner), "").await;
    assert_eq!(status, 502, "{v}");
    assert_eq!(v["error"]["code"], "response_too_large", "{v}");
    let lb = clax_server::loopback::Loopback::new(state);
    let (status, v) = in_process(&lb, "GET", "/api/artifacts", Some(&owner), "").await;
    assert_eq!(status, 200, "{v}");
}

/// A tool called through the adapter's tools acts with its caller's
/// credential and never the daemon's token: a caller without the token
/// cannot publish through a tool.
#[tokio::test]
async fn the_adapters_tools_act_with_the_callers_credential() {
    let (ts, state) = server_and_state().await;
    let tools = clax_server::routes::mcp_http::tools(&state);
    let publish = |auth: Option<&str>, title: &str| {
        let tools = tools.for_caller(auth.map(|a| axum::http::HeaderValue::from_str(a).unwrap()));
        let args = json!({"html": "<p>hi</p>", "title": title});
        async move {
            let r = tools
                .call_direct(
                    rmcp::model::CallToolRequestParams::new("publish")
                        .with_arguments(args.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            let rmcp::model::CallToolResponse::Complete(r) = r else {
                panic!("publish completes")
            };
            serde_json::to_value(r).unwrap()
        }
    };
    for auth in [None, Some("Bearer not-the-token")] {
        let r = publish(auth, "Refused").await;
        assert_eq!(r["isError"], true, "{auth:?}: {r}");
    }
    let r = publish(Some(&format!("Bearer {}", ts.token)), "Mine").await;
    assert_eq!(r["isError"], false, "{r}");
    let listed: Value = ts.get_authed("/api/artifacts").await.json().await.unwrap();
    let titles: Vec<&Value> = listed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| &a["title"])
        .collect();
    assert_eq!(titles, [&json!("Mine")]);
}
