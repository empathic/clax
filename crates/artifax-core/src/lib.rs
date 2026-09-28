//! Core types and storage for Artifax.

pub mod error;
pub mod home;
pub mod ids;

pub use error::{CoreError, Result};
pub use home::Home;
pub use ids::{ArtifactId, new_ulid};
