//! Core types and storage for Artifax.

pub mod error;
pub mod home;
pub mod ids;
pub mod model;
pub mod publish;
pub mod store;

pub use error::{CoreError, Result};
pub use home::Home;
pub use ids::{ArtifactId, new_ulid};
pub use store::Store;
pub use store::artifacts::MetaPatch;
