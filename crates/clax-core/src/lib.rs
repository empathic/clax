//! The vocabulary and the rules of Clax that need no database: IDs, the
//! records the store returns and the API serves (`model`), the event bus,
//! configuration and the home directory, signing, the visibility
//! vocabulary, audit records, and the daemon's discovery files. The SQLite
//! store is `clax-store`'s; nothing here runs a query.

pub mod anchor;
pub mod audit;
pub mod capabilities;
pub mod changelog;
pub mod clips;
pub mod config;
pub mod daemon;
pub mod content;
pub mod db;
pub mod embedded;
pub mod draft_diff;
pub mod drafts;
pub mod error;
pub mod events;
pub mod extension;
pub mod feedback;
pub mod gitctx;
pub mod home;
pub mod ids;
pub mod live;
pub mod live_layers;
pub mod mentions;
pub mod model;
pub mod names;
pub mod perf;
pub mod presence;
pub mod publish;
pub mod questions;
pub mod remote;
pub mod rights;
pub mod room;
pub mod seats;
pub mod secret;
pub mod sig;
pub mod time;
pub mod visibility;
pub mod working;
pub mod wrap;

/// The commit the running binary was built from, as the binary sets it at
/// startup ([`set_build_commit`]); `unknown` until then (a library build,
/// a test) or when the build had none.
pub fn build_commit() -> &'static str {
    BUILD_COMMIT.get().copied().unwrap_or("unknown")
}

/// Records the commit the running binary was built from: full lowercase
/// hex, or `unknown`. Embedded by the `clax` binary's build rather than this
/// crate's, so a new commit relinks one crate instead of the workspace. The
/// first call wins.
pub fn set_build_commit(commit: &'static str) {
    let _ = BUILD_COMMIT.set(commit);
}

/// [`build_commit`] shortened to its first seven characters (`unknown` is
/// seven letters).
pub fn build_commit_short() -> &'static str {
    let c = build_commit();
    c.get(..7).unwrap_or(c)
}

static BUILD_COMMIT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

pub use anchor::{Anchor, AnchorKind};
pub use error::{CoreError, Result};
pub use events::{EVENT_BUS_CAPACITY, Event, EventBus, Resume, Stamped};
pub use feedback::{
    FeedbackBatch, FeedbackItem, FeedbackPhase, FeedbackState, ItemWithheld, Notice, Placeholder,
    Tier, Touched,
};
pub use home::Home;
pub use ids::{
    ArtifactId, is_agent_handle, is_public_id, is_ulid, new_agent_handle, new_public_id, new_ulid,
};
pub use model::RegisterSession;
pub use publish::html_title;
pub use secret::Secret;
