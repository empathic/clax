//! The route handlers, one module per resource. The router that mounts
//! them is `crate::router`; `guards` checks every route against the rules
//! a handler's signature must show.

pub mod admin;
pub mod app_bundles;
pub mod app_drafts;
pub mod approvals;
pub mod artifacts;
pub mod assets;
pub mod audit_trail;
pub mod bot;
pub mod client_imports;
pub mod connect;
pub mod content;
pub mod discovery;
pub mod docs;
pub mod drafts;
#[cfg(not(target_arch = "wasm32"))]
pub mod events;
pub mod extension;
pub mod feedback;
pub mod git_token;
pub mod grants;
pub mod health;
pub mod inbox;
pub mod jobs;
pub mod live;
pub mod live_layers;
pub mod login;
#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;
pub mod mcp_http;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
pub mod people;
pub mod perf;
pub mod permissions;
pub mod questions;
pub mod queue_agent;
pub mod queue_reporter;
pub mod queues;
pub mod remote_apps;
pub mod remote_requests;
pub mod remotes;
pub mod resolve;
pub mod room;
pub mod sample;
pub mod sessions;
pub mod settings;
pub mod site;
pub mod site_api;
pub mod site_signin;
pub mod sites;
pub mod snapshot;
pub mod stream;
pub mod surface;
pub mod tags;
pub mod threads;
pub mod token;
pub mod toolpath;
pub mod viewers;
pub mod watches;
pub mod working;
pub mod world;

#[cfg(test)]
mod guards;
