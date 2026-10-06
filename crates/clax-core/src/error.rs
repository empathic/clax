//! Error type shared by every core operation.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("not found")]
    NotFound,
    #[error("version conflict: current version is {current}")]
    Conflict { current: u32 },
    /// A document write pinned to a version the document no longer has (or,
    /// with `current: None`, pinned to a document that does not exist).
    #[error("document {path} changed: current version is {current:?}")]
    DocConflict { path: String, current: Option<u64> },
    /// A document that is missing, that the caller may not read, or that the
    /// caller may not write: the three are indistinguishable by design. A
    /// missing artifact is [`CoreError::NotFound`] instead.
    #[error("document {path} not found")]
    DocNotFound { path: String },
    /// A write to an existing document without `if_version` and without `lww`.
    #[error("document {path} exists at version {current}; read it and pass if_version")]
    DocPinRequired { path: String, current: u64 },
    /// Write `op` (0-based) of a batch, addressing `path`, failed with
    /// `error`; nothing in the batch landed.
    #[error("batch write {op} ({path}): {error}")]
    InBatch {
        op: usize,
        path: String,
        error: Box<CoreError>,
    },
    #[error("{message}")]
    Invalid { code: &'static str, message: String },
    /// The artifact's current declaration does not include `capability`, which
    /// the request needs.
    #[error("this artifact does not declare the {capability} capability")]
    NotDeclared { capability: &'static str },
    /// A stored JSON column of `artifact_id` (and, for version columns, of
    /// version `version`) does not parse.
    #[error("corrupt {column} for artifact {artifact_id}{}", version.map(|n| format!(" version {n}")).unwrap_or_default())]
    Corrupt {
        artifact_id: String,
        column: &'static str,
        version: Option<u32>,
    },
    /// A read ran longer than the store's read limit
    /// ([`crate::store::exec::READ_LIMIT`] by default) and was interrupted.
    #[error("a database read ran too long and was stopped")]
    ReadTimeout,
    /// A store job panicked, or the thread that would run it stopped.
    #[error("storage task failed")]
    TaskFailed,
    /// The database's schema (`found`) is newer than this binary knows
    /// (`known`): a newer clax migrated it. Opening it would serve it under
    /// rules that predate it, so the store refuses.
    #[error(
        "this database's schema is version {found}, newer than this clax knows (version {known}); upgrade clax"
    )]
    SchemaNewer { found: u32, known: u32 },
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
