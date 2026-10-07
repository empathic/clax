//! The MCP server a Clax plugin copy serves when another copy acts for the
//! harness session: it completes the handshake and offers one tool,
//! `status`, whose result says which copy acts. It needs no daemon.

use crate::tools::StatusArgs;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};

/// What the Claude Code plugin's copy says when Grok Build runs it. The
/// wrapper (`scripts/ensure-clax.sh`, `GROK_STANDDOWN`) carries the same
/// text; `scripts/test-plugins.sh` checks that they match.
pub const GROK_STANDDOWN: &str = "This is the Clax plugin for Claude Code, which Grok Build also loads. In Grok, Clax runs from the clax-grok plugin, whose tools are named `clax_grok__<tool>` (for example `clax_grok__publish`); this server does nothing. If no `clax_grok` tools are listed, run `clax init --agent grok`. To remove this server from Grok, run `grok plugin disable clax`.";

/// The description of the stand-down server's `status` tool.
pub const STATUS_DESCRIPTION: &str =
    "Says which Clax plugin serves this Grok session; this server does nothing else.";

/// A server whose one tool, `status`, returns `text`.
#[derive(Clone)]
pub struct StandDown {
    text: &'static str,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl StandDown {
    pub fn new(text: &'static str) -> StandDown {
        StandDown {
            text,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Says which Clax plugin serves this Grok session; this server does nothing else.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    pub async fn status(
        &self,
        Parameters(_args): Parameters<StatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(self.text)]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for StandDown {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(self.text);
        config.server_info = Implementation::new("clax", env!("CARGO_PKG_VERSION"));
        config
    }
}

/// Serves [`StandDown`] with `text` on stdin/stdout until the client
/// closes the connection.
pub async fn serve(text: &'static str) -> anyhow::Result<()> {
    let service = {
        let server = StandDown::new(text);
        let transport = crate::probe::stdio(&server);
        server.serve(transport)
    }
    .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_offers_exactly_one_tool_named_status() {
        let names: Vec<String> = StandDown::new(GROK_STANDDOWN)
            .tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(names, ["status"]);
        assert!(GROK_STANDDOWN.contains("clax_grok__publish"));
        assert!(GROK_STANDDOWN.contains("clax init --agent grok"));
    }

    #[test]
    fn the_status_tool_is_described_by_status_description() {
        let tools = StandDown::new(GROK_STANDDOWN).tool_router.list_all();
        assert_eq!(tools[0].description.as_deref(), Some(STATUS_DESCRIPTION));
        let a = tools[0].annotations.as_ref().expect("annotated");
        assert_eq!(
            (a.read_only_hint, a.open_world_hint),
            (Some(true), Some(false))
        );
    }
}
