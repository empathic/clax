//! The errors of this crate, by the codes Clax's API answers with.

use crate::id::ObjectId;

/// What went wrong, named by the Clax error code it maps to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    /// The caller asked for something malformed or not allowed: a bad path,
    /// tag name, author or message, or a tag that already exists.
    #[error("{0}")]
    InvalidArgument(String),
    /// The repository, commit or file is not there.
    #[error("{0}")]
    NotFound(String),
    /// The parent a commit was built on is no longer the head of `main`.
    /// `head` is the head as last seen (`None`: `main` does not exist).
    #[error("{message}")]
    VersionConflict {
        message: String,
        head: Option<ObjectId>,
    },
    /// The git remote refused the credentials (HTTP 401 or 403). A caller
    /// holding a cached token tells this apart to mint another and retry.
    #[error("{message}")]
    Auth { status: u16, message: String },
    /// The repository or remote failed, answered something malformed, or is
    /// missing an object it should hold.
    #[error("{0}")]
    RepoUnavailable(String),
}

impl GitError {
    /// The Clax error code: `invalid_argument`, `not_found`,
    /// `version_conflict` or `repo_unavailable` (an auth refusal is
    /// `repo_unavailable`).
    pub fn code(&self) -> &'static str {
        match self {
            GitError::InvalidArgument(_) => "invalid_argument",
            GitError::NotFound(_) => "not_found",
            GitError::VersionConflict { .. } => "version_conflict",
            GitError::Auth { .. } | GitError::RepoUnavailable(_) => "repo_unavailable",
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        GitError::InvalidArgument(message.into())
    }

    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        GitError::RepoUnavailable(message.into())
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        GitError::NotFound(message.into())
    }
}

pub type Result<T, E = GitError> = std::result::Result<T, E>;
