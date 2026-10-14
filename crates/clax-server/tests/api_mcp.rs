use crate::common;
use common::TestServer;
use serde_json::{Value, json};
use std::net::{IpAddr, UdpSocket};

pub(crate) const ACCEPT: &str = "application/json, text/event-stream";

/// rmcp's streamable HTTP service, the daemon's `/mcp`.
pub(crate) const RMCP: &str = "/mcp";
/// The tool set's JSON-RPC adapter (`clax_mcp::http`), what a Durable
/// Object serves at `/mcp`.
pub(crate) const ADAPTER: &str = "/mcp/json";

/// Runs test `$name(path)` against both MCP endpoints: one suite proves
/// they answer alike.
macro_rules! both {
    ($name:ident) => {
        mod $name {
            #[tokio::test]
            async fn rmcp() {
                super::$name(super::RMCP).await
            }
            #[tokio::test]
            async fn adapter() {
                super::$name(super::ADAPTER).await
            }
        }
    };
}

pub(crate) fn initialize() -> Value {
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

pub(crate) fn mcp_post(ts: &TestServer, path: &str, body: &Value) -> reqwest::RequestBuilder {
    ts.client
        .post(format!("{}{path}", ts.base))
        .header("accept", ACCEPT)
        .json(body)
}

/// The JSON-RPC message in a response that is either plain JSON or an SSE stream.
pub(crate) async fn rpc_message(res: reqwest::Response) -> Value {
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

async fn mcp_requires_the_bearer_token(path: &str) {
    let ts = TestServer::spawn().await;
    let res = mcp_post(&ts, path, &initialize()).send().await.unwrap();
    assert_eq!(res.status(), 401);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");

    let res = mcp_post(&ts, path, &initialize())
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}
both!(mcp_requires_the_bearer_token);

async fn mcp_refuses_a_non_local_host(path: &str) {
    let ts = TestServer::spawn().await;
    let res = mcp_post(&ts, path, &initialize())
        .bearer_auth(&ts.token)
        .header("host", "evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}
both!(mcp_refuses_a_non_local_host);

async fn mcp_lists_the_twenty_five_tools(path: &str) {
    let ts = TestServer::spawn().await;
    let res = mcp_post(&ts, path, &initialize())
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
            path,
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
            path,
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
            "ask",
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
            "draft",
            "list",
            "open",
            "pin",
            "publish",
            "read",
            "status",
            "unpin",
            "wait_for_feedback",
            "watch",
            "working"
        ]
    );

    // A tool call goes through the daemon's own REST API.
    let res = with_session(
        mcp_post(
            &ts,
            path,
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
both!(mcp_lists_the_twenty_five_tools);

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

async fn mcp_accepts_the_daemons_own_address_when_bound_to_it(path: &str) {
    let Some(ip) = non_loopback_ipv4() else {
        eprintln!("skipping: this machine has no non-loopback IPv4 address");
        return;
    };
    let ts = TestServer::spawn_on(ip, |_| {}).await;
    assert_eq!(ts.base, format!("http://{ip}:{}", ts.addr.port()));
    let res = mcp_post(&ts, path, &initialize())
        .bearer_auth(&ts.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let init = rpc_message(res).await;
    assert!(init["result"]["instructions"].is_string(), "{init}");
    let res = mcp_post(&ts, path, &initialize())
        .bearer_auth(&ts.token)
        .header("host", "evil.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}
both!(mcp_accepts_the_daemons_own_address_when_bound_to_it);

/// Initializes an MCP session on `/mcp`; the `mcp-session-id`, if any.
async fn mcp_session(ts: &TestServer, path: &str) -> Option<String> {
    let res = mcp_post(ts, path, &initialize())
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
    .bearer_auth(&ts.token);
    if let Some(s) = &session {
        req = req.header("mcp-session-id", s);
    }
    assert!(req.send().await.unwrap().status().is_success());
    session
}

async fn mcp_call(
    ts: &TestServer,
    path: &str,
    session: &Option<String>,
    id: u32,
    name: &str,
    args: Value,
) -> Value {
    let mut req = mcp_post(
        ts,
        path,
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
async fn mcp_reply_and_resolve_without_a_session_are_unknown_session(path: &str) {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent fix").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let session = mcp_session(&ts, path).await;
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
        let call = mcp_call(&ts, path, &session, id, name, args).await;
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
both!(mcp_reply_and_resolve_without_a_session_are_unknown_session);

/// Drafting needs a harness session, which `/mcp` has none of: the `draft`
/// tool is called (not unknown to the endpoint) and answers `no_session`.
async fn mcp_draft_without_a_session_is_no_session(path: &str) {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let session = mcp_session(&ts, path).await;
    let call = mcp_call(
        &ts,
        path,
        &session,
        2,
        "draft",
        json!({"url_or_id": aid, "html": "<h2>y</h2>"}),
    )
    .await;
    assert_eq!(call["result"]["isError"], true, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("no_session"), "{text}");
}
both!(mcp_draft_without_a_session_is_no_session);

/// A `server/discover` probe for `version`, with its `MCP-Protocol-Version`
/// header and no session.
async fn mcp_discover(ts: &TestServer, path: &str, version: &str) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": "probe", "method": "server/discover", "params": {"_meta": {
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "api-mcp-test", "version": "0"},
    }}});
    let res = mcp_post(ts, path, &body)
        .bearer_auth(&ts.token)
        .header("mcp-protocol-version", version)
        .header("mcp-method", "server/discover")
        .send()
        .await
        .unwrap();
    assert!(res.headers().get("mcp-session-id").is_none());
    rpc_message(res).await
}

/// A discover probe opens no session, so `initialize` after it starts a
/// legacy session whose requests need no `_meta`.
async fn mcp_serves_a_legacy_session_after_a_discover_probe(path: &str) {
    let ts = TestServer::spawn().await;
    let v = mcp_discover(&ts, path, "2099-01-01").await;
    assert_eq!(v["error"]["code"], -32022, "{v}");
    let v = mcp_discover(&ts, path, "2026-07-28").await;
    let versions = v["result"]["supportedVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("{v}"));
    assert!(versions.iter().any(|x| x == "2026-07-28"), "{v}");
    let session = mcp_session(&ts, path).await;
    let mut req = mcp_post(
        &ts,
        path,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .bearer_auth(&ts.token);
    if let Some(s) = &session {
        req = req.header("mcp-session-id", s);
    }
    let list = rpc_message(req.send().await.unwrap()).await;
    assert!(list["result"]["tools"].as_array().is_some(), "{list}");
    let v = mcp_call(&ts, path, &session, 3, "status", json!({})).await;
    assert!(v["result"]["isError"] != true, "{v}");
}
both!(mcp_serves_a_legacy_session_after_a_discover_probe);

/// The daemon's `/mcp` acts as the sessionless agent on `mcp` (spec §5.2),
/// under the call's identity, with no working directory to capture.
async fn mcp_tool_calls_are_the_sessionless_agent_under_a_call(path: &str) {
    let ts = TestServer::spawn().await;
    let session = mcp_session(&ts, path).await;
    let args = json!({"html": "<p>via mcp</p>", "title": "Via MCP"});
    let call = mcp_call(&ts, path, &session, 2, "publish", args.clone()).await;
    assert_eq!(call["result"]["isError"], false, "{call}");
    let c = rusqlite::Connection::open(ts.home.db_path()).unwrap();
    let (actor, call_id, body): (String, Option<String>, String) = c
        .query_row(
            "SELECT actor, call_id, body FROM audit_events WHERE kind = 'artifact.create'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let actor: Value = serde_json::from_str(&actor).unwrap();
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(actor, json!({"type": "agent", "session_id": null}));
    assert_eq!(body["via"], "mcp");
    assert_eq!(body["git_capture"], "no-cwd");
    assert_eq!(body["call"]["tool"], "publish");
    assert_eq!(
        body["call"]["args_sha256"],
        clax_core::toolpath::args::args_sha256(&args)
    );
    assert_eq!(call_id.as_deref(), body["call"]["call_id"].as_str());
}
both!(mcp_tool_calls_are_the_sessionless_agent_under_a_call);

/// The `read` tool, for an agent whose person may not see a file, hands
/// it the daemon's `withheld` answer, alike for a withheld file and one
/// the version lacks, and never the name it was asked for.
async fn the_read_tool_answers_a_withheld_file_as_withheld(path: &str) {
    use crate::common::people::{CARRIED_NAME, FILE_NAME, World};
    let w = World::two_people().await;
    let session = mcp_session(&w.ts, path).await;
    let aid = &w.read.artifact;
    let mut answers = Vec::new();
    for (i, file) in [FILE_NAME, CARRIED_NAME, "not-there.css"]
        .into_iter()
        .enumerate()
    {
        let v = mcp_call(
            &w.ts,
            path,
            &session,
            10 + i as u32,
            "read",
            json!({"url_or_id": aid, "version": 2, "path": file}),
        )
        .await;
        let text = v.to_string();
        assert_eq!(v["result"]["isError"], true, "{file}: {text}");
        assert!(
            text.contains("withheld") && text.contains("Mia"),
            "{file}: {text}"
        );
        assert!(!text.contains(file), "{file}: {text}");
        answers.push(v["result"]["content"].clone());
    }
    assert!(answers.windows(2).all(|p| p[0] == p[1]), "{answers:?}");
    // A file it may see reads as before.
    let v = mcp_call(
        &w.ts,
        path,
        &session,
        20,
        "read",
        json!({"url_or_id": aid, "version": 2, "path": "owner.html"}),
    )
    .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    assert!(v.to_string().contains("<p>mine</p>"), "{v}");
}
both!(the_read_tool_answers_a_withheld_file_as_withheld);

/// The db tools, for an agent whose person may not see a document, hand
/// it the placeholder (`withheld` with its authors, no data) when it names
/// the document, and a listing's count of withheld documents.
async fn the_db_tools_answer_withheld_documents_as_withheld(path: &str) {
    use crate::common::people::{DOC_FIELD, World};
    let w = World::two_people().await;
    let session = mcp_session(&w.ts, path).await;
    let aid = &w.read.artifact;
    let v = mcp_call(
        &w.ts,
        path,
        &session,
        10,
        "db_get",
        json!({"url_or_id": aid, "collection": "notes", "doc_id": "MIACANARYdocpath"}),
    )
    .await;
    let text = v.to_string();
    assert_eq!(v["result"]["isError"], false, "{text}");
    assert!(!text.contains(DOC_FIELD), "{text}");
    let r: Value =
        serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(r["exists"], true, "{r}");
    assert_eq!(r["doc"]["withheld"]["authors"][0]["name"], "Mia", "{r}");
    assert!(r["doc"].get("data").is_none(), "{r}");
    let v = mcp_call(
        &w.ts,
        path,
        &session,
        11,
        "db_list",
        json!({"url_or_id": aid, "collection": "notes"}),
    )
    .await;
    let r: Value =
        serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        (r["docs"].clone(), r["withheld_count"].clone()),
        (json!([]), json!(2)),
        "{r}"
    );
}
both!(the_db_tools_answer_withheld_documents_as_withheld);

/// An asset upload goes out as a multipart form: streamed over HTTP by
/// `/mcp`'s tools, written out whole by the adapter's in-process client.
async fn mcp_uploads_an_asset(path: &str) {
    let ts = TestServer::spawn().await;
    let a = ts.publish("Doc", &[("index.html", "<h2>x</h2>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dot \"1\".png");
    let bytes = [137u8, 80, 78, 71, 0, 255];
    std::fs::write(&file, bytes).unwrap();
    let session = mcp_session(&ts, path).await;
    let v = mcp_call(
        &ts,
        path,
        &session,
        2,
        "asset_upload",
        json!({"url_or_id": aid, "file_path": file.to_string_lossy()}),
    )
    .await;
    assert_eq!(v["result"]["isError"], false, "{v}");
    let r: Value =
        serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let asset = &r["assets"][0];
    assert_eq!(asset["content_type"], "image/png", "{r}");
    assert_eq!(asset["size"], 6, "{r}");
    let url = asset["url"].as_str().unwrap();
    let blob = url.split_once(&format!(":{}", ts.addr.port())).unwrap().1;
    let res = ts.get(blob).await;
    assert_eq!(res.bytes().await.unwrap().as_ref(), bytes);
}
both!(mcp_uploads_an_asset);
