//! Core types and storage for Clax.

pub mod anchor;
pub mod audit;
pub mod capabilities;
pub mod changelog;
pub mod config;
pub mod db;
pub mod error;
pub mod events;
pub mod extension;
pub mod feedback;
pub mod gitctx;
pub mod home;
pub mod ids;
pub mod live;
pub mod mentions;
pub mod model;
pub mod perf;
pub mod presence;
pub mod publish;
pub mod questions;
pub mod room;
pub mod store;
pub mod toolpath;
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
    FeedbackBatch, FeedbackItem, FeedbackPhase, FeedbackState, Notice, Tier, Touched,
};
pub use home::Home;
pub use ids::{
    ArtifactId, is_agent_handle, is_public_id, is_ulid, new_agent_handle, new_public_id, new_ulid,
};
pub use publish::html_title;
pub use store::Store;
pub use store::artifacts::{CorruptRow, MetaPatch};
pub use store::attention::{AgentView, Attention, AttentionSummary, Participants, Person};
pub use store::audit::AuditRow;
pub use store::feedback::{SendTarget, TakeFeedback};
pub use store::sessions::{EndedSession, Reaped, RegisterSession};
pub use store::threads::{NewComment, NewThread, ThreadExtras};
