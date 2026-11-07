//! ClaxBot's containers behind the wire of [`clax_bot_remote::wire`], for a
//! host of runs that cannot hold one (the hosted Worker). It wraps a
//! [`clax_bot_workspace::workspace::ContainerWorkspace`] and adds what a host
//! at a distance needs:
//!
//! - [`jobs`]: a call that goes on whoever waits for it, known by its key;
//! - [`begun`]: the commands begun, kept across a restart;
//! - [`previews`]: what became of a fix's preview, kept on disk until the host
//!   says it told it;
//! - [`routes`]: the wire itself, behind a bearer secret;
//! - [`settings`]: what the binary reads from its environment.
//!
//! It holds the runtime's key and the forge's, which reach it through the
//! environment. It holds no model key, no key-encryption key, no store and no
//! credential into Clax: the host polls it, never the other way about. It
//! listens on loopback, for a tunnel to front.
//!
//! One service serves one instance, named in its settings and on every
//! request: reaping ends everything of its labels that the host does not name,
//! so a second host with the same secret must reach nothing.

pub mod begun;
pub mod jobs;
pub mod previews;
pub mod routes;
pub mod settings;

pub use jobs::{Clock, Limits, Service};
pub use routes::router;
pub use settings::{BearerToken, Settings};
