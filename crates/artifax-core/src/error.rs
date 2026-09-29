//! Error type shared by every core operation.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("not found")]
    NotFound,
    #[error("version conflict: current version is {current}")]
    Conflict { current: u32 },
    #[error("{message}")]
    Invalid { code: &'static str, message: String },
    /// A stored JSON column of `artifact_id` (and, for version columns, of
    /// version `version`) does not parse.
    #[error("corrupt {column} for artifact {artifact_id}{}", version.map(|n| format!(" version {n}")).unwrap_or_default())]
    Corrupt {
        artifact_id: String,
        column: &'static str,
        version: Option<u32>,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

impl CoreError {
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        CoreError::Invalid {
            code,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
