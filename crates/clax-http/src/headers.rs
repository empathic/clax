//! Request headers and path pieces that the request-identity modules and
//! the routes both read.

use crate::error::ApiError;
use clax_core::ArtifactId;

/// The artifact ID in a path segment, or 400 for a malformed one.
pub fn parse_id(raw: &str) -> Result<ArtifactId, ApiError> {
    ArtifactId::parse(raw).map_err(ApiError::from)
}

/// Header a shell sets when the page itself publishes (`artifact.publish`):
/// the version event then carries `by_page`.
pub const VIA_HEADER: &str = "x-clax-via";

/// Header naming the session a publish is attributed to.
pub const SESSION_HEADER: &str = "x-clax-session";

/// Where the MCP adapter answers (`clax_server::routes::mcp_http`): `/mcp`
/// on a Durable Object; beside rmcp's `/mcp` in the daemon.
#[cfg(not(target_arch = "wasm32"))]
pub const MCP_JSON_PATH: &str = "/mcp/json";
/// Where the MCP adapter answers (`clax_server::routes::mcp_http`): `/mcp`
/// on a Durable Object; beside rmcp's `/mcp` in the daemon.
#[cfg(target_arch = "wasm32")]
pub const MCP_JSON_PATH: &str = "/mcp";
