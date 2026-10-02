//! MCP tools for Clax: an rmcp tool set that calls the daemon's REST API,
//! served by the stdio shim and by the daemon's `/mcp` endpoint.

pub mod client;
pub mod plugin;
pub mod render;
pub mod shim;
pub mod standdown;
pub mod tools;

pub use client::{ClientError, DaemonClient};
pub use tools::ClaxTools;
