//! ClaxBot's workspace at a distance, for a host that cannot hold a container
//! or wait for one (the hosted Worker):
//!
//! - [`wire`]: what such a host and a workspace service say to each other,
//!   which both sides build on;
//! - [`RemoteWorkspace`]: a [`clax_bot::Workspace`] that speaks it over a
//!   [`Transport`], the host's own way of making a request.
//!
//! It builds for native targets and for wasm32, and depends on no runtime of
//! its own: a host supplies the [`Transport`], and where [`Kept`] is kept
//! ([`Keeper`]).

mod remote;
pub mod wire;

pub use remote::{
    Keeper, Kept, LONGEST_TEND_PAUSE, LinkHosts, MAX_PREVIEW_TRIES, MAX_SNAPSHOT_AGE_MS, Method,
    RemoteWorkspace, Transport, Unreachable, WireRequest, WireResponse, lapsed_offer,
};
