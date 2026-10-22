//! MCP tools for Clax: an rmcp tool set that calls the daemon's REST API,
//! served by the stdio shim and by the daemon's `/mcp` endpoint.
//!
//! The tool set and its client build for wasm32 too, for a Durable Object
//! to serve over `http` (MCP's Streamable HTTP answered without rmcp's
//! transport) with a client that reaches its daemon in-process
//! (`client::InProcess`); the stdio shim and its transports (`shim`, `probe`,
//! `standdown`) run on tokio's runtime and are native only, and not built
//! for iOS, where no agent runs beside the app to start them.

pub mod calls;
pub mod channel;
pub mod client;
pub mod git;
pub mod http;
pub mod plugin;
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub mod probe;
pub mod render;
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub mod shim;
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub mod standdown;
pub mod target;
pub mod tools;

pub use client::{ClientError, DaemonClient};
pub use tools::ClaxTools;
