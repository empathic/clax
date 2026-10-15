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
    /// A write whose outcome depends on the content of a document the
    /// caller's reader may not see (`str_replace`): refused before
    /// anything is evaluated, naming the `authors` withheld (spec §8.3).
    #[error("document {path} is withheld until your person approves its authors")]
    DocWithheld {
        path: String,
        authors: Vec<crate::visibility::Author>,
    },
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
    /// An agent's reply, resolve or handoff on a thread another live agent
    /// owns (spec 2026-10-08 §12, M24): `name` is the owner's agent name
    /// (its harness when it has none).
    #[error(
        "this thread is taken by {name}; only its owner agent may reply to, resolve or hand it off"
    )]
    ThreadTaken { name: String },
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
    /// (`clax_store::exec::READ_LIMIT` by default) and was interrupted.
    #[error("a database read ran too long and was stopped")]
    ReadTimeout,
    /// A store job panicked, or the thread that would run it stopped.
    #[error("storage task failed")]
    TaskFailed,
    /// On a store on one connection ([`crate::Store::open_on`]), a job
    /// started inside another job on the same thread (`outer` and `inner`
    /// are `read` or `write`): the connection is busy with the outer one.
    /// The pool runs a read inside a read and a write inside a read; this
    /// fails the one call instead of panicking, which on `wasm32` would
    /// abort the instance.
    #[error("a store {inner} job started inside a {outer} job on the one connection")]
    NestedJob {
        outer: &'static str,
        inner: &'static str,
    },
    /// A synchronous store method on one connection would have had to wait:
    /// for an artifact's content lock, which another task holds across an
    /// await, or for content I/O that is not ready when polled. One thread
    /// cannot wait for either, so the method fails at once; its `async`
    /// twin waits (see the `store` module's Content).
    #[error("the store is busy with {what}; retry, or await the async method")]
    WouldBlock { what: &'static str },
    /// The database's schema (`found`) is newer than this binary knows
    /// (`known`): a newer clax migrated it. Opening it would serve it under
    /// rules that predate it, so the store refuses.
    #[error(
        "this database's schema is version {found}, newer than this clax knows (version {known}); upgrade clax"
    )]
    SchemaNewer { found: u32, known: u32 },
    /// Bringing the database's schema up to date failed at schema version
    /// `at`, or the copy taken before it ([`crate::store::backup`]) could
    /// not be made. The migration that failed was rolled back: the database
    /// is left at `at`, and opening it again fails the same way, so a
    /// supervisor stops restarting on it.
    #[error("could not upgrade the database from schema version {at}: {detail}")]
    MigrationFailed { at: u32, detail: String },
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
