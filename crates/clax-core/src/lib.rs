//! Core types and storage for Clax.

pub mod anchor;
pub mod capabilities;
pub mod changelog;
pub mod config;
pub mod db;
pub mod error;
pub mod events;
pub mod feedback;
pub mod home;
pub mod ids;
pub mod mentions;
pub mod model;
pub mod publish;
pub mod store;
pub mod working;
pub mod wrap;

pub use anchor::{Anchor, AnchorKind};
pub use error::{CoreError, Result};
pub use events::{EVENT_BUS_CAPACITY, Event, EventBus};
pub use feedback::{FeedbackItem, FeedbackPhase, FeedbackState, Tier, Touched};
pub use home::Home;
pub use ids::{
    ArtifactId, is_agent_handle, is_public_id, is_ulid, new_agent_handle, new_public_id, new_ulid,
};
pub use publish::html_title;
pub use store::Store;
pub use store::artifacts::{CorruptRow, MetaPatch};
pub use store::attention::{AgentView, Attention, AttentionSummary, Participants, Person};
pub use store::feedback::TakeFeedback;
pub use store::sessions::{Reaped, RegisterSession};
pub use store::threads::{NewComment, NewThread};
