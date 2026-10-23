//! The request body limits of the routes, here so the agent gate
//! (`crate::agent`) can bound an agent's unverified body by its route
//! without depending on the routes.

/// Request body cap for the publish routes. A fully base64-encoded 64 MiB
/// payload is about 85 MiB on the wire, so the cap sits above that;
/// `validate` still enforces the 64 MiB decoded limit.
pub const PUBLISH_BODY_LIMIT: usize = 96 * 1024 * 1024;

/// The most an asset upload's body may hold.
pub const UPLOAD_BODY_LIMIT: usize = 21 * 1024 * 1024;

/// Request cap for thread creation; a clip over `MAX_CLIP_BYTES` but under this
/// is dropped with `clip_error`, a larger request is refused with 413.
pub const THREAD_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Request body cap for `docs:batch`: 50 documents of up to 256 KiB each,
/// with room for JSON escaping. Single-document writes keep axum's 2 MB default.
pub const DOCS_BATCH_LIMIT: usize = 14 * 1024 * 1024;

/// Largest accepted ask body.
pub const ASK_BODY_LIMIT: usize = 128 * 1024;

/// The most body a draft request may carry:
/// [`MAX_DRAFT_BYTES`](clax_core::drafts::MAX_DRAFT_BYTES) of files, with
/// room for JSON escaping and base64.
pub const DRAFT_BODY_LIMIT: usize = 4 * clax_core::drafts::MAX_DRAFT_BYTES;

/// The most a bundle upload's body may hold: the largest bundle a version
/// may carry ([`clax_store::app_bundles::MAX_BUNDLE_BYTES`], 32 MiB) and
/// 64 KiB more, so a bundle just over that cap reaches the store and is
/// refused as `bundle_too_large` there.
pub const BUNDLE_BODY_LIMIT: usize = clax_store::app_bundles::MAX_BUNDLE_BYTES as usize + 64 * 1024;
