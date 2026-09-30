//! Commands that print an MCP tool's result: they run the tool against the
//! daemon, so the CLI and the tools share one implementation and one JSON shape.

use crate::client::Client;
use clax_core::Home;
use clax_mcp::{ClaxTools, DaemonClient};
use rmcp::model::CallToolResult;
use std::future::Future;

/// Runs `f` with the tool set for the daemon (started if needed) and returns
/// the tool's result object. An error result becomes an error naming its
/// `code` and `message`.
pub fn call<F, Fut>(cli: &crate::Cli, home: &Home, f: F) -> anyhow::Result<serde_json::Value>
where
    F: FnOnce(ClaxTools) -> Fut,
    Fut: Future<Output = Result<CallToolResult, rmcp::ErrorData>>,
{
    let c = Client::connect(home, cli.port)?;
    let tools = ClaxTools::new(
        DaemonClient::new(c.base.clone(), c.token.clone(), None),
        c.browser_url(""),
        None,
        home.log_path(),
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = rt
        .block_on(f(tools))
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .ok_or_else(|| anyhow::anyhow!("the tool returned no text"))?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    if result.is_error == Some(true) {
        let e = &value["error"];
        anyhow::bail!(
            "{}: {}",
            e["code"].as_str().unwrap_or("error"),
            e["message"].as_str().unwrap_or("the tool failed")
        );
    }
    Ok(value)
}
