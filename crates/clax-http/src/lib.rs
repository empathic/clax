//! What every route of the daemon shares: its state (`AppState`) and the
//! store calls off the async runtime (`blocking`), the error answer
//! (`ApiError`), who a request speaks for (`auth`, `identity`, `agent`,
//! `grant_auth`, `db_caller`, `viewer`, `extension`), the middleware
//! (`host`, `http_cache`, `live_access`, the audit context and journal),
//! the body limits and shared headers, the listener's socket options
//! (`conn`), the suggestion of where a session asks its person
//! (`surface`), and the JSON views of threads, questions, inbox items,
//! approvals, grants and artifacts (`views`). The routes themselves, the
//! router and `serve` are `clax-server`'s.

pub mod agent;
pub mod audit;
pub mod auth;
pub mod blocking;
#[cfg(not(target_arch = "wasm32"))]
pub mod conn;
pub mod content_policy;
pub mod db_caller;
pub mod error;
pub mod exports;
pub mod extension;
pub mod git_remotes;
pub mod github;
pub mod grant_auth;
pub mod headers;
pub mod host;
pub mod hosted;
pub mod http_cache;
pub mod identity;
pub mod jobs_owner;
pub mod limits;
pub mod live_access;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
#[cfg(not(target_arch = "wasm32"))]
pub mod net_cache;
pub mod perf;
pub mod pushes;
pub mod seats;
pub mod socket_caps;
pub mod state;
pub mod surface;
pub mod viewer;
pub mod views;
pub mod wrap_cache;

pub use state::AppState;
