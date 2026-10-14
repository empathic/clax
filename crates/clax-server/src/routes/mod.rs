//! The route handlers, one module per resource. The router that mounts
//! them is `crate::router`; `guards` checks every route against the rules
//! a handler's signature must show.

pub mod approvals;
pub mod artifacts;
pub mod assets;
pub mod connect;
pub mod content;
pub mod docs;
pub mod events;
pub mod extension;
pub mod feedback;
pub mod grants;
pub mod health;
pub mod inbox;
pub mod live;
pub mod mcp;
pub mod people;
pub mod perf;
pub mod questions;
pub mod room;
pub mod sample;
pub mod sessions;
pub mod sites;
pub mod snapshot;
pub mod stream;
pub mod threads;
pub mod token;
pub mod toolpath;
pub mod viewers;
pub mod watches;
pub mod working;

#[cfg(test)]
mod guards;
