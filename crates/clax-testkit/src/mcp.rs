//! The MCP endpoints' request helpers the server's test modules share: the
//! two endpoints' paths, an `initialize` request, and the JSON-RPC message of
//! a plain JSON or SSE response.

use super::TestServer;
use serde_json::{Value, json};

pub const ACCEPT: &str = "application/json, text/event-stream";

/// rmcp's streamable HTTP service, the daemon's `/mcp`.
pub const RMCP: &str = "/mcp";
/// The tool set's JSON-RPC adapter (`clax_mcp::http`), what a Durable
/// Object serves at `/mcp`.
pub const ADAPTER: &str = "/mcp/json";

pub fn initialize() -> Value {
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

pub fn mcp_post(ts: &TestServer, path: &str, body: &Value) -> reqwest::RequestBuilder {
    ts.client
        .post(format!("{}{path}", ts.base))
        .header("accept", ACCEPT)
        .json(body)
}

/// The JSON-RPC message in a response that is either plain JSON or an SSE stream.
pub async fn rpc_message(res: reqwest::Response) -> Value {
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
